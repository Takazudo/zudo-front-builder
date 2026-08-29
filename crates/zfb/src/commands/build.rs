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
    apply_prod_asset_pipeline, synthesize_page_id_from_output, validate_companion_file_set,
    AssetEmitterPayload, CompanionFile, ProdAssetEmitterInputs, ProdRenderedFile, RelDistPath,
};
use zfb_build::renderer::{render_all, Backend, RendererInput, RendererOutput};
use zfb_css::{
    css_relative_path, is_tailwind_import_line, AuthoredCssEngine, CssEngine,
    TailwindSubprocessConfig, TailwindSubprocessEngine,
};
#[cfg(test)]
use zfb_islands::scan_islands_with_meta;
use zfb_islands::{
    build_production_client_scripts_with_workers, build_production_islands_asset,
    discover_client_scripts, scan_islands_with_meta_and_first_party_root,
    scan_reachable_modules_with_meta, scan_reachable_modules_with_meta_and_first_party_root,
    BundleConfig, ClientScriptBundleOutput, ClientScriptEntry, ClientScriptWorkerEntry,
    EsbuildSubprocessBundler, EsbuildSubprocessConfig, FrameworkKind, FsResolver, StageAuditPolicy,
};
use zfb_router::Router;

use zfb_render::paths::PathsCache;

use crate::cli::{
    BuildArgs, BuildEmitRenderArtifacts, BuildMinifyHtml, BuildStrictBrokenLinks,
    BuildStrictContentBridge,
};
use crate::commands::css_support::{resolve_framework_css, role_classes_inline_sources};
use crate::commands::resolve::{
    resolve_outdir, resolve_outdir_arg, validate_outdir_safety, wipe_outdir_contents,
};
#[cfg(test)]
use crate::config::CodeHighlightMode;
use crate::config::{Config, OutputMode};
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, check_runtime_installed, embedded_node_modules,
    eval_deferred_paths_via_worker, expand_dynamic_routes, is_ssr_route, DeferredDynamicRoute,
    DynamicResolvedEntry, RouteUniversePlan, WorkerDispatch,
};

/// Entry point for `zfb build`.
///
/// Available only when the `embed_v8` cargo feature is on (issue #371,
/// sub-task 4.1a). The V8-off counterpart further down in this file
/// surfaces a clear runtime error.
#[cfg(feature = "embed_v8")]
pub async fn run(args: &BuildArgs) -> Result<()> {
    let started = Instant::now();
    let timing_enabled = build_timing_enabled();

    let project_root = env::current_dir().context("failed to read current working directory")?;

    // The conventional pages root. The `pages/` dir requirement is
    // relaxed below (#1193): a project with package-owned build routes
    // may ship a truly empty/absent `pages/`, so the hard requirement is
    // re-checked AFTER plugin setup, once we know whether any build
    // routes were registered.
    let pages_dir = project_root.join("pages");

    let phase_started = build_phase_start(timing_enabled);
    let mut config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;
    emit_build_phase_timing("config-load", phase_started);
    let minify_html = resolve_minify_html(args.minify_html(), &config);

    // #2117 — strict-broken-links override. Mutates the OWNED config in
    // place, before any downstream consumer reads it. Every consumer
    // below (preBuild JSON, `run_build`'s bundler-input assembly, the
    // content-snapshot builder, postBuild JSON) reads this SAME
    // `config` binding, so applying the override here — instead of
    // inside the shared `bundler_input.rs` — keeps it build-only and
    // never leaks into `zfb dev`.
    //
    // `config.strict_broken_links` itself is also reset to the
    // resolved value (not just the nested `fail_on_broken` flag): the
    // same `config` is serialised verbatim into the preBuild/postBuild
    // plugin JSON below, and a plugin reading the top-level field
    // should see the EFFECTIVE value CLI precedence produced, not the
    // raw config-file value it overrode (codex review, #2117).
    let strict_broken_links = resolve_strict_broken_links(args.strict_broken_links(), &config);
    config.strict_broken_links = strict_broken_links;
    if strict_broken_links {
        apply_strict_broken_links_override(&mut config);
    }

    // #2220 — strict-content-bridge override. Unlike strict-broken-links,
    // there is no adjacent feature to force-enable (the content-bridge gate
    // always runs for every compiled collection entry), so this is a plain
    // resolve-and-write-back with no `apply_*_override` mutation. Written
    // back to `config` for the same reason as `strict_broken_links` above:
    // the same `config` binding is serialised verbatim into the
    // preBuild/postBuild plugin JSON below, so a plugin reading the
    // top-level field should see the CLI-resolved effective value.
    //
    // The actual bail-on-fallback decision happens later in `run_build`,
    // after the bundler reports `BundlerOutput::content_bridge_fallback_pages`
    // — `zfb_build::bundle()` itself never consults this value, which is
    // what keeps `zfb dev` (which shares that same bundler call) unaffected
    // by construction. See `MaterialiseCtx::content_bridge_fallbacks`'s doc
    // comment in `crates/zfb-build/src/bundler.rs`.
    let strict_content_bridge =
        resolve_strict_content_bridge(args.strict_content_bridge(), &config);
    config.strict_content_bridge = strict_content_bridge;

    // Epic #2421 — emit-render-artifacts override. Same write-back
    // discipline as `strict_broken_links`/`strict_content_bridge` above:
    // the same `config` binding is serialised verbatim into the
    // preBuild/postBuild plugin JSON below, so a plugin reading the
    // top-level field should see the CLI-resolved effective value. No
    // consumer reads `config.emit_render_artifacts` yet — a sibling
    // sub-issue adds the extraction pass and artifact writer.
    let emit_render_artifacts =
        resolve_emit_render_artifacts(args.emit_render_artifacts(), &config);
    config.emit_render_artifacts = emit_render_artifacts;

    let selected_outdir = resolve_outdir_arg(args.outdir.clone(), &config.out_dir);
    let outdir = resolve_outdir(&project_root, &selected_outdir);

    // Sub 3 / #108 — plugin lifecycle. Spawn the host before any heavy
    // work so `preBuild` can prepare files the bundler will see (e.g.
    // claude-resources index emission). If no plugins are declared, we
    // skip the spawn entirely so a config-less project pays nothing.
    let phase_started = build_phase_start(timing_enabled);
    let plugin_host = crate::commands::plugins::maybe_spawn_host(&config).await?;
    emit_build_phase_timing("plugin-host-spawn", phase_started);

    // #255 / #260 / #261 / #268 — shared plugin setup phase:
    // setup → virtual-module prefetch → alias/virtual-module derivation.
    //
    // `SetupCommand::Build` is the per-command difference (dev uses
    // `SetupCommand::Dev`).  As of #1193, `injectRoute` is ACCEPTED
    // during a build — a registered route becomes a package-owned build
    // route the overlay materialiser prerenders (see below) — rather than
    // the pre-#1193 dev-only hard error.
    let phase_started = build_phase_start(timing_enabled);
    let plugin_setup = crate::commands::plugins::run_plugin_setup(
        &plugin_host,
        &project_root,
        &config,
        zfb_build::SetupCommand::Build,
    )
    .await?;
    emit_build_phase_timing("plugin-setup", phase_started);

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
                timing_enabled,
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

// ---------------------------------------------------------------------------
// ZFB_BUILD_TIMING — one-shot build phase timing (issue #2687)
// ---------------------------------------------------------------------------

/// Read `ZFB_BUILD_TIMING` once per build. Truthy values are `1` and `true`
/// (case-insensitive); every other value is off. Each phase gates its
/// `Instant::now()` and stderr formatting on this result, so the default path
/// does not allocate timing state or change the command's output.
fn build_timing_enabled() -> bool {
    std::env::var("ZFB_BUILD_TIMING")
        .ok()
        .as_deref()
        .map(|raw| {
            let value = raw.trim();
            value.eq_ignore_ascii_case("1") || value.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

fn build_phase_start(enabled: bool) -> Option<Instant> {
    enabled.then(Instant::now)
}

fn emit_build_phase_timing(phase: &str, started: Option<Instant>) {
    if let Some(started) = started {
        eprintln!(
            "[zfb-build-timing] phase={phase} elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }
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

/// Resolve the effective strict-broken-links override (issue #2117).
///
/// Same tri-state precedence as [`resolve_minify_html`]: explicit CLI
/// (`--strict-broken` / `--no-strict-broken`) beats the config
/// `strictBrokenLinks` field, which beats the default (`false`).
/// `Config::strict_broken_links` is already defaulted to `false` by
/// serde, so this function returns the single boolean the caller uses
/// to decide whether to force-enable link validation.
pub(crate) fn resolve_strict_broken_links(cli: BuildStrictBrokenLinks, config: &Config) -> bool {
    cli.as_option().unwrap_or(config.strict_broken_links)
}

/// Force-enable `markdown.features.linkValidation.failOnBroken` on the
/// given (already resolved-true) config, in place (issue #2117).
///
/// `link_validation` has TWO optional ancestors — `Config::markdown`
/// and `MarkdownConfig::features` — so the full chain is walked with
/// `get_or_insert_default()` at each step:
///
/// - `markdown == None` → created with defaults.
/// - `features == None` → created with defaults.
/// - `link_validation == None` → created with defaults (epic #2112
///   Decision 1: force-enable — a bare project with no link-validation
///   config at all still gets validation turned on and failing).
/// - `link_validation == Some(cfg)` → only `fail_on_broken` is
///   overridden to `Some(true)`; every other field on `cfg`, and every
///   sibling field on `markdown`/`features`, is left untouched.
pub(crate) fn apply_strict_broken_links_override(config: &mut Config) {
    config
        .markdown
        .get_or_insert_default()
        .features
        .get_or_insert_default()
        .link_validation
        .get_or_insert_default()
        .fail_on_broken = Some(true);
}

/// Resolve the effective strict-content-bridge override (issue #2220).
///
/// Same tri-state precedence as [`resolve_minify_html`] /
/// [`resolve_strict_broken_links`]: explicit CLI (`--strict-content-bridge`
/// / `--no-strict-content-bridge`) beats the config `strictContentBridge`
/// field, which beats the default (`false`). `Config::strict_content_bridge`
/// is already defaulted to `false` by serde, so this function returns the
/// single boolean `run_build` uses to decide whether a reported
/// content-bridge fallback should fail the build.
///
/// Unlike [`resolve_strict_broken_links`], there is no paired
/// `apply_*_override` function — the content-bridge gate always runs for
/// every compiled collection entry, so there is no adjacent feature to
/// force-enable.
pub(crate) fn resolve_strict_content_bridge(
    cli: BuildStrictContentBridge,
    config: &Config,
) -> bool {
    cli.as_option().unwrap_or(config.strict_content_bridge)
}

/// Resolve the effective emit-render-artifacts override (Render Artifact
/// Export epic #2421).
///
/// Same tri-state precedence as [`resolve_minify_html`] /
/// [`resolve_strict_broken_links`] / [`resolve_strict_content_bridge`]:
/// explicit CLI (`--emit-render-artifacts` / `--no-emit-render-artifacts`)
/// beats the config `emitRenderArtifacts` field, which beats the default
/// (`false`). `Config::emit_render_artifacts` is already defaulted to
/// `false` by serde, so this function returns the single boolean the
/// caller writes back into the owned config for downstream stages to read.
///
/// Unlike [`resolve_strict_broken_links`], there is no paired
/// `apply_*_override` function — no other config field needs force-enabling
/// alongside this one.
pub(crate) fn resolve_emit_render_artifacts(
    cli: BuildEmitRenderArtifacts,
    config: &Config,
) -> bool {
    cli.as_option().unwrap_or(config.emit_render_artifacts)
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
    fn timing_enabled(&self) -> bool {
        false
    }

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
    timing_enabled: bool,
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
    fn timing_enabled(&self) -> bool {
        self.timing_enabled
    }

    fn bundle(&self, input: BundlerInput) -> Result<BundlerOutput> {
        let started = build_phase_start(self.timing_enabled);
        let result = bundle(input);
        emit_build_phase_timing("main-esbuild-bundle", started);
        result
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
        let started = build_phase_start(self.timing_enabled);
        let factory =
            crate::v8_host_adapter::make_v8_host_factory_with_hooks(self.v8_plugin_hooks.clone());
        if deferred.is_empty() {
            // No deferred routes: skip host construction entirely. Return the
            // factory so `render_all` can still boot the host for SSG.
            let result = Ok((
                crate::render_pipeline::DynamicExpansion::default(),
                Backend::EmbeddedV8 {
                    host_factory: factory,
                },
                WorkerHandle(None),
            ));
            emit_build_phase_timing("v8-paths-eval", started);
            return result;
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
        let result = Ok((
            expansion,
            Backend::EmbeddedV8 {
                host_factory: factory,
            },
            WorkerHandle(Some(state)),
        ));
        emit_build_phase_timing("v8-paths-eval", started);
        result
    }

    fn render_all(&self, input: RendererInput) -> Result<RendererOutput> {
        let started = build_phase_start(self.timing_enabled);
        let result = render_all(input).map_err(anyhow::Error::from);
        emit_build_phase_timing("v8-boot-and-render", started);
        result
    }

    fn emit_prod_assets(
        &self,
        project_root: &Path,
        user_pages_dir: &Path,
        package_route_entrypoints: &[PathBuf],
        outdir: &Path,
        config: &Config,
    ) -> Result<(ProdAssetEmitterInputs, std::collections::BTreeSet<String>)> {
        let started = build_phase_start(self.timing_enabled);
        // Run `CssPipeline::build_emitter` and
        // `build_production_islands_asset` eagerly (before render) so
        // head injection knows which stable URLs are backed by
        // bytes. Either slot independently returns `None` when the
        // project doesn't exercise it (Tailwind disabled, no
        // `"use client"` components, etc.).
        let css = build_default_css_payload(
            project_root,
            outdir,
            config,
            package_route_entrypoints,
            &self.islands_plugin_config.alias_entries,
            &self.islands_plugin_config.virtual_modules,
        )
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
        let client_scripts = build_default_client_scripts_payloads_with_plugin_config(
            project_root,
            outdir,
            config.framework,
            &self.registered_client_entries,
            config.bundle.as_ref(),
            &self.islands_plugin_config,
        )
        .context("client-script emitters (DefaultRunner) failed")?;
        let result = Ok((
            ProdAssetEmitterInputs {
                css,
                islands,
                client_scripts,
            },
            registered_marker_names,
        ));
        emit_build_phase_timing("production-assets", started);
        result
    }
}

/// Observer invoked with the CSS source-plan seam's computed sibling
/// mirror roots (issue #1802, epic #1799 gap (a)) as soon as they are
/// known — BEFORE the Tailwind subprocess runs and regardless of whether
/// that subprocess later succeeds or fails. This is the seam the dev-watch
/// registration hooks into: a failed boot CSS build must still register
/// sibling watches, or there is no filesystem event through which recovery
/// could ever trigger. See
/// [`build_default_css_payload_with_source_plan`] for exactly when it
/// fires.
pub(crate) type CssSourcePlanObserver<'a> = &'a dyn Fn(&[PathBuf]);

/// The CSS sibling-mirror-root skip-dir name list
/// ([`CSS_SIBLING_MIRROR_SKIP_DIRS`]) exposed to callers outside this
/// module — currently the dev command layer, which threads it down to
/// `zfb_build::OrchestratorConfig::with_css_mirror_skip_dir_names` (issue
/// #1802) so `Watcher::sync_recursive_dir_watches` (issue #1801)
/// suppresses the same infra subtrees the CSS-scan sibling walk already
/// excludes.
pub(crate) fn css_sibling_mirror_skip_dir_names() -> &'static [&'static str] {
    CSS_SIBLING_MIRROR_SKIP_DIRS
}

/// Compatibility wrapper around
/// [`build_default_css_payload_with_source_plan`] for every existing call
/// site (unit tests, `DefaultRunner::emit_prod_assets`) that has no use for
/// the CSS source-plan seam (issue #1802) — byte-identical to the
/// pre-#1802 behaviour, with a no-op observer.
pub(crate) fn build_default_css_payload(
    project_root: &Path,
    outdir: &Path,
    config: &Config,
    package_route_entrypoints: &[PathBuf],
    plugin_alias_entries: &[(String, String)],
    plugin_virtual_modules: &[(String, String)],
) -> Result<Option<AssetEmitterPayload>> {
    build_default_css_payload_with_source_plan(
        project_root,
        outdir,
        config,
        package_route_entrypoints,
        plugin_alias_entries,
        plugin_virtual_modules,
        &|_roots| {},
    )
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
///
/// `on_source_plan` (issue #1802) is called EXACTLY ONCE with the computed
/// sibling mirror roots, on BOTH the Tailwind-enabled and the
/// `tailwind.enabled = false` paths. It is deliberately NOT empty when
/// Tailwind is disabled: `build_authored_only_css_payload` still discovers
/// a claimed sibling's `.module.css` files through the same claim plan
/// (issue #824 — disabling Tailwind opts out of the Tailwind layers, not
/// CSS Modules), so dev-watch registration needs the roots on that path
/// too. On the Tailwind path it is the same slice
/// [`assemble_css_content_globs`] folds into the content-glob list.
///
/// The call happens strictly BEFORE the Tailwind subprocess is invoked, so
/// it fires even when that subprocess later fails — the seam dev-watch
/// registration needs (see the type's own doc comment). The one path that
/// does NOT publish is a `discover_css_plugin_virtual_files` failure, where
/// publishing the narrower alias-only set would retire live roots under
/// replace semantics; see the comment at that call site.
pub(crate) fn build_default_css_payload_with_source_plan(
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
    // Sibling Mirror (issue #1691/#1696): tsconfig/plugin alias targets
    // (claim source b), forwarded to `discover_css_source_files` so a
    // claimed workspace-sibling package's own source files join the CSS
    // Modules scan. Empty on the no-workspace / no-alias path (byte-identical
    // parity).
    plugin_alias_entries: &[(String, String)],
    // Virtual-module claim sources a+c (issue #1775): registered plugin
    // virtual modules (`IslandsPluginConfig::virtual_modules`). A registered
    // virtual module's own sibling closure is discovered via
    // `discover_css_plugin_virtual_files` below and folded into the same
    // `SiblingMirrorPlan` claim `discover_css_source_files` /
    // `compute_css_module_class_maps` already consult for claim source b, so
    // a sibling `.module.css` reached ONLY through a virtual module (no
    // direct alias) is scanned and shipped too. Empty on the no-virtual-
    // module path (byte-identical parity).
    plugin_virtual_modules: &[(String, String)],
    on_source_plan: CssSourcePlanObserver<'_>,
) -> Result<Option<AssetEmitterPayload>> {
    // Build the same `ModuleWorkerBuildContext` shape esbuild will see for
    // this project's plugin registrations, then discover the file set behind
    // any registered virtual module (issue #1775). `production`/`sourcemap`
    // semantics don't affect which files this discovery reaches (they only
    // shape emitted worker bytes elsewhere), so `true` is fine for both the
    // build and dev callers of this function.
    let virtual_worker_context = module_worker_build_context(
        true,
        config.framework,
        config.bundle.as_ref(),
        plugin_alias_entries,
        plugin_virtual_modules,
    );
    let discovered_graph_files =
        match discover_css_plugin_virtual_files(project_root, &virtual_worker_context) {
            Ok(files) => files,
            Err(err) => {
                // Deliberately publish NOTHING here (issue #1799 review).
                //
                // An earlier revision published an alias/tsconfig-derived
                // fallback claim on this path, reasoning that "a failed boot
                // CSS build must still register sibling watches". That is
                // actively harmful: the fallback is a strict SUBSET of the
                // real root set (empty `discovered_graph_files` yields claim
                // source b only, dropping the virtual-module sources a+c),
                // and the whole chain is replace-semantics
                // (`replace_css_mirror_roots` -> `sync_recursive_dir_watches`).
                // Publishing the subset therefore UNWATCHES every root reached
                // only through the virtual-module graph — so if the edit that
                // would fix `err` lives in one of those siblings, no
                // filesystem event can ever arrive to retry. That is exactly
                // the recovery lock this seam exists to prevent.
                //
                // Skipping publication instead preserves the last successful
                // set, which is the documented orchestrator contract: "the
                // registry exposes the last successful closures, so a
                // transient failed rebuild never drops recovery watches"
                // (`zfb-build/src/orchestrator.rs`, boot registration). On a
                // FIRST-boot failure nothing was registered yet, so nothing is
                // lost either — and recovery still arrives through the boot
                // watcher on the project root, whose next in-project edit
                // re-runs discovery.
                return Err(err);
            }
        };

    // `.module.css` files a virtual module imports DIRECTLY (issue #1775
    // follow-up): fed into CSS emission's explicit module slot so a direct
    // virtual→sibling-CSS import ships its rules, matching the class map
    // `compute_css_module_class_maps` produces for the same set. Empty on the
    // no-virtual-module path.
    let direct_css_modules = discovered_direct_css_modules(&discovered_graph_files);

    // Sibling Mirror (issue #1691/#1776): computed here, BEFORE the
    // Tailwind-enabled branch below, and published unconditionally (issue
    // #1802) — `build_authored_only_css_payload` on the `tailwind.enabled =
    // false` path ALSO discovers sibling `.module.css` files through this
    // same claim plan (issue #824: disabling Tailwind opts out of the
    // Tailwind layers, not CSS Modules), so the dev-watch registration
    // needs this set on BOTH paths, not just the Tailwind-scan one. A
    // review finding caught an earlier version of this seam publishing an
    // empty set whenever Tailwind was disabled, which would have left a
    // claimed sibling's CSS Modules unwatched.
    let sibling_mirror_roots: Vec<PathBuf> = zfb_build::SiblingMirrorPlan::compute(
        project_root,
        &zfb_types::first_party_root_for(project_root),
        &discovered_graph_files,
        &read_tsconfig_paths(project_root),
        plugin_alias_entries,
    )
    .mirror_roots()
    .map(Path::to_path_buf)
    .collect();

    // Issue #1802 (epic #1799 gap (a)): publish the mirror roots NOW —
    // before the Tailwind subprocess below ever runs (on the Tailwind-
    // enabled path), and therefore even if that subprocess later fails. A
    // failed boot CSS build must still register sibling watches, or there
    // is no filesystem event through which recovery could ever trigger.
    on_source_plan(&sibling_mirror_roots);

    // `tailwind: { enabled: false }` disables only the Tailwind layers,
    // not the authored-CSS pipeline. Route to the Tailwind-free path so
    // global CSS + CSS Modules still ship (issue #824). Falling back to
    // the Tailwind subprocess path here would re-add the preflight the
    // user opted out of and incur subprocess cost.
    // #1533: the default hi token stylesheet is class-mode-only and
    // classPrefix-aware. Resolve it once here and thread the plain
    // `Option<String>` down — both the authored-only and Tailwind paths
    // funnel through `run_css_emitter`, so neither helper needs the whole
    // `&Config`.
    let framework_css = resolve_framework_css(config);

    let tailwind_enabled = config.tailwind.as_ref().map(|t| t.enabled).unwrap_or(true);
    if !tailwind_enabled {
        return build_authored_only_css_payload(
            project_root,
            outdir,
            framework_css,
            plugin_alias_entries,
            &discovered_graph_files,
            direct_css_modules,
        );
    }

    let sources =
        discover_css_source_files(project_root, plugin_alias_entries, &discovered_graph_files);
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
    let default_content_globs = crate::commands::css_support::default_content_globs(project_root);

    // `sibling_mirror_roots` is computed once above, BEFORE the
    // Tailwind-enabled branch, so the issue #1802 seam can publish it on
    // both paths. It extends Tailwind's own `@source` scan here: without
    // it, a utility class used ONLY inside a claimed sibling file is
    // scanned for CSS Modules discovery but never reaches Tailwind's
    // content scan, so the class would silently never be emitted (green
    // build, unstyled page — the same failure shape fix-A [5] closed for
    // package routes below).
    //
    // Issue #1803 (epic #1799 gap b): `discover_css_source_files` already
    // skips `CSS_SIBLING_MIRROR_SKIP_DIRS` infra dirs when it wholesale-walks
    // a mirror root, but the Tailwind `@source` scan fed by `content_globs`
    // has no equivalent exclusion — an ungitignored generated subtree inside
    // a mirror root (e.g. a stale `dist/`) can leak stale class strings into
    // the emitted stylesheet. Mirror that exclusion onto the engine via
    // `negative_source_globs`.
    let (content_globs, negative_source_globs) = assemble_css_content_globs(
        &default_content_globs,
        package_route_entrypoints,
        &sibling_mirror_roots,
    );

    let tw_cfg = TailwindSubprocessConfig::default()
        .with_working_dir(project_root.to_path_buf())
        .with_content_globs(content_globs)
        .with_negative_source_globs(negative_source_globs)
        .with_inline_sources(role_classes_inline_sources(config));

    // Sub #212 — wire in the embedded-binary extraction tier so consumers
    // running `zfb build` from a project that doesn't ship the
    // `crates/zfb/binaries/` workspace dir still resolve a working tailwind
    // CLI. The TempDir handle rides on the config (and hence the engine)
    // so the extracted file outlives every `produce_utility_css` call.
    let mut tw_cfg = crate::commands::css_support::with_embedded_tailwind_binary(tw_cfg);

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
    let payload = run_css_emitter(
        engine,
        project_root,
        outdir,
        sources,
        direct_css_modules,
        framework_css,
    )?;
    Ok(Some(payload))
}

/// Assemble the Tailwind `@source` content-glob list for a project:
/// `defaults` (the rebased [`zfb_css::engine::DEFAULT_CONTENT_ROOTS`])
/// extended with each package-route entrypoint's parent directory
/// (fix-A [5], #1191) and each Sibling Mirror
/// [`zfb_build::SiblingMirrorPlan`] mirror root (issue #1776), in that
/// order, de-duped. Package-route dirs and mirror roots share one `seen`
/// set so a mirror root that happens to coincide with a package-route dir
/// (or a default root) is not emitted twice. Extracted as a standalone
/// function so this wiring — as opposed to "does `@source` become CSS
/// output" (covered by `zfb-css`'s own tests) — is unit-testable without a
/// real project tree.
///
/// Issue #1803 (epic #1799 gap b): also returns the `@source not`
/// exclusion globs for [`TailwindSubprocessConfig::negative_source_globs`]
/// — one `<root>/**/<skip_dir>/**` glob per (mirror root, skip dir) pair
/// for every entry in [`CSS_SIBLING_MIRROR_SKIP_DIRS`], matching the
/// infra-dir exclusion `discover_css_source_files`'s `filter_entry`
/// already applies when it wholesale-walks the same mirror root. Emitted
/// *only* for mirror roots that are freshly appended above (the same
/// `seen`-gated branch) — a mirror root that dedupes away against a
/// default root or a package-route dir carries no exclusions either,
/// keeping "scope: mirror roots only" exact: package-route dirs (and any
/// root coinciding with one) keep their pre-#1803 behavior untouched.
fn assemble_css_content_globs(
    defaults: &[String],
    package_route_entrypoints: &[PathBuf],
    sibling_mirror_roots: &[PathBuf],
) -> (Vec<String>, Vec<String>) {
    let mut content_globs = defaults.to_vec();
    let mut seen: std::collections::HashSet<String> = content_globs.iter().cloned().collect();
    for entry in package_route_entrypoints {
        if let Some(dir) = entry.parent() {
            let glob = dir.to_string_lossy().into_owned();
            if seen.insert(glob.clone()) {
                content_globs.push(glob);
            }
        }
    }
    let mut negative_source_globs = Vec::new();
    for root in sibling_mirror_roots {
        let glob = root.to_string_lossy().into_owned();
        if seen.insert(glob.clone()) {
            for skip_dir in CSS_SIBLING_MIRROR_SKIP_DIRS.iter() {
                negative_source_globs.push(format!("{glob}/**/{skip_dir}/**"));
            }
            content_globs.push(glob);
        }
    }
    (content_globs, negative_source_globs)
}

/// Build-only adapter around the shared CSS emitter core.
///
/// [`crate::commands::css_support::run_css_emitter`] returns the
/// engine-agnostic [`zfb_css::CssEmitterOutput`] so the standalone `zfb css`
/// command can consume it without depending on production asset-graph types.
/// `zfb build` adapts that output here into its [`AssetEmitterPayload`], at the
/// boundary where CSS companions become `zfb-build` companion files.
fn run_css_emitter<E: CssEngine>(
    engine: E,
    project_root: &Path,
    outdir: &Path,
    sources: Vec<PathBuf>,
    // `.module.css` files a registered virtual module imports DIRECTLY (issue
    // #1775 follow-up, `discovered_direct_css_modules`). The pipeline's
    // auto-discovery only reaches modules imported by a scan `source`; a
    // direct virtual→CSS import has no such source, so these are handed to the
    // explicit `css_modules` slot to be compiled and emitted. Auto-discovered
    // modules are appended after, deduped by the pipeline. Empty on the
    // no-virtual-module path (byte-identical parity).
    explicit_css_modules: Vec<PathBuf>,
    framework_css: Option<String>,
) -> Result<AssetEmitterPayload> {
    let emitter_out = crate::commands::css_support::run_css_emitter(
        engine,
        project_root,
        outdir,
        sources,
        explicit_css_modules,
        framework_css,
    )?;

    // Package-attributed `url()` references the engine resolved and
    // rewrote `emitter_out.bytes` to point at (issue #2316) become CSS
    // companions here — the writer-side boundary where zfb-css's
    // engine-agnostic companion type crosses into zfb-build's
    // `CompanionFile`, mirroring `production_islands_asset_to_payload`'s
    // chunk/worker/resource conversion above.
    let companions = emitter_out
        .companions
        .into_iter()
        .map(|c| CompanionFile {
            filename: c.filename,
            bytes: c.bytes,
        })
        .collect();

    Ok(AssetEmitterPayload {
        bytes: emitter_out.bytes,
        relative_path: css_relative_path(),
        stable_url: emitter_out.stable_url,
        companions,
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
/// [`zfb_css::CssPipeline::build_emitter`].
///
/// Returns `Ok(None)` when the project has neither an authored global
/// stylesheet nor any CSS Modules — the combined output would be
/// whitespace only, and emitting it would leave a broken `<link>` tag
/// in HTML. This mirrors the empty-stylesheet guard on the Tailwind
/// path.
fn build_authored_only_css_payload(
    project_root: &Path,
    outdir: &Path,
    framework_css: Option<String>,
    plugin_alias_entries: &[(String, String)],
    // Virtual-module claim sources a+c (issue #1775) — see
    // `build_default_css_payload`'s parameter doc. The Tailwind-disabled
    // path needs the same threading: `enabled: false` opts out of Tailwind,
    // not out of CSS (issue #824), so a virtual-only sibling CSS Module must
    // still be discovered here.
    discovered_graph_files: &std::collections::BTreeSet<PathBuf>,
    // Direct virtual→`.module.css` imports (issue #1775 follow-up) — see
    // `run_css_emitter`'s `explicit_css_modules` doc. The Tailwind-disabled
    // path needs the same wiring: `enabled: false` opts out of Tailwind, not
    // out of CSS (issue #824), so a directly-imported virtual-only sibling
    // CSS Module must still be compiled and emitted here.
    direct_css_modules: Vec<PathBuf>,
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
            let stripped = strip_tailwind_imports(&raw);
            zfb_css::bundle_authored_css(&path, project_root, &stripped)?
        }
        None => String::new(),
    };

    let sources =
        discover_css_source_files(project_root, plugin_alias_entries, discovered_graph_files);
    let engine = AuthoredCssEngine::new(authored_css);

    let payload = run_css_emitter(
        engine,
        project_root,
        outdir,
        sources,
        direct_css_modules,
        framework_css,
    )?;

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

/// Infra directories skipped when wholesale-walking a claimed sibling
/// mirror root for CSS-scan sources (issue #1696). Mirrors
/// `zfb_build::bundler`'s own `MIRROR_SKIP_DIRS` used to wholesale-mirror
/// the same root into the SSR shadow, so the CSS-side walk and the
/// bundler's real mirror agree on what counts as infra vs. source.
const CSS_SIBLING_MIRROR_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    ".git",
    "target",
    ".turbo",
    ".next",
    ".vercel",
];

/// Push `path` onto `out` iff its extension (case-insensitively) is one of
/// `extensions`. Shared by both walk loops in [`discover_css_source_files`]
/// so the project-root walk (`walkdir`) and the sibling-mirror-root walk
/// (`ignore`) apply the identical filter.
fn push_if_matching_extension(path: PathBuf, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if let Some(ext) = ext {
        if extensions.contains(&ext.as_str()) {
            out.push(path);
        }
    }
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
///
/// Sibling Mirror (issue #1691/#1696): a project file can reach a
/// workspace-sibling COMPONENT through a claimed tsconfig/plugin alias
/// (e.g. `@shared/Button` -> a sibling package). That component's own
/// relative `import styles from "./Button.module.css"` is only found by
/// the scanner below when the component file itself is a scan source —
/// so every claimed [`zfb_build::SiblingMirrorPlan`] mirror root is
/// additionally walked wholesale, same as the bundler's own wholesale
/// sibling mirror. The plan is built from claim source (b) only
/// (tsconfig / plugin alias targets): claim sources (a)/(c) key off the
/// bundler's preprocessing-discovery graph, which does not exist yet at
/// this pre-bundle command-layer call site. Empty (and inert) for a
/// standalone project, so a non-workspace build walks exactly the same
/// files as before.
fn discover_css_source_files(
    project_root: &Path,
    plugin_alias_entries: &[(String, String)],
    // Virtual-module claim sources a+c (issue #1775): every file the
    // registered-virtual-module preprocessing graph reaches, from
    // `discover_css_plugin_virtual_files`. Folded into the same
    // `SiblingMirrorPlan::compute` call as the alias-target claim source
    // (b) below so a sibling reached ONLY through a virtual module (no
    // direct alias) still gets its mirror root wholesale-walked. Empty for
    // a project with no registered virtual modules — byte-identical to
    // before.
    discovered_graph_files: &std::collections::BTreeSet<PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    // page-extension-drift-guard: allow — the CSS/Tailwind source-scan
    // extension set (any file that may contain class names, at any depth,
    // page or not), not the routable page allowlist.
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
            push_if_matching_extension(entry.into_path(), &extensions, &mut out);
        }
    }

    let first_party_root = zfb_types::first_party_root_for(project_root);
    let tsconfig_paths = read_tsconfig_paths(project_root);
    let plan = zfb_build::SiblingMirrorPlan::compute(
        project_root,
        &first_party_root,
        discovered_graph_files,
        &tsconfig_paths,
        plugin_alias_entries,
    );
    for mirror_root in plan.mirror_roots() {
        let walker = ignore::WalkBuilder::new(mirror_root)
            .standard_filters(true) // .gitignore + .git/info/exclude + global gitignore + hidden
            .require_git(false) // honor .gitignore even when the sibling isn't a git repo
            .filter_entry(|entry| {
                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy();
                    if CSS_SIBLING_MIRROR_SKIP_DIRS
                        .iter()
                        .any(|skip| name == *skip)
                    {
                        return false;
                    }
                }
                true
            })
            .build();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            push_if_matching_extension(entry.into_path(), &extensions, &mut out);
        }
    }
    out
}

/// `.module.css` files a registered virtual module's own source imports
/// **directly** (issue #1775 follow-up).
///
/// The virtual-module discovery graph records every file esbuild reaches
/// through a registered virtual module, including a `.module.css` imported
/// with no intermediate JS/TS component. But [`discover_css_source_files`]
/// only returns JS/TS/MD scan *sources*, and the virtual module's source is
/// in-memory — never a scan source — so a direct virtual→sibling-`.module.css`
/// import is otherwise dropped from both the class map and CSS emission while
/// the bundler still rewrites the staged file to `export default {}` (a green
/// build with missing classes and styles). Filtering the shared discovery set
/// keeps it as the only oracle: these are esbuild-visible, already-shipped
/// files, so they are added directly without re-deriving resolution. The
/// [`CSS_SIBLING_MIRROR_SKIP_DIRS`] skip-dir filter matches the sibling walk
/// in [`discover_css_source_files`] so an infra-dir CSS module
/// (`node_modules/`, `dist/`, …) is never picked up. Empty (and inert) for a
/// project with no registered virtual modules.
fn discovered_direct_css_modules(
    discovered_graph_files: &std::collections::BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    discovered_graph_files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".module.css"))
        })
        .filter(|p| {
            !p.components().any(|c| match c {
                std::path::Component::Normal(os) => CSS_SIBLING_MIRROR_SKIP_DIRS
                    .iter()
                    .any(|skip| os == std::ffi::OsStr::new(*skip)),
                _ => false,
            })
        })
        .filter(|p| p.exists())
        .cloned()
        .collect()
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
/// `CssModulesProcessor` with the same workspace-aware hash root (issue
/// #1694's `for_project_and_first_party_roots`, the same config
/// `run_css_emitter` feeds its pipeline), so the scoped names agree
/// without a shared channel.
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
///
/// Sibling Mirror (issue #1691/#1696): a resolved module outside
/// `project_root` is kept only when the [`zfb_build::SiblingMirrorPlan`]
/// actually claims it. `discover_css_source_files` already restricts scan
/// *sources* to project files + claimed sibling files, but a claimed
/// sibling source can still contain a stray relative import that escapes
/// its own claimed region (e.g. `../../another-unclaimed-lib/x.module.css`)
/// — the real SSR shadow never mirrors that region, so including it here
/// would desync the class map from the emitted stylesheet. This is the
/// gate that makes an unclaimed sibling `.module.css` a no-op for CSS
/// output, matching the epic invariant that esbuild-visible reachability
/// (staged trees) is the only thing that ships.
pub(crate) fn compute_css_module_class_maps(
    project_root: &Path,
    plugin_alias_entries: &[(String, String)],
    // Virtual-module claim sources a+c (issue #1775) — see the parameter
    // doc on `discover_css_source_files`. Threaded through to both the
    // source-discovery walk below and this function's own
    // `SiblingMirrorPlan::compute` call, so a `.module.css` resolved only
    // through a registered virtual module's sibling closure is kept
    // instead of silently dropped by the `plan.claims_path` gate below.
    discovered_graph_files: &std::collections::BTreeSet<PathBuf>,
) -> Result<std::collections::HashMap<PathBuf, std::collections::HashMap<String, String>>> {
    use std::collections::HashMap;

    // `.module.css` files a virtual module imports DIRECTLY (no intermediate
    // scan source) — added straight from the shared discovery oracle, see
    // `discovered_direct_css_modules`. Computed up front so a project whose
    // ONLY CSS Module is reached this way (no JS/TS scan sources at all) is
    // not short-circuited by the `sources.is_empty()` gate below.
    let direct_css_modules = discovered_direct_css_modules(discovered_graph_files);

    let sources =
        discover_css_source_files(project_root, plugin_alias_entries, discovered_graph_files);
    if sources.is_empty() && direct_css_modules.is_empty() {
        return Ok(HashMap::new());
    }

    let first_party_root = zfb_types::first_party_root_for(project_root);
    let project_root_norm = zfb_types::normalize_path_lexical(project_root);
    let plan = zfb_build::SiblingMirrorPlan::compute(
        project_root,
        &first_party_root,
        discovered_graph_files,
        &read_tsconfig_paths(project_root),
        plugin_alias_entries,
    );

    // Auto-discovered modules: keep only resolved paths that exist on
    // disk — mirrors `CssPipeline::collect_modules`. Bare specifiers
    // (`@org/pkg/x.module.css`) cannot be compiled by lightningcss and
    // are dropped here too. A path outside `project_root` is kept only
    // when the claim plan stages that sibling region — see the doc
    // comment above.
    let mut module_files: Vec<PathBuf> = if sources.is_empty() {
        Vec::new()
    } else {
        let scan =
            zfb_css::scan_css_module_imports(&sources).context("CSS Modules import scan failed")?;
        scan.modules
            .into_iter()
            .filter(|m| m.exists())
            .filter(|m| m.starts_with(&project_root_norm) || plan.claims_path(m))
            .collect()
    };

    // Fold in the direct virtual→`.module.css` imports (deduped) — these are
    // reachability-proven by the discovery graph, so they bypass the
    // scan/claim gate above.
    for m in direct_css_modules {
        if !module_files.contains(&m) {
            module_files.push(m);
        }
    }

    if module_files.is_empty() {
        return Ok(HashMap::new());
    }

    // Hash scoped names off the project-relative (or, for a claimed
    // sibling, workspace-sibling-relative) path via the shared
    // workspace-aware constructor (issues #825/#1694) — the same config
    // `run_css_emitter` feeds its pipeline, so the scoped names baked into
    // the JSX rewrite match the ones in the emitted `styles-<hash>.css`.
    let processor = zfb_css::CssModulesProcessor::new(
        zfb_css::modules::CssModulesConfig::for_project_and_first_party_roots(
            project_root,
            &first_party_root,
        ),
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
    /// esbuild's working directory inside the shadow: the mirrored project
    /// dir. Equal to the shadow root except in a workspace-widened shadow
    /// (issue #1664), where the project is mirrored at its workspace-relative
    /// location.
    bundle_working_dir: PathBuf,
    /// Logical original terminal targets represented by generated modules.
    raw_targets: std::collections::BTreeSet<PathBuf>,
    /// Logical first-party paths that participate in any module-worker URL
    /// graph: the constructor importer plus the worker entry and its complete
    /// transitive closure (JS/TS helpers, terminal raw assets, CSS, etc.).
    /// Dev invalidation watches this set even when a path lives outside the
    /// default islands roots such as `components/` and `src/`.
    module_worker_dependencies: std::collections::BTreeSet<PathBuf>,
    /// Logical worker entry sources, including entries discovered only behind
    /// an exact plugin alias. The caller emits one companion per source.
    module_worker_sources: std::collections::BTreeSet<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct PluginPreprocessingMeta {
    files: std::collections::BTreeSet<PathBuf>,
    raw_import_edges: std::collections::BTreeSet<zfb_build::ModuleWorkerRawImportEdge>,
    worker_edges: std::collections::BTreeSet<zfb_build::ModuleWorkerEdge>,
    config_dependencies: std::collections::BTreeSet<PathBuf>,
}

impl PluginPreprocessingMeta {
    fn extend(&mut self, other: Self) {
        self.files.extend(other.files);
        self.raw_import_edges.extend(other.raw_import_edges);
        self.worker_edges.extend(other.worker_edges);
        self.config_dependencies.extend(other.config_dependencies);
    }

    fn extend_discovery(&mut self, graph: zfb_build::ModulePreprocessingDiscovery) {
        self.files.extend(graph.files);
        self.raw_import_edges.extend(graph.raw_import_edges);
        self.worker_edges.extend(graph.worker_edges);
        self.config_dependencies.extend(graph.config_dependencies);
    }
}

/// Construct the [`zfb_build::ModuleWorkerBuildContext`] shape esbuild will
/// actually see, from the same bundle options and plugin registrations every
/// caller already threads (mode, loaders, defines, framework JSX source,
/// plugin aliases/virtual modules, output semantics). Shared by the islands
/// bundler wiring and the command-layer CSS discovery pass (issue #1775) so
/// the two flows cannot silently drift on what "the bundler's context"
/// means — this is the ONE place that shape is assembled.
pub(crate) fn module_worker_build_context(
    production: bool,
    framework: crate::config::Framework,
    bundle_config: Option<&crate::config::BundleConfig>,
    plugin_alias_entries: &[(String, String)],
    plugin_virtual_modules: &[(String, String)],
) -> zfb_build::ModuleWorkerBuildContext {
    let jsx_import_source = match framework {
        crate::config::Framework::Preact => zfb_islands::FrameworkKind::Preact,
        crate::config::Framework::React => zfb_islands::FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_loaders = crate::config::resolve_bundle_loaders(bundle_config);
    let bundle_define = crate::config::resolve_bundle_define(bundle_config);
    zfb_build::ModuleWorkerBuildContext::new(
        production,
        &bundle_loaders,
        &bundle_define,
        jsx_import_source,
    )
    .with_plugins(
        plugin_alias_entries.to_vec(),
        plugin_virtual_modules.to_vec(),
    )
    .with_output_semantics(production, !production)
}

/// Discover the file-reachability set behind registered plugin virtual
/// modules (claim sources a+c of [`zfb_build::SiblingMirrorPlan`], issue
/// #1775) for command-layer CSS discovery. Reuses
/// [`discover_plugin_preprocessing`] with `include_registered_virtuals:
/// true` and no extra roots — the SAME resolver-backed discovery the
/// islands/client-script bundlers feed their own claim plans with, never
/// re-derived here. Empty plugin registrations short-circuit inside
/// `discover_plugin_preprocessing` and return an empty set, keeping the
/// no-plugin path byte-identical.
pub(crate) fn discover_css_plugin_virtual_files(
    project_root: &Path,
    worker_build_context: &zfb_build::ModuleWorkerBuildContext,
) -> Result<std::collections::BTreeSet<PathBuf>> {
    Ok(
        discover_plugin_preprocessing(
            project_root,
            std::iter::empty(),
            worker_build_context,
            true,
        )?
        .files,
    )
}

fn discover_plugin_preprocessing(
    project_root: &Path,
    roots: impl IntoIterator<Item = PathBuf>,
    worker_build_context: &zfb_build::ModuleWorkerBuildContext,
    include_registered_virtuals: bool,
) -> Result<PluginPreprocessingMeta> {
    if !worker_build_context.has_plugin_resolver_inputs() {
        return Ok(PluginPreprocessingMeta::default());
    }
    // Issue #1664: entries logicalize against the widened first-party root so
    // workspace-sibling sources are walked instead of silently skipped.
    let first_party_root = zfb_types::first_party_root_for(project_root);
    let paths = IslandsShadowPaths::new(&first_party_root);
    let mut discovered = PluginPreprocessingMeta::default();
    if include_registered_virtuals {
        discovered.extend_discovery(
            zfb_build::discover_registered_virtual_preprocessing_with_context(
                project_root,
                worker_build_context,
            )
            .context("validate registered virtual-module preprocessing syntax")?,
        );
    }
    for root in roots {
        let Some(logical_root) = paths.logical_project_path(&root) else {
            // Installed-package entries remain the documented first-party
            // preprocessing boundary.
            continue;
        };
        let graph = zfb_build::discover_module_preprocessing_with_context(
            &logical_root,
            project_root,
            worker_build_context,
        )
        .with_context(|| {
            format!(
                "discover plugin-resolved preprocessing graph from {}",
                logical_root.display()
            )
        })?;
        discovered.extend_discovery(graph);
    }
    Ok(discovered)
}

fn remap_project_plugin_aliases_to_shadow(
    project_root: &Path,
    shadow_root: &Path,
    aliases: &[(String, String)],
) -> Vec<(String, String)> {
    let paths = IslandsShadowPaths::new(project_root);
    aliases
        .iter()
        .map(|(specifier, target)| {
            let target_path = Path::new(target);
            let remapped = paths
                .project_local_rel(target_path)
                .map(|relative| shadow_root.join(relative))
                .filter(|candidate| candidate.is_file())
                .unwrap_or_else(|| target_path.to_path_buf());
            (specifier.clone(), remapped.to_string_lossy().into_owned())
        })
        .collect()
}

/// Command-layer analog of the `zfb-build` bundler's virtual-module remap
/// (issue #1701, closing the gap #1699/#1700 left in these parallel flows): a
/// registered virtual module whose source absolute-imports a workspace-sibling
/// file must point esbuild at the sibling's STAGED copy under the shadow
/// (`stage_root`), not the live first-party tree. Without this, the esbuild
/// pass bundles the unprocessed live sibling and its `?raw` / nested-worker
/// macros reach esbuild literally, even though the sibling closure above
/// already stages an expanded copy. Mirrors `remap_project_plugin_aliases_to_shadow`
/// for the virtual-module side. Used by all three command-layer esbuild flows:
/// client-script preprocess (production build + dev) and the islands shadow.
///
/// Uses the WORKSPACE-SIBLING-ONLY remap variant (not the SSR bundler's
/// both-tiers form): these flows' stage roots prune hidden / `dist` / `target`
/// dirs, so the project tier's `<project>/pruned/x.ts` → `"./pruned/x.ts"`
/// rewrite would resolve to an unstaged path. The epic is scoped to the
/// workspace tier, so that is exactly what these flows adopt; under-project
/// absolute virtual imports keep their prior (unremapped) behaviour.
/// Bare/relative imports and paths outside the first-party root are left
/// untouched by the underlying remap.
fn remap_project_plugin_virtual_modules_to_shadow(
    project_root: &Path,
    stage_root: &Path,
    virtual_modules: &[(String, String)],
) -> Vec<(String, String)> {
    let first_party_root = zfb_types::first_party_root_for(project_root);
    virtual_modules
        .iter()
        .map(|(specifier, source)| {
            (
                specifier.clone(),
                zfb_build::remap_virtual_module_workspace_sibling_imports_to_shadow(
                    source,
                    project_root,
                    &first_party_root,
                    stage_root,
                ),
            )
        })
        .collect()
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
        if rel.as_os_str().is_empty() {
            return None;
        }
        // Any `node_modules` segment disqualifies — in a workspace-widened
        // root (issue #1664) an installed-package path can carry the segment
        // mid-path (`sub-packages/host/node_modules/...`), not just first.
        if zfb_types::has_node_modules_segment(rel) {
            return None;
        }
        Some(rel.to_path_buf())
    }
}

fn dedup_shadow_paths(
    paths: &IslandsShadowPaths<'_>,
    values: impl IntoIterator<Item = PathBuf>,
) -> std::collections::BTreeSet<PathBuf> {
    values
        .into_iter()
        .map(|path| (paths.path_key(&path), path))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect()
}

fn is_islands_shadow_js_like_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    // page-extension-drift-guard: allow — every JS-LIKE module extension in
    // the islands shadow tree (incl. mjs/cjs/mts/cts), not the page allowlist.
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
///
/// Single source of truth since issue #1726/sub #2160:
/// [`zfb_build::shadow_mirror_prunes_path`] owns the actual DEPTH-DEPENDENT
/// rule (named infra dirs at any depth, `dist`/`target` only at depth 1, a
/// hidden dir at any depth below the root) so the SSR shadow-remap
/// diagnostic and this islands-shadow walk can never drift apart. `entry`
/// carries its own depth relative to `mirror_root` implicitly (`entry.path()`
/// is `mirror_root` joined with the walked-so-far relative path), so passing
/// it straight through reproduces the exact walkdir-depth semantics this
/// function used to compute inline.
fn is_islands_shadow_pruned_dir(mirror_root: &Path, entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    zfb_build::shadow_mirror_prunes_path(mirror_root, entry.path())
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

fn is_typescript_project_config(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("tsconfig") || lower.starts_with("jsconfig")) && lower.ends_with(".json")
}

fn normalize_shadow_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => out.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if out.file_name().is_some() {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            std::path::Component::Normal(segment) => out.push(segment),
        }
    }
    out
}

fn config_extends_values(value: &serde_json::Value) -> Vec<String> {
    match value.get("extends") {
        Some(serde_json::Value::String(extends)) => vec![extends.clone()],
        Some(serde_json::Value::Array(extends)) => extends
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn nearest_typescript_project_config(project_root: &Path, source: &Path) -> Option<PathBuf> {
    let mut dir = source.parent()?;
    loop {
        let tsconfig = dir.join("tsconfig.json");
        if tsconfig.is_file() {
            return Some(tsconfig);
        }
        let jsconfig = dir.join("jsconfig.json");
        if jsconfig.is_file() {
            return Some(jsconfig);
        }
        if dir == project_root {
            return None;
        }
        let parent = dir.parent()?;
        if !parent.starts_with(project_root) {
            return None;
        }
        dir = parent;
    }
}

fn nearest_ancestor_typescript_config(project_root: &Path) -> Option<PathBuf> {
    let mut dir = project_root.parent()?;
    loop {
        let tsconfig = dir.join("tsconfig.json");
        if tsconfig.is_file() {
            return Some(tsconfig);
        }
        let jsconfig = dir.join("jsconfig.json");
        if jsconfig.is_file() {
            return Some(jsconfig);
        }
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

fn internal_shadow_config_path(project_root: &Path, config: &Path) -> Result<Option<PathBuf>> {
    let normalized_root = normalize_shadow_path(project_root);
    let normalized_config = normalize_shadow_path(config);
    let Ok(relative) = normalized_config.strip_prefix(&normalized_root) else {
        return Ok(None);
    };
    // Conscious migration to the canonical whole-path predicate (issue #2300):
    // in production this runs against the WIDENED workspace root, so a nested
    // package config (e.g. `apps/site/node_modules/@scope/pkg/tsconfig.json`)
    // has `apps` as its first component, not `node_modules` — the old
    // `.next()`-only check never excluded it. This mirrors the migration
    // `usable_rel` already received (issues #2051/#2128/#2146, `build.rs`'s
    // `IslandsShadowPaths::usable_rel`).
    //
    // Deliberately unsupported (issue #2322): a path-style `extends` (relative
    // or absolute alike — `config` arrives already resolved) into a workspace
    // SIBLING's unhoisted `node_modules` — the two wholesale stage symlinks
    // cover only the workspace-root and project installs. Bare package-name
    // `extends` is unaffected; a fix adds coverage, not resolution.
    if zfb_types::has_node_modules_segment(relative) {
        return Ok(None);
    }
    let canonical_root = project_root.canonicalize().with_context(|| {
        format!(
            "canonicalize islands shadow project root {}",
            project_root.display()
        )
    })?;
    let canonical_config = normalized_config.canonicalize().with_context(|| {
        format!(
            "canonicalize shadow TypeScript config {}",
            normalized_config.display()
        )
    })?;
    if !canonical_config.starts_with(canonical_root) {
        return Ok(None);
    }
    // Keep the lexical destination (including an in-project symlink name)
    // after canonical containment succeeds. Relative extends spellings then
    // resolve to the same path inside the shadow.
    Ok(Some(project_root.join(relative)))
}

fn collect_shadow_config_chain(
    project_root: &Path,
    config: &Path,
    configs: &mut std::collections::BTreeSet<PathBuf>,
    depth: usize,
) -> Result<()> {
    if depth > 8 {
        return Ok(());
    }
    let Some(logical_config) = internal_shadow_config_path(project_root, config)? else {
        return Ok(());
    };
    if !configs.insert(logical_config.clone()) {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&logical_config)
        .with_context(|| format!("read shadow TypeScript config {}", logical_config.display()))?;
    let cleaned = zfb_plugin_resolver::strip_tsconfig_jsonc(&raw);
    let value: serde_json::Value = serde_json::from_str(&cleaned).with_context(|| {
        format!(
            "parse shadow TypeScript config {}",
            logical_config.display()
        )
    })?;
    for extends in config_extends_values(&value) {
        let Some(parent) = zfb_plugin_resolver::resolve_tsconfig_extends_file(
            logical_config.parent().unwrap_or(project_root),
            &extends,
        ) else {
            continue;
        };
        collect_shadow_config_chain(project_root, &parent, configs, depth + 1)?;
    }
    Ok(())
}

fn shadow_config_target_replacement(
    project_root: &Path,
    shadow_root: &Path,
    base_dir: &Path,
    target: &str,
) -> Option<String> {
    let authored = Path::new(target);
    if authored.is_absolute() {
        // Absolute external aliases already survive relocation. Preserve the
        // authored spelling byte-for-byte; only absolute targets that point
        // back into the project need rebasing into the shadow.
        return rebase_config_path_to_shadow(project_root, shadow_root, authored)
            .map(|path| path.to_string_lossy().into_owned());
    }

    let resolved = zfb_plugin_resolver::resolve_tsconfig_path_target(base_dir, target);
    let resolved_path = Path::new(&resolved);
    Some(
        rebase_config_path_to_shadow(project_root, shadow_root, resolved_path)
            .unwrap_or_else(|| resolved_path.to_path_buf())
            .to_string_lossy()
            .into_owned(),
    )
}

fn rewrite_shadow_config_resolver_options(
    project_root: &Path,
    shadow_root: &Path,
    config: &Path,
    value: &mut serde_json::Value,
) -> bool {
    let parsed = zfb_plugin_resolver::read_tsconfig_paths_file(config);
    let config_dir = config.parent().unwrap_or(project_root);
    let paths_base_dir = parsed
        .as_ref()
        .map(|parsed| parsed.base_dir.clone())
        .unwrap_or_else(|| normalize_shadow_path(config_dir));
    let Some(compiler_options) = value
        .get_mut("compilerOptions")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return false;
    };

    let mut changed = false;
    if let Some(base_url) = compiler_options
        .get("baseUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    {
        let authored = Path::new(&base_url);
        let replacement = if authored.is_absolute() {
            // As with absolute path aliases, a genuinely external absolute
            // baseUrl is already stable and should retain its spelling.
            rebase_config_path_to_shadow(project_root, shadow_root, authored)
        } else {
            let resolved = normalize_shadow_path(&config_dir.join(authored));
            Some(
                rebase_config_path_to_shadow(project_root, shadow_root, &resolved)
                    .unwrap_or(resolved),
            )
        };
        if let Some(replacement) = replacement {
            let replacement = replacement.to_string_lossy().into_owned();
            if replacement != base_url {
                compiler_options.insert(
                    "baseUrl".to_string(),
                    serde_json::Value::String(replacement),
                );
                changed = true;
            }
        }
    }

    if let Some(paths) = compiler_options
        .get_mut("paths")
        .and_then(serde_json::Value::as_object_mut)
    {
        for targets in paths.values_mut() {
            let Some(targets) = targets.as_array_mut() else {
                continue;
            };
            for target in targets {
                let Some(authored) = target.as_str() else {
                    continue;
                };
                let Some(replacement) = shadow_config_target_replacement(
                    project_root,
                    shadow_root,
                    &paths_base_dir,
                    authored,
                ) else {
                    continue;
                };
                if replacement != authored {
                    *target = serde_json::Value::String(replacement);
                    changed = true;
                }
            }
        }
    }

    changed
}

fn islands_shadow_config_bytes(
    project_root: &Path,
    shadow_root: &Path,
    config: &Path,
) -> Result<Vec<u8>> {
    let raw = std::fs::read_to_string(config)
        .with_context(|| format!("read shadow TypeScript config {}", config.display()))?;
    let cleaned = zfb_plugin_resolver::strip_tsconfig_jsonc(&raw);
    let mut value: serde_json::Value = serde_json::from_str(&cleaned)
        .with_context(|| format!("parse shadow TypeScript config {}", config.display()))?;
    let mut changed =
        rewrite_shadow_config_resolver_options(project_root, shadow_root, config, &mut value);
    let extends_values = config_extends_values(&value);
    if extends_values.is_empty() {
        return if changed {
            serde_json::to_vec_pretty(&value).context("serialize rewritten shadow config")
        } else {
            Ok(raw.into_bytes())
        };
    }
    let canonical_root = project_root.canonicalize().with_context(|| {
        format!(
            "canonicalize islands shadow project root {}",
            project_root.display()
        )
    })?;
    let mut has_external_path_extends = false;
    let mut rewritten = Vec::with_capacity(extends_values.len());
    for extends in extends_values {
        let is_path_extends = Path::new(&extends).is_absolute()
            || extends.starts_with("./")
            || extends.starts_with("../")
            || extends.starts_with('/');
        if !is_path_extends {
            // A hoisted monorepo package may be visible above project_root in
            // the real tree but not above an unrelated temp shadow. Resolve
            // it while the original ancestry is available and pin the config
            // file absolutely. Effective paths/baseUrl that point back into
            // the project are overlaid and rebased below.
            if let Some(parent) = zfb_plugin_resolver::resolve_tsconfig_extends_file(
                config.parent().unwrap_or(project_root),
                &extends,
            ) {
                let canonical_parent = parent.canonicalize().with_context(|| {
                    format!("canonicalize package extended config {}", parent.display())
                })?;
                rewritten.push(canonical_parent.to_string_lossy().into_owned());
                changed = true;
                has_external_path_extends = true;
            } else {
                rewritten.push(extends);
            }
            continue;
        }
        let Some(parent) = zfb_plugin_resolver::resolve_tsconfig_extends_file(
            config.parent().unwrap_or(project_root),
            &extends,
        ) else {
            rewritten.push(extends);
            continue;
        };
        let canonical_parent = parent
            .canonicalize()
            .with_context(|| format!("canonicalize extended config {}", parent.display()))?;
        if let Ok(canonical_relative) = canonical_parent.strip_prefix(&canonical_root) {
            if Path::new(&extends).is_absolute() {
                let logical_parent = internal_shadow_config_path(project_root, &parent)?
                    .unwrap_or_else(|| project_root.join(canonical_relative));
                let relative_parent =
                    logical_parent.strip_prefix(project_root).with_context(|| {
                        format!(
                            "internal extended config {} escaped {}",
                            logical_parent.display(),
                            project_root.display()
                        )
                    })?;
                rewritten.push(
                    shadow_root
                        .join(relative_parent)
                        .to_string_lossy()
                        .into_owned(),
                );
                changed = true;
            } else {
                // Relative internal edges retain their spelling; collection
                // mirrors the target at that exact lexical path, including an
                // in-project symlink name.
                rewritten.push(extends);
            }
        } else {
            // A path edge that leaves the project cannot keep the same
            // spelling after its leaf moves under a tempdir. Its effective
            // baseUrl/paths may still point back into this project, so those
            // resolver fields are rebased below as a local shadow override.
            rewritten.push(canonical_parent.to_string_lossy().into_owned());
            changed = true;
            has_external_path_extends = true;
        }
    }
    if !changed {
        return Ok(raw.into_bytes());
    }
    value["extends"] = if matches!(value.get("extends"), Some(serde_json::Value::Array(_))) {
        serde_json::Value::Array(
            rewritten
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        )
    } else {
        serde_json::Value::String(
            rewritten
                .into_iter()
                .next()
                .expect("non-empty extends values"),
        )
    };

    if has_external_path_extends {
        let parsed = zfb_plugin_resolver::read_tsconfig_paths_file(config);
        let mut paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(config);
        let paths_rebased = rebase_config_paths_to_shadow(project_root, shadow_root, &mut paths);
        let rebased_base_url = parsed
            .as_ref()
            .and_then(|parsed| parsed.base_url.as_deref())
            .and_then(|base_url| rebase_config_path_to_shadow(project_root, shadow_root, base_url));
        if paths_rebased || rebased_base_url.is_some() {
            let compiler_options = value
                .as_object_mut()
                .expect("parsed tsconfig root is an object")
                .entry("compilerOptions")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if !compiler_options.is_object() {
                *compiler_options = serde_json::Value::Object(serde_json::Map::new());
            }
            let compiler_options = compiler_options
                .as_object_mut()
                .expect("compilerOptions was normalized to an object");
            if !paths.is_empty() {
                compiler_options.insert("paths".to_string(), serde_json::to_value(paths)?);
            }
            if let Some(base_url) = rebased_base_url {
                compiler_options.insert(
                    "baseUrl".to_string(),
                    serde_json::Value::String(base_url.to_string_lossy().into_owned()),
                );
            }
        }
    }
    serde_json::to_vec_pretty(&value).context("serialize rewritten shadow config")
}

fn collect_islands_shadow_configs<'a>(
    project_root: &Path,
    sources: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<std::collections::BTreeSet<PathBuf>> {
    let mut configs = std::collections::BTreeSet::new();
    for source in sources {
        if let Some(config) = nearest_typescript_project_config(project_root, source) {
            collect_shadow_config_chain(project_root, &config, &mut configs, 0)?;
        }
    }
    Ok(configs)
}

fn rebase_config_path_to_shadow(
    project_root: &Path,
    shadow_root: &Path,
    path: &Path,
) -> Option<PathBuf> {
    fn canonicalize_existing_prefix(path: &Path) -> Option<PathBuf> {
        let mut existing = normalize_shadow_path(path);
        let mut suffix = Vec::new();
        while !existing.exists() {
            suffix.push(existing.file_name()?.to_os_string());
            if !existing.pop() {
                return None;
            }
        }
        let mut canonical = existing.canonicalize().ok()?;
        for component in suffix.into_iter().rev() {
            canonical.push(component);
        }
        Some(normalize_shadow_path(&canonical))
    }

    let lexical_root = normalize_shadow_path(project_root);
    let canonical_root = project_root
        .canonicalize()
        .map(|path| normalize_shadow_path(&path))
        .unwrap_or_else(|_| lexical_root.clone());
    let normalized = normalize_shadow_path(path);
    let canonical_candidate = canonicalize_existing_prefix(&normalized);
    let relative = if let Ok(relative) = normalized.strip_prefix(&lexical_root) {
        // Preserve an authored in-project symlink spelling when its physical
        // target also stays inside the project. A symlink escape is genuinely
        // external and must remain an absolute real-tree resolver target.
        if canonical_candidate
            .as_ref()
            .is_some_and(|candidate| !candidate.starts_with(&canonical_root))
        {
            return None;
        }
        relative
    } else {
        canonical_candidate
            .as_deref()
            .unwrap_or(&normalized)
            .strip_prefix(&canonical_root)
            .ok()?
    };
    if relative.as_os_str().is_empty() {
        Some(shadow_root.to_path_buf())
    } else {
        Some(shadow_root.join(relative))
    }
}

fn rebase_config_paths_to_shadow(
    project_root: &Path,
    shadow_root: &Path,
    paths: &mut std::collections::BTreeMap<String, Vec<String>>,
) -> bool {
    let mut changed = false;
    for targets in paths.values_mut() {
        for target in targets {
            let (path_part, wildcard) = target
                .strip_suffix("/*")
                .map(|path| (path, "/*"))
                .unwrap_or((target.as_str(), ""));
            let Some(rebased_path) =
                rebase_config_path_to_shadow(project_root, shadow_root, Path::new(path_part))
            else {
                continue;
            };
            let mut rebased = rebased_path.to_string_lossy().into_owned();
            rebased.push_str(wildcard);
            *target = rebased;
            changed = true;
        }
    }
    changed
}

fn shadow_boundary_config_bytes(project_root: &Path, shadow_root: &Path) -> Result<Vec<u8>> {
    let mut root = serde_json::Map::new();
    root.insert(
        "//".to_string(),
        serde_json::Value::String(
            "Synthetic zfb preprocessing-shadow boundary config.".to_string(),
        ),
    );
    if let Some(ancestor) = nearest_ancestor_typescript_config(project_root) {
        let canonical_ancestor = ancestor.canonicalize().with_context(|| {
            format!(
                "canonicalize ancestor TypeScript config {}",
                ancestor.display()
            )
        })?;
        root.insert(
            "extends".to_string(),
            serde_json::Value::String(canonical_ancestor.to_string_lossy().into_owned()),
        );
        let parsed = zfb_plugin_resolver::read_tsconfig_paths_file(&canonical_ancestor);
        let mut paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&canonical_ancestor);
        let paths_rebased = rebase_config_paths_to_shadow(project_root, shadow_root, &mut paths);
        let rebased_base_url = parsed
            .as_ref()
            .and_then(|parsed| parsed.base_url.as_deref())
            .and_then(|base_url| rebase_config_path_to_shadow(project_root, shadow_root, base_url));
        if paths_rebased || rebased_base_url.is_some() {
            let mut compiler_options = serde_json::Map::new();
            if !paths.is_empty() {
                compiler_options.insert("paths".to_string(), serde_json::to_value(paths)?);
            }
            if let Some(base_url) = rebased_base_url {
                compiler_options.insert(
                    "baseUrl".to_string(),
                    serde_json::Value::String(base_url.to_string_lossy().into_owned()),
                );
            }
            root.insert(
                "compilerOptions".to_string(),
                serde_json::Value::Object(compiler_options),
            );
        }
    }
    serde_json::to_vec_pretty(&serde_json::Value::Object(root))
        .context("serialize preprocessing-shadow boundary config")
}

fn materialise_shadow_typescript_configs(
    project_root: &Path,
    shadow_root: &Path,
    configs: &std::collections::BTreeSet<PathBuf>,
) -> Result<()> {
    for config in configs {
        let rel = config.strip_prefix(project_root).with_context(|| {
            format!(
                "shadow TypeScript config {} is outside {}",
                config.display(),
                project_root.display()
            )
        })?;
        if rel.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        }) {
            return Err(anyhow!(
                "shadow TypeScript config destination escaped project root: {}",
                config.display()
            ));
        }
        let to = shadow_root.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create shadow config dir {}", parent.display()))?;
        }
        let bytes = islands_shadow_config_bytes(project_root, shadow_root, config)?;
        std::fs::write(&to, bytes).with_context(|| {
            format!(
                "write shadow TypeScript config {} -> {}",
                config.display(),
                to.display()
            )
        })?;
    }

    // Stop esbuild at the shadow boundary. If the project has no root config,
    // native discovery must not continue into the tempdir's unrelated
    // ancestors. Preserve a real monorepo ancestor explicitly when one exists;
    // otherwise an empty config is the intentional boundary.
    if !shadow_root.join("tsconfig.json").is_file() && !shadow_root.join("jsconfig.json").is_file()
    {
        std::fs::write(
            shadow_root.join("tsconfig.json"),
            shadow_boundary_config_bytes(project_root, shadow_root)?,
        )
        .context("write preprocessing-shadow boundary tsconfig")?;
    }
    Ok(())
}

fn shadow_config_scope_uses_paths(
    project_root: &Path,
    configs: &std::collections::BTreeSet<PathBuf>,
) -> bool {
    configs
        .iter()
        .any(|config| !zfb_plugin_resolver::read_tsconfig_paths_file_into_map(config).is_empty())
        || nearest_ancestor_typescript_config(project_root).is_some_and(|config| {
            !zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&config).is_empty()
        })
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
#[cfg(test)]
fn materialise_islands_shadow(
    project_root: &Path,
    islands: &[zfb_islands::Island],
    scan_meta: &zfb_islands::ScanMeta,
) -> Result<IslandsShadowOutcome> {
    materialise_islands_shadow_with_worker_context(
        project_root,
        islands,
        scan_meta,
        &zfb_build::ModuleWorkerBuildContext::default(),
        &PluginPreprocessingMeta::default(),
    )
}

fn materialise_islands_shadow_with_worker_context(
    project_root: &Path,
    islands: &[zfb_islands::Island],
    scan_meta: &zfb_islands::ScanMeta,
    worker_build_context: &zfb_build::ModuleWorkerBuildContext,
    plugin_preprocessing: &PluginPreprocessingMeta,
) -> Result<IslandsShadowOutcome> {
    use std::collections::{BTreeSet, HashMap};

    // Issue #1664: in a pnpm workspace the mirrorable first-party tree is the
    // whole workspace, so sibling-package sources reached through tsconfig
    // aliases are mirrored instead of becoming stopgap offenders. The shadow
    // layout is keyed relative to this widened root; without a workspace
    // marker it is exactly `project_root` and the layout is unchanged.
    let first_party_root = zfb_types::first_party_root_for(project_root);
    let root = first_party_root.as_path();
    let paths = IslandsShadowPaths::new(root);
    // Issue #2163: the project's OWN boundary, kept separate from the
    // widened first-party root above so the copy-mode trigger below can tell
    // "project-local" apart from "workspace sibling" — mirrors the
    // client-script preprocessing stage's `paths`/`project_paths` pair.
    let project_paths = IslandsShadowPaths::new(project_root);

    // Issue #1703, Stage Escape Guards — Guard (a): a bare package-name
    // import of a first-party workspace sibling resolves through the
    // wholesale `node_modules` symlink this shadow sets up below straight
    // to the UNPROCESSED source, silently bypassing whatever `?raw` /
    // module-worker / `import.meta.glob` rewrite this shadow exists to
    // stage. `scan_meta.workspace_package_edges_from_islands` is already
    // scoped to the island-reachable graph (never a server-only import —
    // it is projected by the same forward walk as
    // `raw_import_edges_from_islands`), and this function only ever runs
    // once the caller has determined staging is needed for this closure,
    // so a project with no glob/raw/worker preprocessing never reaches
    // this check.
    if let Some(edge) = scan_meta.workspace_package_edges_from_islands.first() {
        return Err(anyhow!(
            "island module {} imports \"{}\" by its workspace-package name, but this island \
             graph requires `?raw`/module-worker/`import.meta.glob` shadow staging; a \
             package-name import resolves through the live node_modules symlink to the \
             unprocessed source and silently bypasses the staged rewrite — use a tsconfig \
             alias or relative import to reach a workspace sibling; package-name imports of \
             first-party siblings are not supported once staging is active",
            edge.importer.display(),
            edge.specifier
        ));
    }

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
    let mut all_raw_edges: BTreeSet<zfb_build::ModuleWorkerRawImportEdge> = scan_meta
        .raw_import_edges_from_islands
        .iter()
        .map(|edge| zfb_build::ModuleWorkerRawImportEdge {
            importer: edge.importer.clone(),
            target: edge.target.clone(),
        })
        .collect();
    all_raw_edges.extend(plugin_preprocessing.raw_import_edges.iter().cloned());
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
    for file in &plugin_preprocessing.files {
        if paths.project_local_rel(file).is_some() {
            to_mirror.insert(file.clone());
        }
    }
    // (b) every file under each glob module's directory subtree (its matched
    //     targets live here).
    for g in &expanded_glob_modules {
        let dir = g.parent().unwrap_or(root);
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_islands_shadow_pruned_dir(dir, e))
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
        // Raw-alias table stays scoped to the project's own tsconfig; the
        // containment bound is widened inside `validate_raw_candidate`.
        let resolver = FsResolver::new().with_project_root(project_root);
        match scan_reachable_modules_with_meta(&target_roots, &resolver) {
            Ok(meta) => {
                for m in meta.modules {
                    if paths.project_local_rel(&m).is_some() {
                        to_mirror.insert(m);
                    }
                }
                all_raw_edges.extend(meta.raw_import_edges.into_iter().map(|edge| {
                    zfb_build::ModuleWorkerRawImportEdge {
                        importer: edge.importer,
                        target: edge.target,
                    }
                }));
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
    let raw_importers = dedup_shadow_paths(
        &paths,
        all_raw_edges.iter().map(|edge| edge.importer.clone()),
    );
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
                root.display()
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
    let worker_importers = dedup_shadow_paths(
        &paths,
        scan_meta
            .module_worker_edges_from_islands
            .iter()
            .map(|edge| edge.importer.clone())
            .chain(
                plugin_preprocessing
                    .worker_edges
                    .iter()
                    .map(|edge| edge.importer.clone()),
            ),
    );
    let mut module_worker_dependencies: BTreeSet<PathBuf> = BTreeSet::new();
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
                root.display()
            )
        })?;
        module_worker_dependencies.insert(logical_importer.clone());
        let source = match expanded_by_key.get(&key) {
            Some(expanded) => expanded.clone(),
            None => std::fs::read_to_string(&importer)
                .with_context(|| format!("read module-worker importer {}", importer.display()))?,
        };
        match zfb_build::rewrite_module_worker_urls_with_context(
            &source,
            &logical_importer,
            project_root,
            worker_build_context,
        ) {
            Ok(rewrite) => {
                expanded_by_key.insert(key, rewrite.expanded_source);
                to_mirror.insert(importer.clone());
                for dependency in rewrite.dependencies {
                    match paths.project_local_rel(&dependency.dependency) {
                        Some(_) => {
                            if let Some(logical_dependency) =
                                paths.logical_project_path(&dependency.dependency)
                            {
                                module_worker_dependencies.insert(logical_dependency);
                            }
                            to_mirror.insert(dependency.dependency);
                        }
                        None => offenders.push(format!(
                            "{} — module-worker dependency of {} is outside the mirrorable first-party project tree",
                            dependency.dependency.display(),
                            importer.display()
                        )),
                    }
                }
                for config in rewrite.config_dependencies {
                    let watched_config = paths
                        .logical_project_path(&config.dependency)
                        .unwrap_or(config.dependency);
                    module_worker_dependencies.insert(watched_config);
                }
            }
            Err(error) => offenders.push(format!("{}: {error:#}", importer.display())),
        }
    }
    for config in &plugin_preprocessing.config_dependencies {
        let watched_config = paths
            .logical_project_path(config)
            .unwrap_or_else(|| config.clone());
        module_worker_dependencies.insert(watched_config);
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

    // Issue #2163: a genuine workspace sibling anywhere in the mirrored
    // closure (`to_mirror` is complete at this point) forces real-file
    // materialisation below, regardless of `node_modules`/tsconfig `paths` —
    // nothing in the copy path requires `node_modules` to exist. Same
    // containment test `consider_sibling` uses on the client-script
    // preprocessing stage: project-local under the widened first-party root,
    // but outside the project's own root.
    let sibling_present = to_mirror.iter().any(|physical| {
        paths.project_local_rel(physical).is_some()
            && project_paths.project_local_rel(physical).is_none()
    });

    // Mirror the closest config for every executable source plus its relative
    // extends chain. Configs are real copies, not symlinks: their `baseUrl`
    // and `paths` substitutions must be anchored in the rewritten shadow so
    // aliases reach expanded `?raw` / module-worker importers instead of
    // escaping back to untouched project files. Keeping every source's
    // closest leaf preserves nested tsconfig/jsconfig scope.
    let mut config_sources: BTreeSet<PathBuf> = to_mirror
        .iter()
        .filter_map(|source| paths.logical_project_path(source))
        .collect();
    config_sources.extend(
        islands
            .iter()
            .filter_map(|island| paths.logical_project_path(&island.source_path)),
    );
    let shadow_configs = collect_islands_shadow_configs(root, &config_sources)?;

    // --- Materialise. ----------------------------------------------------
    let tempdir = tempfile::Builder::new()
        .prefix("zfb-islands-shadow-")
        .tempdir()
        .context("failed to allocate islands shadow tempdir")?;
    let shadow_root = tempdir.path();
    // A workspace-widened shadow needs node_modules at BOTH install roots:
    // the workspace root (hoisted deps) and the project's own nested dir.
    // Without a workspace the two are the same lookup and behavior is
    // unchanged.
    let first_party_node_modules = detect_project_node_modules(root);
    let project_node_modules = if root == project_root {
        None
    } else {
        detect_project_node_modules(project_root)
    };
    let has_node_modules = first_party_node_modules.is_some() || project_node_modules.is_some();
    let source_copy_mode = sibling_present
        || (has_node_modules && shadow_config_scope_uses_paths(root, &shadow_configs));
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

    materialise_shadow_typescript_configs(root, shadow_root, &shadow_configs)?;

    // Symlink node_modules as a whole so shadow files' bare imports
    // (`preact`, `@takazudo/zfb/runtime`, …) resolve — esbuild walks up from
    // each shadow file to the nearest mirrored `node_modules`. In a widened
    // workspace shadow, both the workspace-root install and the project's
    // nested install are linked so nearest-package precedence is preserved.
    //
    // Issue #1682 (shared with the client-script preprocessing stage below):
    // a sibling imported through its pnpm PACKAGE NAME resolves through this
    // live node_modules symlink to the unprocessed source, bypassing the
    // staged rewrite. Sibling reach via tsconfig alias / relative path
    // (what #1674 covers) resolves to the staged files instead. Guarded by
    // this epic (#1702): guard (a) (issue #1703, checked earlier in this
    // function against `scan_meta.workspace_package_edges_from_islands`)
    // pre-flight rejects the escape before this symlink is even created;
    // guard (b) (issue #1705/#1707) is the esbuild-time backstop — a
    // per-subprocess metafile audit that rejects it even if a lower-level
    // bundler invocation ever bypassed guard (a).
    if let Some(nm) = first_party_node_modules {
        let shadow_nm = shadow_root.join("node_modules");
        shadow_symlink(&nm, &shadow_nm).with_context(|| {
            format!(
                "symlink shadow node_modules {} -> {}",
                shadow_nm.display(),
                nm.display()
            )
        })?;
    }
    let shadow_project_dir =
        match zfb_types::normalize_path_lexical(project_root).strip_prefix(root) {
            Ok(rel) if !rel.as_os_str().is_empty() => shadow_root.join(rel),
            _ => shadow_root.to_path_buf(),
        };
    std::fs::create_dir_all(&shadow_project_dir)
        .with_context(|| format!("create shadow project dir {}", shadow_project_dir.display()))?;
    if let Some(nm) = project_node_modules {
        let shadow_nm = shadow_project_dir.join("node_modules");
        shadow_symlink(&nm, &shadow_nm).with_context(|| {
            format!(
                "symlink shadow project node_modules {} -> {}",
                shadow_nm.display(),
                nm.display()
            )
        })?;
    }
    // The user's selected tsconfig/jsconfig hierarchy is now present in the
    // shadow. The caller uses the mirrored project dir as esbuild's cwd (and
    // the shadow root as the tsconfig search boundary), so implicit config
    // lookup and plugin-merged per-entry configs agree on shadow paths.

    // --- Remap island source_paths into the shadow. ----------------------
    let mut remap: HashMap<PathBuf, PathBuf> = HashMap::new();
    for island in islands {
        if let Some(rel) = paths.project_local_rel(&island.source_path) {
            remap.insert(island.source_path.clone(), shadow_root.join(rel));
        }
    }
    let module_worker_sources = scan_meta
        .module_worker_edges_from_islands
        .iter()
        .map(|edge| edge.source_path.clone())
        .chain(
            plugin_preprocessing
                .worker_edges
                .iter()
                .map(|edge| edge.source_path.clone()),
        )
        .filter_map(|source| paths.logical_project_path(&source))
        .collect();

    Ok(IslandsShadowOutcome::Ready(IslandsShadow {
        _tempdir: tempdir,
        remap,
        preserve_symlinks,
        bundle_working_dir: shadow_project_dir,
        raw_targets: all_raw_edges
            .into_iter()
            .filter_map(|edge| paths.logical_project_path(&edge.target))
            .collect(),
        module_worker_dependencies,
        module_worker_sources,
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
#[allow(clippy::too_many_arguments)] // 8 params: #1497 added raw_invalidation; thin test-only shim over the _with_bundle_options variant below
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

/// Issue #1707 (Stage Escape Guards): build the guard (b) stage-escape audit
/// policy for an islands / client-scripts esbuild config — but ONLY when
/// [`zfb_types::stage_escape_audit_eligibility`] (issue #1986) says a
/// first-party stage escape is structurally possible for this build.
///
/// Issue #1730/#1987: this used to gate on the widened-stage proxy alone
/// (`first_party_root != normalize(project_root)`), which reads a workspace
/// whose `pnpm-workspace.yaml` claims its own root (`packages: ['.',
/// 'packages/*']`) as "not a workspace" — building FROM the workspace root
/// makes `first_party_root_for` return `project_root` itself, so the stage
/// never widens even though the wholesale `node_modules` symlink this stage
/// sets up reaches first-party siblings exactly as it does from a nested
/// member. The eligibility predicate is a strict superset of that proxy (see
/// its module docs for the full decision table): every currently-armed build
/// stays armed, and a root-claimed workspace with a reachable first-party
/// `node_modules` link now arms too. `stage_root.join("node_modules")` is the
/// wholesale symlink every call site below sets up (to `first_party_root`'s
/// live install), so it is what the predicate scans.
///
/// Outside a workspace, or inside one with nothing first-party reachable
/// under `node_modules`, the audit stays pure overhead — the dev loop is
/// latency-sensitive (`dev_sibling_watch_1678_e2e` guards this pipeline) —
/// and the policy stays `None`, leaving the argv byte-identical (no
/// `--metafile`).
///
/// When eligible, the audit is armed with `stage_root` as the sole stage
/// boundary (every legitimately staged input — the mirrored project plus the
/// wholesale-mirrored siblings — lives under it) and `first_party_root` as the
/// live-source boundary a package-name escape climbs to. `metafile_cwd` is not
/// passed here: [`EsbuildSubprocessConfig`] already runs esbuild from — and
/// audits against — its `working_dir` (the stage's `bundle_working_dir`,
/// nested below `stage_root`), wired in issue #1705.
fn stage_escape_audit_policy(
    project_root: &Path,
    first_party_root: &Path,
    stage_root: &Path,
) -> Option<StageAuditPolicy> {
    let node_modules_dir = stage_root.join("node_modules");
    if !zfb_types::stage_escape_audit_eligibility(project_root, first_party_root, &node_modules_dir)
        .is_eligible()
    {
        return None;
    }
    Some(StageAuditPolicy {
        stage_roots: vec![stage_root.to_path_buf()],
        first_party_root: first_party_root.to_path_buf(),
    })
}

#[cfg(test)]
mod stage_escape_audit_policy_tests {
    use super::*;

    #[test]
    fn none_when_not_widened_even_with_an_unnormalized_project_root() {
        // Outside a workspace `first_party_root_for` hands back the NORMALIZED
        // project root, so the widened check compares against that — a raw
        // `!= project_root` would false-positive on an unnormalized path. An
        // unnormalized project_root that normalizes to the first-party root is
        // NOT widened → no policy → no `--metafile` in the argv.
        let project_root = Path::new("/proj/./sub/..");
        let first_party_root = zfb_types::normalize_path_lexical(project_root); // "/proj"
        assert!(stage_escape_audit_policy(
            project_root,
            &first_party_root,
            Path::new("/tmp/stage")
        )
        .is_none());
    }

    #[test]
    fn some_with_correct_roots_when_widened() {
        // A pnpm-workspace build: first_party_root is a proper ancestor of
        // project_root → the stage widened → audit armed with the stage root
        // as the boundary and the workspace root as the first-party root.
        let project_root = Path::new("/ws/packages/app");
        let first_party_root = Path::new("/ws");
        let stage_root = Path::new("/tmp/zfb-stage-abc/ws");
        let policy = stage_escape_audit_policy(project_root, first_party_root, stage_root)
            .expect("a stage widened past project_root must arm the audit");
        assert_eq!(policy.stage_roots, vec![stage_root.to_path_buf()]);
        assert_eq!(policy.first_party_root, first_party_root.to_path_buf());
    }

    /// Issue #1987 (Wave 5): the #1730 root-claimed-workspace case the old
    /// widened-stage proxy read as "not a workspace". `project_root` IS the
    /// workspace root (never widened), but a reachable first-party symlink
    /// under `stage_root/node_modules` — the wholesale link every call site
    /// sets up — now arms the audit via
    /// `zfb_types::stage_escape_audit_eligibility`'s row 3, exactly the
    /// eligibility fixture proven in `zfb-types`.
    #[cfg(unix)]
    #[test]
    fn some_when_root_workspace_has_a_reachable_first_party_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages: ['.', 'packages/*']\n",
        )
        .unwrap();
        let child = root.join("packages/child");
        std::fs::create_dir_all(&child).unwrap();
        let node_modules = root.join("node_modules");
        std::fs::create_dir_all(node_modules.join("@scope")).unwrap();
        std::os::unix::fs::symlink(&child, node_modules.join("@scope/child")).unwrap();

        let stage_root = dir.path().join("stage/ws");
        std::fs::create_dir_all(&stage_root).unwrap();
        std::os::unix::fs::symlink(&node_modules, stage_root.join("node_modules")).unwrap();

        let policy = stage_escape_audit_policy(&root, &root, &stage_root)
            .expect("a reachable first-party link at a root-claimed workspace must arm the audit");
        assert_eq!(policy.stage_roots, vec![stage_root.to_path_buf()]);
        assert_eq!(policy.first_party_root, root);
    }
}

/// Issue #2090 (epic #2078, Wave 4, Sub 10c) — the epic's **sanctioned
/// loud-failure fallback** for #2048's silent islands/client shape, in place
/// of a full acceptance⇒enrolment coupling.
///
/// # What is silent today, and why the SSR fix cannot reach it
///
/// A first-party workspace sibling that is a *declared consume-from-source*
/// package (#2040's declared-entry exemption: its own `package.json` declares
/// an entry root pointing straight at un-built source) is **accepted** by the
/// stage-escape audit's case-2 rule, but nothing ever mirror-enrols it — so
/// its own `import.meta.glob(...)` is never expanded. On the SSR side that
/// throws at render time. Here it does not: the islands/client pipeline ships
/// the literal, unexpanded macro text into the browser bundle with a fully
/// GREEN build, and the throw only happens in the user's browser at hydration.
///
/// Worse, the shape that leaks is precisely the one with **no audit attached
/// at all**. `stage_escape_audit_policy` (guard (b)) is only ever consulted
/// `if let Some(islands_stage_root) = _islands_shadow…`, and the shadow itself
/// is only materialised when the scan already found a glob/raw/worker/plugin
/// preprocessing need. A sibling reached by bare package name through an edge
/// the islands scanner never records (a query-free `require(...)`, which
/// `collect_import_edges` does not visit) contributes no such need: no shadow,
/// no `--metafile`, no guard (a), no guard (b). Bolting a check onto the
/// existing audit call site would therefore never fire for it — hence this
/// pass, which is deliberately **not** gated on a stage existing.
///
/// # Why this is the fallback and not the coupling
///
/// Coupling acceptance to enrolment here would need esbuild to have run
/// first (acceptance is a property of its metafile), then a second bundling
/// pass over a shadow that also *redirects* the bare specifier away from the
/// wholesale live `node_modules` symlink the shadow sets up. That is a second
/// esbuild pass plus a curated `node_modules` layer in the latency-sensitive
/// dev rebundle path — new machinery of exactly the kind epic #2078's stop
/// condition reserves the loud-failure fallback for. esbuild stays the only
/// resolver: this pass predicts nothing.
///
/// # The evidence chain (no heuristics on the "is it broken" question)
///
/// 1. **Scope gate** — only a NESTED workspace member is in scope, matching
///    #2048's and #2083's own scoping (root-claimed topology is explicitly
///    out). Outside a workspace this returns before touching the bytes, so a
///    single-project build pays nothing.
/// 2. **Trigger** — the emitted, browser-bound artifacts are searched for a
///    Vite-only macro that must never ship. This is esbuild's own output, so a
///    hit is *proof* the macro leaked, not a prediction that it might. It also
///    subsumes "and the package was not otherwise mirror-enrolled": an
///    enrolled package's macro was expanded, so it cannot be here.
/// 3. **Attribution** — only once a leak is proven does this walk the
///    workspace's CLAIMED members, looking for a genuine `import.meta.glob`
///    CALL (via the AST-based `source_contains_import_meta_glob`, not a
///    substring) inside a location the package's own manifest DECLARES as an
///    entry root ([`zfb_build::declared_first_party_package_for_source`] —
///    Sub #2088's declared-data query, the same rule the audit accepts case-2
///    inputs by). Nothing is named on the strength of workspace membership
///    alone, and nothing is failed on inert text alone.
///
/// # Known blind spots (deliberately not widened here)
///
/// - **Attribution is candidate-level, not causal.** With no `--metafile` on
///   this path there is no record of which input esbuild actually pulled in,
///   so when several claimed members declare a macro-bearing entry this names
///   all of them. Both conditions must hold to fail at all (a macro really
///   leaked into the browser bytes AND a declared first-party entry really
///   contains one), so this cannot fail a build that ships nothing broken —
///   it can only be imprecise about which package to blame, and the
///   diagnostic says so.
/// - A leak attributable to no claimed member — e.g. a macro in a
///   project-local module the scanner never visited — leaves behaviour exactly
///   as it is today rather than failing an out-of-scope build.
/// - Only `import.meta.glob` is covered: it is the marker #2083 pins and the
///   one proven to survive into emitted bytes as literal text. `?raw` and the
///   module-worker `new URL(...)` macro are NOT covered by this fallback.
fn audit_unenrolled_first_party_macro_leak(
    project_root: &Path,
    emitted: &[&[u8]],
    artifact_label: &str,
) -> Result<()> {
    // (1) Scope gate — nested workspace members only.
    let first_party_root = zfb_types::first_party_root_for(project_root);
    let normalized_project_root = zfb_types::normalize_path_lexical(project_root);
    if first_party_root == normalized_project_root {
        return Ok(());
    }

    // (2) Trigger — did a Vite-only macro actually reach the browser bundle?
    if !emitted
        .iter()
        .any(|bytes| bytes_contain_import_meta_glob(bytes))
    {
        return Ok(());
    }

    // (3) Attribution — reached only when a leak is already proven, so this
    // walk never runs on a healthy build.
    //
    // The three filters below are ordered cheapest-first on purpose. The AST
    // check is what makes the whole pass decisive rather than textual: step
    // (2)'s byte scan cannot tell a real macro CALL from the same characters
    // sitting in a string or a comment, so on its own it could fail a build
    // over inert text. `source_contains_import_meta_glob` parses and looks for
    // a genuine call expression, and nothing is ever flagged without it
    // agreeing — the same detector `materialise_islands_shadow_with_worker_context`
    // already uses for its own stopgap offenders.
    let mut offenders: Vec<String> = Vec::new();
    for (name, package_root) in
        zfb_types::first_party::claimed_workspace_member_names(&first_party_root)
    {
        if package_root == normalized_project_root {
            continue; // the project's own package — its sources go through the ordinary pipeline.
        }
        for entry in walkdir::WalkDir::new(&package_root)
            .follow_links(false)
            .into_iter()
            // Only the two boundaries `claimed_workspace_member_names` itself
            // honours. Deliberately NOT `is_islands_shadow_pruned_dir`, which
            // also prunes top-level `dist`/`target` and hidden dirs: a package
            // may legitimately DECLARE an entry under one of those, and
            // skipping it would silently drop the attribution and let the leak
            // through unreported.
            .filter_entry(|e| {
                !e.file_type().is_dir()
                    || !matches!(
                        e.file_name().to_string_lossy().as_ref(),
                        "node_modules" | ".git"
                    )
            })
            .filter_map(|r| r.ok())
        {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_islands_shadow_js_like_file(path) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            if !source.contains("import.meta.glob") {
                continue; // cheap prefilter before paying for a parse.
            }
            if !matches!(
                zfb_build::glob_expand::source_contains_import_meta_glob(&source),
                Ok(true)
            ) {
                continue; // inert text (string/comment), or unparseable — fail open, not closed.
            }
            // Only a location the package ITSELF declares reachable can have
            // been the case-2 accepted input — an undeclared file behind a
            // built entry is a stage escape the audit rejects, not a leak this
            // fallback should name. Left last: it is the costliest check
            // (canonicalisation + manifest parse) and the AST filter above has
            // already reduced the candidates to ~nothing.
            let Some(package) =
                zfb_build::declared_first_party_package_for_source(path, &first_party_root)
            else {
                continue;
            };
            if package.name != name {
                continue;
            }
            offenders.push(format!("`{}` ({})", package.name, path.display()));
        }
    }

    if offenders.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "the emitted {artifact_label} contains a literal, unexpanded \
         `import.meta.glob(...)` call — a Vite-only macro that throws in the browser at \
         hydration, so this build must not ship. The unexpanded macro is declared by {} \
         — a first-party workspace package this project consumes FROM SOURCE (its own \
         package.json declares that location as an entry root). zfb accepts such a \
         package as first-party, but does not mirror its sources into the preprocessing \
         shadow, so its macros are never expanded. Reach the sibling through a tsconfig \
         alias or a relative import instead of its package name (those ARE mirrored and \
         expanded), replace the glob with explicit static imports, or move the usage to a \
         server-only (non-\"use client\") module. If more than one package is listed, the \
         leaked macro came from at least one of them — zfb runs no esbuild `--metafile` \
         on this bundling path, so it can name every candidate but not single one out. \
         Tracked at https://github.com/Takazudo/zudo-front-builder/issues/2048.",
        offenders.join(", ")
    ))
}

/// Cheap, esbuild-free boundary coverage for
/// [`audit_unenrolled_first_party_macro_leak`]. The end-to-end proof that the
/// fallback fires on the real NO-STAGE islands path is the env-gated
/// `bare_package_consume_from_source_sibling_glob_macro_reaches_islands_bundle_unexpanded`
/// (issue #2083, flipped by #2090); these run on EVERY gate instead, so the
/// bounded-set guarantee is never left resting on the esbuild lane alone.
#[cfg(test)]
mod unenrolled_macro_leak_tests {
    use super::*;
    use tempfile::tempdir;

    const LEAKED_BUNDLE: &[u8] = b"var m = import.meta.glob(\"./data/*.json\");\n";
    const CLEAN_BUNDLE: &[u8] = b"var m = { \"./data/entry.json\": () => x };\n";

    /// A nested-member workspace whose sibling `@acme/sib` is a declared
    /// consume-from-source package (`exports` points straight at un-built
    /// `.ts` source) carrying its own `import.meta.glob`. Returns the nested
    /// project root.
    fn write_nested_member_workspace(root: &Path) -> PathBuf {
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"workspace-root","private":true}"#,
        )
        .unwrap();

        let sibling = root.join("packages/sib");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(
            sibling.join("package.json"),
            r#"{"name":"@acme/sib","exports":{"./glob-source":"./index.ts"}}"#,
        )
        .unwrap();
        std::fs::write(
            sibling.join("index.ts"),
            "export const modules = import.meta.glob('./data/*.json');\n",
        )
        .unwrap();

        let project = root.join("apps/demo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("package.json"),
            r#"{"name":"demo","private":true}"#,
        )
        .unwrap();
        project
    }

    #[test]
    fn names_the_declared_consume_from_source_package_the_macro_leaked_from() {
        let tmp = tempdir().unwrap();
        let project = write_nested_member_workspace(tmp.path());

        let error =
            audit_unenrolled_first_party_macro_leak(&project, &[LEAKED_BUNDLE], "islands bundle")
                .expect_err("a leaked macro attributable to a declared sibling must fail loudly");
        let message = format!("{error:#}");
        assert!(message.contains("@acme/sib"), "{message}");
        assert!(message.contains("import.meta.glob("), "{message}");
        assert!(message.contains("islands bundle"), "{message}");
    }

    /// The bounded-set guarantee (epic #2078 Sub 10a's central boundary,
    /// enforced here at the consuming end): workspace MEMBERSHIP alone never
    /// fires this. The sibling declares a macro-bearing entry exactly as
    /// above, but nothing leaked into the emitted bytes, so nothing is
    /// flagged — no walk result can substitute for the evidence.
    #[test]
    fn stays_silent_when_no_macro_actually_reached_the_emitted_bundle() {
        let tmp = tempdir().unwrap();
        let project = write_nested_member_workspace(tmp.path());

        audit_unenrolled_first_party_macro_leak(&project, &[CLEAN_BUNDLE], "islands bundle")
            .expect("a clean bundle must not be failed just because a claimed member has a macro");
    }

    /// Scope gate: root-claimed / non-workspace topology is explicitly out of
    /// scope for #2048's fix, so a leak there leaves behaviour exactly as it
    /// was rather than failing an out-of-scope build.
    #[test]
    fn stays_silent_outside_a_workspace_even_when_a_macro_leaked() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("solo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("package.json"), r#"{"name":"solo"}"#).unwrap();

        audit_unenrolled_first_party_macro_leak(&project, &[LEAKED_BUNDLE], "islands bundle")
            .expect("a project outside a pnpm workspace is out of this fallback's scope");
    }

    /// Inert text is not a macro. A declared entry that merely MENTIONS
    /// `import.meta.glob(` in a string literal has no call for the shadow to
    /// expand, so it must never fail a build — the AST check
    /// (`source_contains_import_meta_glob`), not the byte scan, is what
    /// decides. Without it this pass would hard-fail on a package that does
    /// nothing wrong.
    #[test]
    fn does_not_fail_on_a_declared_entry_that_only_mentions_the_macro_in_a_string() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let project = write_nested_member_workspace(root);
        // Overwrite the sibling's entry so its only occurrence is inert text.
        std::fs::write(
            root.join("packages/sib/index.ts"),
            "export const docs = \"call import.meta.glob('./data/*.json') to load data\";\n",
        )
        .unwrap();

        audit_unenrolled_first_party_macro_leak(&project, &[LEAKED_BUNDLE], "islands bundle")
            .expect("a string literal is not a macro call and must not fail the build");
    }

    /// The diagnostic names only what it can attribute: a second claimed
    /// member with no macro at all is never named, and a macro sitting in an
    /// UNDECLARED location behind a dist-shipping package's declared entry is
    /// not attributable either — that shape is a stage escape the audit
    /// rejects, not an accepted case-2 input this fallback covers.
    #[test]
    fn names_neither_an_unrelated_claimed_member_nor_an_undeclared_source() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let project = write_nested_member_workspace(root);

        let unrelated = root.join("packages/unrelated");
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(
            unrelated.join("package.json"),
            r#"{"name":"@acme/unrelated","exports":{"./src":"./index.ts"}}"#,
        )
        .unwrap();
        std::fs::write(unrelated.join("index.ts"), "export const x = 1;\n").unwrap();

        let built = root.join("packages/built");
        std::fs::create_dir_all(built.join("src")).unwrap();
        std::fs::write(
            built.join("package.json"),
            r#"{"name":"@acme/built","exports":{".":"./dist/index.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            built.join("src/internal.ts"),
            "export const modules = import.meta.glob('./data/*.json');\n",
        )
        .unwrap();

        let error =
            audit_unenrolled_first_party_macro_leak(&project, &[LEAKED_BUNDLE], "islands bundle")
                .expect_err("the declared sibling still leaked, so this must still fail");
        let message = format!("{error:#}");
        assert!(message.contains("@acme/sib"), "{message}");
        assert!(
            !message.contains("@acme/unrelated"),
            "a claimed member carrying no macro must never be named: {message}"
        );
        assert!(
            !message.contains("@acme/built"),
            "a macro in an UNDECLARED location behind a built entry is not an accepted case-2 \
             input and must not be named: {message}"
        );
    }
}

#[allow(clippy::too_many_arguments)] // 10 params: #1497 added bundle mode/config + raw_invalidation; each param carries its own routing contract (see per-param comments), a struct would just shuffle the same fields
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
        // The islands scanner DFS-walks *imports*, so the seed set is
        // exactly the page sources esbuild can parse — the SCRIPT subset
        // (`zfb_types::SCRIPT_PAGE_EXTENSIONS`), not the routable one.
        // `.mdx`/`.md`/`.html` pages reach their islands through their own
        // pipelines and would not be valid entry points here.
        for ext in zfb_types::SCRIPT_PAGE_EXTENSIONS {
            let ext = *ext;
            for entry in walkdir::WalkDir::new(user_pages_dir)
                .into_iter()
                .filter_map(|r| r.ok())
            {
                if entry.file_type().is_file()
                    && entry.path().extension().and_then(|s| s.to_str()) == Some(ext)
                    // Conventional non-page sidecars (`*.d.ts`, `*.test.*`,
                    // `*.spec.*`) are not pages, so they must not seed the
                    // islands walk either — a test's unsupported import query
                    // would otherwise become a hard `ScanError` under the
                    // build policy for a file the router deliberately ignores.
                    && !zfb_types::is_page_sidecar_file(entry.path())
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
    let resolver = FsResolver::new()
        .with_project_root(project_root)
        .with_injected_route_roots(package_route_entrypoints);
    // Issue #2161: scope Guard (a)'s workspace-package edge detection (used
    // by `materialise_islands_shadow_with_worker_context` below, via
    // `scan_meta.workspace_package_edges_from_islands`) to the first-party
    // boundary — an npm-link/`file:` dependency pointing outside it is a
    // legitimate external dependency, not a workspace sibling (issue #1731),
    // and must never be recorded as a `WorkspacePackageImportEdge`.
    let first_party_root = zfb_types::first_party_root_for(project_root);
    let (islands_set, scan_meta) = match scan_islands_with_meta_and_first_party_root(
        &entries,
        &resolver,
        Some(&first_party_root),
    ) {
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
    let islands_jsx_import_source = match framework {
        crate::config::Framework::Preact => zfb_islands::FrameworkKind::Preact,
        crate::config::Framework::React => zfb_islands::FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_loaders = crate::config::resolve_bundle_loaders(bundle_config);
    let bundle_define = crate::config::resolve_bundle_define(bundle_config);
    let worker_build_context = module_worker_build_context(
        matches!(bundle_mode, zfb_islands::BundleMode::Production),
        framework,
        bundle_config,
        &plugin_config.alias_entries,
        &plugin_config.virtual_modules,
    );
    let plugin_preprocessing = discover_plugin_preprocessing(
        project_root,
        islands_set.iter().map(|island| island.source_path.clone()),
        &worker_build_context,
        true,
    )?;

    let mut _islands_shadow: Option<IslandsShadow> = None;
    let mut islands_preserve_symlinks = false;
    if !scan_meta.glob_reachable_from_islands.is_empty()
        || !scan_meta.raw_import_edges_from_islands.is_empty()
        || !scan_meta.module_worker_edges_from_islands.is_empty()
        || !plugin_preprocessing.raw_import_edges.is_empty()
        || !plugin_preprocessing.worker_edges.is_empty()
    {
        match materialise_islands_shadow_with_worker_context(
            project_root,
            &islands_set,
            &scan_meta,
            &worker_build_context,
            &plugin_preprocessing,
        )? {
            IslandsShadowOutcome::Ready(shadow) => {
                islands_preserve_symlinks = shadow.preserve_symlinks;
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

    // Replace the live islands invalidation closure only after the complete
    // scan + preprocessing pass succeeds. Keeping the previous successful
    // set across a transient scan/materialisation failure is intentional: a
    // deleted worker helper must remain watched so recreating it can recover
    // the dev bundle. The no-shadow fast path still carries terminal raw
    // targets (normally empty); a worker shadow contributes its complete
    // first-party graph in addition to raw targets.
    if let Some(invalidation) = raw_invalidation {
        let invalidation_first_party_root = zfb_types::first_party_root_for(project_root);
        let paths = IslandsShadowPaths::new(&invalidation_first_party_root);
        let mut dependencies: std::collections::BTreeSet<PathBuf> = scan_meta
            .raw_import_edges_from_islands
            .iter()
            .map(|edge| {
                paths
                    .logical_project_path(&edge.target)
                    .unwrap_or_else(|| edge.target.clone())
            })
            .collect();
        if let Some(shadow) = &_islands_shadow {
            dependencies.extend(shadow.raw_targets.iter().cloned());
            dependencies.extend(shadow.module_worker_dependencies.iter().cloned());
        }
        invalidation.replace_islands(dependencies);
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
    // warns, and only for the collisions the author can act on (see the
    // #2441 filter below).
    let island_manifest = zfb_islands::Manifest::from_islands(&islands_set);
    for collision in island_manifest.collisions() {
        // #2441: a package that ships both its compiled `dist/` output and
        // its sources can have the same component reach the scanner twice,
        // through two entry graphs. Those two participants are the same
        // component — hydration is correct whichever the manifest keeps —
        // and the remediation below is not actionable, because both live
        // inside a dependency. Drop them silently; every collision the
        // author CAN act on still warns.
        if zfb_islands::is_same_package_duplicate(collision) {
            continue;
        }
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
    let islands_tsconfig_boundary = _islands_shadow
        .as_ref()
        .map(|shadow| shadow._tempdir.path().to_path_buf());
    // cwd is the mirrored project dir (== shadow root outside a workspace);
    // the tsconfig search boundary stays the shadow root so workspace-level
    // configs mirrored above the project dir are still discoverable.
    let islands_bundler_working_dir = _islands_shadow
        .as_ref()
        .map(|shadow| shadow.bundle_working_dir.clone())
        .unwrap_or_else(|| project_root.to_path_buf());
    let mut esbuild_cfg =
        EsbuildSubprocessConfig::default().with_working_dir(islands_bundler_working_dir);
    // Issue #1707: arm the guard (b) stage-escape audit for the islands shadow
    // when the stage widened past project_root (a pnpm-workspace build). The
    // shadow root (`_tempdir`) is the boundary every staged input lives under;
    // esbuild's cwd (`bundle_working_dir`) is nested below it and is what #1705
    // audits metafile keys against.
    if let Some(islands_stage_root) = _islands_shadow
        .as_ref()
        .map(|shadow| shadow._tempdir.path())
    {
        if let Some(policy) = stage_escape_audit_policy(
            project_root,
            &zfb_types::first_party_root_for(project_root),
            islands_stage_root,
        ) {
            esbuild_cfg = esbuild_cfg.with_stage_audit(policy);
        }
    }
    if let Some(boundary) = islands_tsconfig_boundary {
        esbuild_cfg = esbuild_cfg.with_tsconfig_search_boundary(boundary);
    }
    if detect_project_node_modules(project_root).is_some()
        || detect_project_node_modules(&zfb_types::first_party_root_for(project_root)).is_some()
    {
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
    let islands_alias_entries = _islands_shadow
        .as_ref()
        .map(|shadow| {
            remap_project_plugin_aliases_to_shadow(
                &zfb_types::first_party_root_for(project_root),
                shadow._tempdir.path(),
                &plugin_config.alias_entries,
            )
        })
        .unwrap_or_else(|| plugin_config.alias_entries.clone());
    if !islands_alias_entries.is_empty() {
        esbuild_cfg = esbuild_cfg.with_alias_entries(islands_alias_entries);
    }
    // Issue #1701: the islands bundler is the THIRD parallel esbuild flow (with
    // the SSR bundler and the client-script preprocess); like the client sites
    // it must point a virtual module's absolute workspace-sibling import at the
    // sibling's staged copy in the islands shadow, not the live tree. Aliases
    // were already remapped above; the virtual-module sources were not.
    let islands_virtual_modules = _islands_shadow
        .as_ref()
        .map(|shadow| {
            remap_project_plugin_virtual_modules_to_shadow(
                project_root,
                shadow._tempdir.path(),
                &plugin_config.virtual_modules,
            )
        })
        .unwrap_or_else(|| plugin_config.virtual_modules.clone());
    if !islands_virtual_modules.is_empty() {
        esbuild_cfg = esbuild_cfg.with_virtual_modules(islands_virtual_modules);
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
    // Issue #1501: turn the scanner's direct + nested worker edges into one
    // deterministic entry per logical source. Worker code is read from the
    // preprocessing shadow (where `?raw` and nested worker URLs have already
    // been rewritten), while its filename is derived from the corresponding
    // original project path by the locked #1500 contract.
    let islands_first_party_root = zfb_types::first_party_root_for(project_root);
    let shadow_paths = IslandsShadowPaths::new(&islands_first_party_root);
    let module_worker_sources: std::collections::BTreeSet<PathBuf> = match &_islands_shadow {
        Some(shadow) => shadow.module_worker_sources.clone(),
        None => scan_meta
            .module_worker_edges_from_islands
            .iter()
            .map(|edge| edge.source_path.clone())
            .collect(),
    };
    let mut module_workers = Vec::with_capacity(module_worker_sources.len());
    for source in module_worker_sources {
        let logical_source = shadow_paths.logical_project_path(&source).ok_or_else(|| {
            anyhow!(
                "module-worker source {} has no logical path under {}",
                source.display(),
                project_root.display()
            )
        })?;
        let physical_source = match &_islands_shadow {
            Some(shadow) => shadow_paths
                .project_local_rel(&source)
                .map(|relative| shadow._tempdir.path().join(relative))
                .unwrap_or_else(|| source.clone()),
            None => source.clone(),
        };
        module_workers.push(zfb_islands::ModuleWorkerBundleEntry::new_scoped(
            project_root,
            &islands_first_party_root,
            &logical_source,
            physical_source,
        )?);
    }

    let bundle_cfg = match bundle_mode {
        zfb_islands::BundleMode::Production => BundleConfig::production(),
        zfb_islands::BundleMode::Development => BundleConfig::dev(),
    }
    .with_outdir(outdir.to_path_buf())
    .with_jsx_import_source(islands_jsx_import_source)
    .with_client_router(scan_meta.uses_client_router)
    .with_loaders(bundle_loaders)
    .with_define(bundle_define)
    .with_module_workers(module_workers)
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
            // Issue #2090 — the sanctioned loud-failure fallback, run against
            // every browser-bound artifact this bundle produced. Deliberately
            // OUTSIDE the `_islands_shadow` block above: the leak it catches
            // happens precisely when no shadow (and therefore no stage-escape
            // audit) exists at all.
            //
            // Dev takes the SAME `WarnAndSkip` branch the `KeepStopgap` arm
            // above already takes for this identical macro class, rather than
            // inventing a second dev semantics here. Note what that branch
            // really does: `rebundle_islands` reads `None` as "this project
            // has no islands bundle", so it clears the published URL and
            // prunes the previous generation's companions (`commands/dev.rs`)
            // — the tab loses islands until the offending file is fixed and
            // saved. That is pre-existing shared behaviour, not something this
            // fallback introduces, so the warning below says so plainly
            // instead of promising an untouched last-good bundle.
            let emitted: Vec<&[u8]> = std::iter::once(asset.bytes.as_slice())
                .chain(asset.chunks.iter().map(|chunk| chunk.bytes.as_slice()))
                .chain(asset.workers.iter().map(|worker| worker.bytes.as_slice()))
                .collect();
            if let Err(error) =
                audit_unenrolled_first_party_macro_leak(project_root, &emitted, "islands bundle")
            {
                match islands_glob_policy {
                    IslandsGlobPolicy::HardError => return Err(error),
                    IslandsGlobPolicy::WarnAndSkip => {
                        output::warn(format!(
                            "zfb islands: {error:#} The dev server stays up, but this rebundle \
                             emits no islands bundle at all — the page will serve without \
                             islands until the file(s) above are fixed and saved."
                        ));
                        return Ok((None, std::collections::BTreeSet::new()));
                    }
                }
            }
            Ok((
                Some(production_islands_asset_to_payload(asset)),
                registered_marker_names,
            ))
        }
        None => Ok((None, registered_marker_names)),
    }
}

/// Convert the islands crate's typed production output into the generic
/// writer payload without erasing its companion lifecycle. Chunks, workers,
/// and file-loader resources remain independently typed until this final
/// writer boundary, where all three must be copied verbatim beside the entry.
fn production_islands_asset_to_payload(
    asset: zfb_islands::ProductionIslandsAsset,
) -> AssetEmitterPayload {
    let companions = asset
        .chunks
        .into_iter()
        .chain(asset.workers)
        .map(|chunk| CompanionFile {
            filename: chunk.filename,
            bytes: chunk.bytes,
        })
        .chain(asset.resources.into_iter().map(|resource| CompanionFile {
            filename: resource.filename,
            bytes: resource.bytes,
        }))
        .collect();
    AssetEmitterPayload {
        bytes: asset.bytes,
        relative_path: asset.relative_path,
        stable_url: asset.stable_url,
        companions,
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
    /// Stage root and tsconfig search boundary. In a workspace-widened stage
    /// (issue #1674) this is the mirrored WORKSPACE first-party root, above the
    /// mirrored project dir; without a workspace marker it equals the mirrored
    /// project dir.
    root: PathBuf,
    /// esbuild's working directory inside the stage: the mirrored project dir.
    /// Equal to `root` except in a workspace-widened stage, where the project
    /// is mirrored at its workspace-relative location. Mirrors the islands
    /// shadow's `bundle_working_dir` (issue #1664).
    bundle_working_dir: PathBuf,
    entries: Vec<zfb_islands::client_scripts::ClientScriptEntry>,
    preserve_symlinks: bool,
    raw_targets: std::collections::BTreeSet<PathBuf>,
    worker_targets: std::collections::BTreeSet<PathBuf>,
    /// Workspace-sibling plain modules (neither a terminal `?raw` target nor a
    /// worker dependency) materialised into this stage. Issue #1710: without
    /// this set a sibling normal module is invisible to dev invalidation at
    /// both the watcher and the `mark_client_scripts()` gates, so editing it
    /// serves stale output until a restart.
    client_script_siblings: std::collections::BTreeSet<PathBuf>,
    workers_by_entry: std::collections::BTreeMap<String, Vec<ClientScriptWorkerEntry>>,
}

#[allow(clippy::too_many_arguments)] // 9 params: #1497 threaded the raw/worker expansion maps through staging; physical/logical identity and the copy-mode switches must stay explicit per call
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

#[cfg(test)]
fn stage_client_script_preprocessing(
    project_root: &Path,
    entries: &[zfb_islands::client_scripts::ClientScriptEntry],
) -> Result<Option<ClientScriptsPreprocessStage>> {
    stage_client_script_preprocessing_with_worker_context(
        project_root,
        entries,
        &zfb_build::ModuleWorkerBuildContext::default(),
    )
}

fn stage_client_script_preprocessing_with_worker_context(
    project_root: &Path,
    entries: &[zfb_islands::client_scripts::ClientScriptEntry],
    worker_build_context: &zfb_build::ModuleWorkerBuildContext,
) -> Result<Option<ClientScriptsPreprocessStage>> {
    if entries.is_empty() {
        return Ok(None);
    }
    let roots: Vec<PathBuf> = entries
        .iter()
        .map(|entry| entry.source_path.clone())
        .collect();
    let resolver = FsResolver::new().with_project_root(project_root);
    // Issue #2161: compute the first-party boundary up front so Guard (a)'s
    // workspace-package edge detection below is scoped to it from the
    // start — an npm-link/`file:` dependency pointing outside this boundary
    // is a legitimate external dependency, not a workspace sibling (issue
    // #1731), and must never be recorded as a `WorkspacePackageImportEdge`
    // here.
    let first_party_root = zfb_types::first_party_root_for(project_root);
    let graph = scan_reachable_modules_with_meta_and_first_party_root(
        &roots,
        &resolver,
        Some(&first_party_root),
    )
    .context("scan client-script graph for ?raw and module-worker preprocessing")?;
    let mut plugin_preprocessing = discover_plugin_preprocessing(
        project_root,
        std::iter::empty(),
        worker_build_context,
        true,
    )?;
    let mut plugin_preprocessing_by_entry = std::collections::BTreeMap::new();
    for entry in entries {
        let entry_preprocessing = discover_plugin_preprocessing(
            project_root,
            [entry.source_path.clone()],
            worker_build_context,
            false,
        )?;
        plugin_preprocessing.extend(entry_preprocessing.clone());
        plugin_preprocessing_by_entry.insert(entry.entry_name.clone(), entry_preprocessing);
    }
    if graph.raw_import_edges.is_empty()
        && graph.module_worker_edges.is_empty()
        && plugin_preprocessing.raw_import_edges.is_empty()
        && plugin_preprocessing.worker_edges.is_empty()
    {
        return Ok(None);
    }

    // Issue #1703, Stage Escape Guards — Guard (a): same escape as the
    // islands shadow above, on the client-script preprocessing path. `graph`
    // is already scoped to this closure's client-entry roots (it's the
    // reachable-modules scan seeded from `entries` alone), so a server-only
    // workspace-package import never appears here. This check only runs
    // once the gate above has already established `?raw`/module-worker
    // staging is active for this closure.
    if let Some(edge) = graph.workspace_package_edges.first() {
        return Err(anyhow!(
            "client-script module {} imports \"{}\" by its workspace-package name, but this \
             client-script graph requires `?raw`/module-worker preprocessing; a package-name \
             import resolves through the live node_modules symlink to the unprocessed source \
             and silently bypasses the staged rewrite — use a tsconfig alias or relative \
             import to reach a workspace sibling; package-name imports of first-party \
             siblings are not supported once staging is active",
            edge.importer.display(),
            edge.specifier
        ));
    }

    // Issue #1669/#1674: re-root the client-script preprocessing stage at the
    // workspace first-party boundary, mirroring the islands shadow
    // (`materialise_islands_shadow_with_worker_context`). Without a workspace
    // marker `first_party_root == project_root` (lexically) and every path
    // computation below collapses to the pre-#1674 single-package behavior, so
    // non-workspace projects stage byte-identically. `first_party_root` was
    // already computed above (issue #2161) to scope Guard (a)'s scan.
    let paths = IslandsShadowPaths::new(&first_party_root);
    // The #1500 worker-companion naming contract and the "which files does the
    // full-tree walk cover" question stay PROJECT-scoped: the walk mirrors the
    // project tree under the mirrored project dir, while sibling first-party
    // files are materialised selectively at their workspace-relative location.
    let project_paths = IslandsShadowPaths::new(project_root);
    let project_rel =
        match zfb_types::normalize_path_lexical(project_root).strip_prefix(&first_party_root) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
            _ => PathBuf::new(),
        };
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
            .chain(
                plugin_preprocessing_by_entry
                    .get(&entry.entry_name)
                    .into_iter()
                    .flat_map(|meta| meta.worker_edges.iter())
                    .map(|edge| edge.source_path.clone()),
            )
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
    let all_raw_edges: std::collections::BTreeSet<zfb_build::ModuleWorkerRawImportEdge> = graph
        .raw_import_edges
        .iter()
        .map(|edge| zfb_build::ModuleWorkerRawImportEdge {
            importer: edge.importer.clone(),
            target: edge.target.clone(),
        })
        .chain(plugin_preprocessing.raw_import_edges.iter().cloned())
        .collect();
    let raw_importers = dedup_shadow_paths(
        &paths,
        all_raw_edges.iter().map(|edge| edge.importer.clone()),
    );
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
    // Physical worker-dependency paths (as the rewrite reports them) so a
    // dependency that lives in a workspace sibling can be selectively staged
    // later; `worker_targets` only keeps the logical (watch) spelling.
    let mut worker_dependency_physicals = std::collections::BTreeSet::new();
    let worker_importers = dedup_shadow_paths(
        &paths,
        graph
            .module_worker_edges
            .iter()
            .map(|edge| edge.importer.clone())
            .chain(
                plugin_preprocessing
                    .worker_edges
                    .iter()
                    .map(|edge| edge.importer.clone()),
            ),
    );
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
        // The constructor-bearing importer is itself part of the invalidation
        // closure. If a transitive module removes its Worker edge, it must
        // still trigger one final client-script rebuild so the stable
        // companion is pruned and the live registry can be replaced.
        worker_targets.insert(logical_importer.clone());
        let rewrite = zfb_build::rewrite_module_worker_urls_with_context(
            &source,
            &logical_importer,
            project_root,
            worker_build_context,
        )
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
            worker_dependency_physicals.insert(dependency.dependency);
            worker_targets.insert(logical_dependency);
        }
        for config in rewrite.config_dependencies {
            let watched_config = paths
                .logical_project_path(&config.dependency)
                .unwrap_or(config.dependency);
            worker_targets.insert(watched_config);
        }
        worker_expanded_by_key.insert(key, rewrite.expanded_source);
    }
    for config in &plugin_preprocessing.config_dependencies {
        let watched_config = paths
            .logical_project_path(config)
            .unwrap_or_else(|| config.clone());
        worker_targets.insert(watched_config);
    }

    let mut raw_targets = std::collections::BTreeSet::new();
    for edge in &all_raw_edges {
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

    // Config discovery is per imported module in esbuild, not just per entry.
    // Seed every ordinary client module plus each first-party worker closure;
    // otherwise a nested config is merely copied verbatim by the broad stage
    // walk and an external relative extends edge breaks after relocation.
    let mut config_source_candidates: std::collections::BTreeSet<PathBuf> = entries
        .iter()
        .map(|entry| entry.source_path.clone())
        .chain(graph.modules.iter().cloned())
        .chain(plugin_preprocessing.files.iter().cloned())
        .chain(
            graph
                .module_worker_edges
                .iter()
                .flat_map(|edge| [edge.importer.clone(), edge.source_path.clone()]),
        )
        .chain(
            plugin_preprocessing
                .worker_edges
                .iter()
                .flat_map(|edge| [edge.importer.clone(), edge.source_path.clone()]),
        )
        .collect();
    let worker_config_roots: std::collections::BTreeSet<PathBuf> = graph
        .module_worker_edges
        .iter()
        .map(|edge| edge.source_path.clone())
        .chain(
            plugin_preprocessing
                .worker_edges
                .iter()
                .map(|edge| edge.source_path.clone()),
        )
        .collect();
    for worker_root in worker_config_roots {
        let worker_graph =
            scan_reachable_modules_with_meta(std::slice::from_ref(&worker_root), &resolver)
                .with_context(|| {
                    format!(
                        "scan client-script worker graph {} for staged config scopes",
                        worker_root.display()
                    )
                })?;
        config_source_candidates.extend(worker_graph.modules);
        config_source_candidates.extend(
            worker_graph
                .module_worker_edges
                .into_iter()
                .flat_map(|edge| [edge.importer, edge.source_path]),
        );
    }
    let client_config_sources: std::collections::BTreeSet<PathBuf> = config_source_candidates
        .iter()
        .filter_map(|source| paths.logical_project_path(source))
        .collect();
    let client_configs = collect_islands_shadow_configs(&first_party_root, &client_config_sources)?;

    // Issue #2163: compute the sibling closure now, ahead of the copy-mode
    // decision below — every input it needs (`config_source_candidates`,
    // `all_raw_edges`, `worker_dependency_physicals`) is already final at
    // this point, and the decision must see `sibling_present` before the
    // stage root is even allocated. The materialisation loop further below
    // reuses this same set instead of recomputing it.
    let mut sibling_closure: std::collections::BTreeSet<PathBuf> =
        std::collections::BTreeSet::new();
    let mut consider_sibling = |physical: &Path| {
        if paths.project_local_rel(physical).is_some()
            && project_paths.project_local_rel(physical).is_none()
        {
            sibling_closure.insert(physical.to_path_buf());
        }
    };
    for source in &config_source_candidates {
        consider_sibling(source);
    }
    for edge in &all_raw_edges {
        consider_sibling(&edge.importer);
        consider_sibling(&edge.target);
    }
    for dependency in &worker_dependency_physicals {
        consider_sibling(dependency);
    }
    let sibling_present = !sibling_closure.is_empty();

    let tempdir = tempfile::Builder::new()
        .prefix("zfb-client-preprocess-")
        .tempdir()
        .context("allocate client-script preprocessing directory")?;
    let root = tempdir.path().to_path_buf();
    // The mirrored PROJECT dir: the walked project tree stages here, and it is
    // esbuild's working directory. Equal to `root` without a workspace marker.
    let stage_project_dir = if project_rel.as_os_str().is_empty() {
        root.clone()
    } else {
        root.join(&project_rel)
    };
    // A workspace-widened stage needs node_modules at BOTH install roots: the
    // workspace-hoisted install at the stage root and the project's own nested
    // install at the mirrored project dir. Without a workspace the two collapse
    // to the single project-root install linked at the stage root.
    let first_party_node_modules = detect_project_node_modules(&first_party_root);
    let project_node_modules = if first_party_root.as_path() == project_root {
        None
    } else {
        detect_project_node_modules(project_root)
    };
    let has_node_modules = first_party_node_modules.is_some() || project_node_modules.is_some();
    // Issue #2163: a genuine workspace sibling anywhere in this closure forces
    // real-file materialisation on its own — nothing in the copy path
    // requires `node_modules` to exist, so keeping it as an outer conjunct
    // here would make the sibling case a fragile side effect of the
    // paths+node_modules trigger below rather than a first-class rule.
    let copy_mode = sibling_present
        || (has_node_modules && shadow_config_scope_uses_paths(&first_party_root, &client_configs));

    for entry in walkdir::WalkDir::new(project_root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            if is_islands_shadow_pruned_dir(project_root, entry) {
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
        // Stage the project tree under the mirrored project dir; a workspace
        // sibling reached by the graph is materialised separately below.
        let to = stage_project_dir.join(rel);
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
                .filter_entry(|nested| !is_islands_shadow_pruned_dir(&physical_root, nested))
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
            is_symlinked_file || is_typescript_project_config(from),
        )?;
    }

    // Selectively materialise the already-discovered first-party closure that
    // lives OUTSIDE the walked project tree (workspace siblings). The project
    // tree itself is covered by the walk above; `node_modules` is symlinked
    // whole. Each sibling is written at its workspace-relative location so
    // esbuild resolves the same tsconfig-alias / relative graph it does in the
    // live tree. `materialise_client_preprocess_stage_file` keys expansions on
    // `paths.path_key`, so a sibling `?raw`/worker importer is written with its
    // already-computed rewrite (and any generated raw modules) automatically.
    //
    // Dev-invalidation gap (issue #1683): sibling NORMAL modules staged here are
    // outside the project's client-script roots and absent from the returned
    // `raw_targets`/`worker_targets`, so editing one does not yet rebuild the
    // client script in `zfb dev`. Sibling `?raw` targets and worker deps do
    // invalidate.
    //
    // `sibling_closure` was already computed above, ahead of the copy-mode
    // decision (issue #2163) — reused here rather than recomputed.
    for physical in &sibling_closure {
        let rel = paths.project_local_rel(physical).ok_or_else(|| {
            anyhow!(
                "client-script first-party sibling {} has no logical path under {}",
                physical.display(),
                first_party_root.display()
            )
        })?;
        let to = root.join(&rel);
        materialise_client_preprocess_stage_file(
            physical,
            physical,
            &to,
            &root,
            &paths,
            &expanded_by_key,
            &worker_expanded_by_key,
            copy_mode,
            false,
        )?;
    }

    // Issue #1710: track every sibling in this closure (not just the `?raw`
    // targets / worker dependencies already captured above) for dev
    // invalidation. Converted to the logical (watched) spelling — the same
    // identity space `raw_targets` and `worker_targets` already use — via
    // `logical_project_path`, which is a no-op unless a symlink sits in the
    // physical ancestry.
    let client_script_siblings: std::collections::BTreeSet<PathBuf> = sibling_closure
        .iter()
        .map(|physical| {
            paths
                .logical_project_path(physical)
                .unwrap_or_else(|| physical.clone())
        })
        .collect();

    materialise_shadow_typescript_configs(&first_party_root, &root, &client_configs)?;

    // Workspace-hoisted install at the stage root (the tsconfig search
    // boundary); the project's own nested install at the mirrored project dir.
    // Without a workspace both collapse to the single root-level symlink,
    // byte-identical to the pre-#1674 behavior.
    //
    // Issue #1682 (shared with the islands shadow above): a sibling imported
    // through its pnpm PACKAGE NAME resolves through this live node_modules
    // link to the unprocessed source, bypassing the staged rewrite. Sibling
    // reach via tsconfig alias / relative path (what #1674 covers) resolves
    // to the staged files instead. Guarded by this epic (#1702): guard (a)
    // (issue #1703, checked earlier in this function against
    // `graph.workspace_package_edges`) pre-flight rejects the escape before
    // this symlink is even created; guard (b) (issue #1705/#1707) is the
    // esbuild-time backstop — a per-subprocess metafile audit that rejects
    // it even if a lower-level bundler invocation ever bypassed guard (a).
    if let Some(node_modules) = &first_party_node_modules {
        shadow_symlink(node_modules, &root.join("node_modules")).with_context(|| {
            format!(
                "symlink client preprocess stage node_modules {} -> {}",
                root.join("node_modules").display(),
                node_modules.display()
            )
        })?;
    }
    std::fs::create_dir_all(&stage_project_dir).with_context(|| {
        format!(
            "create client preprocess mirrored project dir {}",
            stage_project_dir.display()
        )
    })?;
    if let Some(node_modules) = &project_node_modules {
        let nested = stage_project_dir.join("node_modules");
        shadow_symlink(node_modules, &nested).with_context(|| {
            format!(
                "symlink client preprocess stage project node_modules {} -> {}",
                nested.display(),
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
            // The #1500 flat-naming contract now covers workspace siblings too
            // (issue #1677, using the `worker--ws-` encoding zfb-types added
            // for #1673): a project-local source keeps its byte-identical
            // unscoped name and stages under the mirrored project dir; a
            // workspace-sibling source — accepted by the widened #1674
            // first-party boundary, formerly rejected here by the #1667 guard
            // — mints a workspace-relative `-ws-` name and stages at its
            // workspace-relative slot instead (the same slot the sibling
            // closure above already materialised it at).
            let (logical_source, staged_source_path, filename) = if let Some(rel) =
                project_paths.project_local_rel(&source)
            {
                let logical_source = project_root.join(&rel);
                let filename = zfb_types::module_worker_filename(project_root, &logical_source)
                    .map_err(|error| {
                        anyhow!("client-script module-worker naming failed: {error}")
                    })?;
                (logical_source, stage_project_dir.join(&rel), filename)
            } else {
                let rel = paths.project_local_rel(&source).ok_or_else(|| {
                    anyhow!(
                        "client-script module-worker source {} is outside the mirrorable \
                             first-party project tree",
                        source.display()
                    )
                })?;
                let logical_source = first_party_root.join(&rel);
                let filename = zfb_types::module_worker_filename_scoped(
                    project_root,
                    &first_party_root,
                    &logical_source,
                )
                .map_err(|error| anyhow!("client-script module-worker naming failed: {error}"))?;
                (logical_source, root.join(&rel), filename)
            };
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
                source_path: staged_source_path,
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
        bundle_working_dir: stage_project_dir,
        entries: staged_entries,
        preserve_symlinks: !copy_mode,
        raw_targets,
        worker_targets,
        client_script_siblings,
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
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn build_default_client_scripts_payloads(
    project_root: &Path,
    outdir: &Path,
    framework: crate::config::Framework,
    registered: &zfb_build::ClientEntryList,
    bundle_config: Option<&crate::config::BundleConfig>,
) -> Result<Vec<AssetEmitterPayload>> {
    build_default_client_scripts_payloads_with_plugin_config(
        project_root,
        outdir,
        framework,
        registered,
        bundle_config,
        &IslandsPluginConfig::default(),
    )
}

pub(crate) fn build_default_client_scripts_payloads_with_plugin_config(
    project_root: &Path,
    outdir: &Path,
    framework: crate::config::Framework,
    registered: &zfb_build::ClientEntryList,
    bundle_config: Option<&crate::config::BundleConfig>,
    plugin_config: &IslandsPluginConfig,
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

    let client_scripts_jsx_import_source = match framework {
        crate::config::Framework::Preact => FrameworkKind::Preact,
        crate::config::Framework::React => FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_loaders = crate::config::resolve_bundle_loaders(bundle_config);
    let bundle_define = crate::config::resolve_bundle_define(bundle_config);
    let worker_build_context = zfb_build::ModuleWorkerBuildContext::new(
        true,
        &bundle_loaders,
        &bundle_define,
        client_scripts_jsx_import_source,
    )
    .with_plugins(
        plugin_config.alias_entries.clone(),
        plugin_config.virtual_modules.clone(),
    )
    .with_output_semantics(true, false);
    let preprocess_stage = stage_client_script_preprocessing_with_worker_context(
        project_root,
        &entries,
        &worker_build_context,
    )?;
    // esbuild's cwd is the mirrored project dir (== stage root outside a
    // workspace); the tsconfig search boundary stays the stage root so a
    // workspace-level config mirrored above the project dir is discoverable.
    let client_tsconfig_boundary = preprocess_stage.as_ref().map(|stage| stage.root.clone());
    let (bundle_entries, bundler_working_dir, preserve_symlinks) = match preprocess_stage.as_ref() {
        Some(stage) => (
            stage.entries.as_slice(),
            stage.bundle_working_dir.clone(),
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
    // Issue #1707: arm the guard (b) stage-escape audit for the client-script
    // stage when it widened past project_root (a pnpm-workspace build). Same
    // shape as the islands shadow — `stage.root` is the staged-input boundary;
    // esbuild's cwd (`bundle_working_dir`) nested below it is what #1705 audits
    // metafile keys against.
    if let Some(client_stage_root) = preprocess_stage.as_ref().map(|stage| stage.root.as_path()) {
        if let Some(policy) = stage_escape_audit_policy(
            project_root,
            &zfb_types::first_party_root_for(project_root),
            client_stage_root,
        ) {
            esbuild_cfg = esbuild_cfg.with_stage_audit(policy);
        }
    }
    let client_alias_entries = preprocess_stage
        .as_ref()
        .map(|stage| {
            remap_project_plugin_aliases_to_shadow(
                &zfb_types::first_party_root_for(project_root),
                &stage.root,
                &plugin_config.alias_entries,
            )
        })
        .unwrap_or_else(|| plugin_config.alias_entries.clone());
    if !client_alias_entries.is_empty() {
        esbuild_cfg = esbuild_cfg.with_alias_entries(client_alias_entries);
    }
    let client_virtual_modules = preprocess_stage
        .as_ref()
        .map(|stage| {
            remap_project_plugin_virtual_modules_to_shadow(
                project_root,
                &stage.root,
                &plugin_config.virtual_modules,
            )
        })
        .unwrap_or_else(|| plugin_config.virtual_modules.clone());
    if !client_virtual_modules.is_empty() {
        esbuild_cfg = esbuild_cfg.with_virtual_modules(client_virtual_modules);
    }
    if let Some(boundary) = client_tsconfig_boundary {
        esbuild_cfg = esbuild_cfg.with_tsconfig_search_boundary(boundary);
    }
    if detect_project_node_modules(project_root).is_some()
        || detect_project_node_modules(&zfb_types::first_party_root_for(project_root)).is_some()
    {
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
    let bundle_cfg = BundleConfig::production()
        .with_outdir(outdir.to_path_buf())
        .with_jsx_import_source(client_scripts_jsx_import_source)
        .with_loaders(bundle_loaders)
        .with_define(bundle_define)
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

    // Issue #2090 — the same sanctioned loud-failure fallback the islands path
    // above runs, applied to the client-script pipeline's own browser-bound
    // artifacts. This entry point has no `IslandsGlobPolicy` to consult, so a
    // leak is always a hard error here.
    {
        let emitted: Vec<&[u8]> = assets
            .iter()
            .flat_map(|asset| {
                std::iter::once(asset.bytes.as_slice()).chain(
                    asset
                        .companions
                        .iter()
                        .map(|companion| companion.bytes.as_slice()),
                )
            })
            .collect();
        audit_unenrolled_first_party_macro_leak(project_root, &emitted, "client-script bundle")?;
    }

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
/// `prev_output_filenames` is the set of flat entry/worker filenames declared
/// by the *previous* successful call. Those files are retained for this call
/// even when absent from the new output set, so HTML from the previous active
/// generation remains servable until the caller publishes replacement HTML.
/// Files retained from an older generation are pruned after all current
/// outputs have been written successfully. Pass an empty set on boot and
/// retain the returned (current-only) set for the next call.
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
#[derive(Debug, Default)]
struct PreparedDevClientScriptGeneration {
    /// Stable entry files, keyed by their flat public basename.
    entries: std::collections::BTreeMap<String, Vec<u8>>,
    /// Module-worker companions shared by the complete entry generation.
    companions: std::collections::BTreeMap<String, Vec<u8>>,
}

impl PreparedDevClientScriptGeneration {
    fn output_filenames(&self) -> std::collections::HashSet<String> {
        self.entries
            .keys()
            .chain(self.companions.keys())
            .cloned()
            .collect()
    }
}

/// Bundle and validate the complete client-script output namespace in memory.
///
/// In particular, this helper must finish every entry bundle before the caller
/// performs its first public write. A later entry failure can therefore never
/// leave an earlier stable entry from the failed generation on disk.
fn prepare_dev_client_script_generation<F>(
    entries: &[ClientScriptEntry],
    workers_by_entry: &std::collections::BTreeMap<String, Vec<ClientScriptWorkerEntry>>,
    mut bundle_entry: F,
) -> Result<PreparedDevClientScriptGeneration>
where
    F: FnMut(&ClientScriptEntry, &[ClientScriptWorkerEntry]) -> Result<ClientScriptBundleOutput>,
{
    // Know the entire stable-entry namespace before accepting any companion.
    // This catches a worker emitted by an early entry that collides with a
    // stable entry bundled later in the generation.
    let mut entry_filenames = std::collections::BTreeMap::new();
    for entry in entries {
        let filename = zfb_types::stable_client_script_filename(&entry.entry_name);
        if let Some(previous_source) =
            entry_filenames.insert(filename.clone(), entry.source_path.clone())
        {
            return Err(anyhow!(
                "client-scripts dev: stable entry filename collision for {filename:?}: {} vs {}",
                previous_source.display(),
                entry.source_path.display()
            ));
        }
    }

    let mut prepared = PreparedDevClientScriptGeneration::default();
    for entry in entries {
        let workers = workers_by_entry
            .get(&entry.entry_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let output = bundle_entry(entry, workers).with_context(|| {
            format!(
                "client-scripts dev: bundler failed for entry `{}` ({})",
                entry.entry_name,
                entry.source_path.display()
            )
        })?;
        let entry_filename = zfb_types::stable_client_script_filename(&entry.entry_name);

        // Reuse the final writer-side safety boundary shared by production
        // and the islands dev publisher. Byte payloads are irrelevant to the
        // filename check, so avoid cloning potentially large bundles here.
        let companion_names = output
            .companions
            .iter()
            .map(|companion| CompanionFile {
                filename: companion.filename.clone(),
                bytes: Vec::new(),
            })
            .collect::<Vec<_>>();
        validate_companion_file_set(&entry_filename, &companion_names).with_context(|| {
            format!(
                "client-scripts dev: invalid entry/companion namespace for `{}`",
                entry.entry_name
            )
        })?;

        for companion in output.companions {
            if let Some(entry_source) = entry_filenames.get(&companion.filename) {
                return Err(anyhow!(
                    "client-scripts dev: output filename collision for {:?}: stable entry {} vs module worker from {}",
                    companion.filename,
                    entry_source.display(),
                    entry.source_path.display()
                ));
            }
            if let Some(previous) = prepared.companions.get(&companion.filename) {
                if previous != &companion.bytes {
                    return Err(anyhow!(
                        "client-scripts dev: deterministic module-worker filename collision for {:?} produced different bytes",
                        companion.filename
                    ));
                }
                continue;
            }
            prepared
                .companions
                .insert(companion.filename, companion.bytes);
        }
        prepared
            .entries
            .insert(entry_filename, output.js.into_bytes());
    }

    Ok(prepared)
}

trait DevClientScriptAtomicWriter {
    fn atomic_write(&mut self, path: &Path, bytes: &[u8]) -> Result<()>;
}

struct FsDevClientScriptAtomicWriter;

impl DevClientScriptAtomicWriter for FsDevClientScriptAtomicWriter {
    fn atomic_write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        zfb_build::atomic_write(path, bytes)
    }
}

struct PlannedDevClientScriptWrite {
    path: PathBuf,
    bytes: Vec<u8>,
    previous_bytes: Option<Vec<u8>>,
}

/// Marker for the one client-publication error that must not restore a prior
/// ready state: the public write failed and its compensating rollback failed
/// too, so disk may contain a partial generation.
#[derive(Debug)]
pub(crate) struct DevClientScriptRollbackError {
    publication_path: PathBuf,
    publication_error: String,
    rollback_paths: Vec<PathBuf>,
    rollback_error: String,
}

impl DevClientScriptRollbackError {
    /// Flat output names whose compensating rollback failed. These exact
    /// files remain unsafe until a later complete client generation publishes
    /// them successfully.
    pub(crate) fn uncertain_output_filenames(&self) -> impl Iterator<Item = &str> {
        self.rollback_paths
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
    }
}

impl std::fmt::Display for DevClientScriptRollbackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "client-scripts dev: failed to publish {}: {}; rollback also failed: {}",
            self.publication_path.display(),
            self.publication_error,
            self.rollback_error
        )
    }
}

impl std::error::Error for DevClientScriptRollbackError {}

fn read_optional_dev_client_script_output(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| {
            format!(
                "client-scripts dev: failed to snapshot existing output {}",
                path.display()
            )
        }),
    }
}

fn rollback_dev_client_script_writes<W: DevClientScriptAtomicWriter>(
    writer: &mut W,
    applied: &[&PlannedDevClientScriptWrite],
) -> Vec<(PathBuf, String)> {
    let mut failures = Vec::new();
    for write in applied.iter().rev() {
        let result = match &write.previous_bytes {
            Some(previous_bytes) => writer.atomic_write(&write.path, previous_bytes),
            None => match std::fs::remove_file(&write.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            },
        };
        if let Err(error) = result {
            failures.push((write.path.clone(), format!("{error:#}")));
        }
    }
    failures
}

/// Atomically commit one fully prepared generation.
///
/// Every companion is published before every stable entry, so an entry never
/// points at a companion that has not yet been installed. If any atomic write
/// reports an error — including after replacing its destination — all paths
/// attempted by this commit are restored in reverse order. Existing files are
/// restored byte-for-byte and newly created candidates are removed.
fn commit_prepared_dev_client_script_generation<W: DevClientScriptAtomicWriter>(
    client_dir: &Path,
    prepared: PreparedDevClientScriptGeneration,
    writer: &mut W,
) -> Result<bool> {
    // Validate and snapshot the entire commit before its first write. The two
    // chained maps deliberately encode the publication barrier: companions
    // first, stable entries last.
    let mut planned = Vec::new();
    for (filename, bytes) in prepared.companions.into_iter().chain(prepared.entries) {
        let path = zfb_build::validate_output_path(client_dir, Path::new(&filename)).with_context(
            || format!("client-scripts dev: refused to write output filename {filename:?}"),
        )?;
        let previous_bytes = read_optional_dev_client_script_output(&path)?;
        if previous_bytes.as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        planned.push(PlannedDevClientScriptWrite {
            path,
            bytes,
            previous_bytes,
        });
    }

    let mut applied = Vec::new();
    for write in &planned {
        // Include the current path in rollback before attempting it: a writer
        // may replace the destination successfully and only then surface a
        // durability error.
        applied.push(write);
        if let Err(write_error) = writer.atomic_write(&write.path, &write.bytes) {
            let rollback_failures = rollback_dev_client_script_writes(writer, &applied);
            return if rollback_failures.is_empty() {
                Err(write_error).with_context(|| {
                    format!(
                        "client-scripts dev: failed to publish {}; restored previous generation",
                        write.path.display()
                    )
                })
            } else {
                let rollback_paths = rollback_failures
                    .iter()
                    .map(|(path, _)| path.clone())
                    .collect();
                let rollback_error = rollback_failures
                    .iter()
                    .map(|(path, error)| format!("{}: {error}", path.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(DevClientScriptRollbackError {
                    publication_path: write.path.clone(),
                    publication_error: format!("{write_error:#}"),
                    rollback_paths,
                    rollback_error,
                }
                .into())
            };
        }
    }

    Ok(!planned.is_empty())
}

fn bundle_and_commit_dev_client_script_generation<F, W>(
    client_dir: &Path,
    entries: &[ClientScriptEntry],
    workers_by_entry: &std::collections::BTreeMap<String, Vec<ClientScriptWorkerEntry>>,
    prev_output_filenames: &std::collections::HashSet<String>,
    writer: &mut W,
    bundle_entry: F,
) -> Result<(bool, std::collections::HashSet<String>)>
where
    F: FnMut(&ClientScriptEntry, &[ClientScriptWorkerEntry]) -> Result<ClientScriptBundleOutput>,
    W: DevClientScriptAtomicWriter,
{
    let prepared = prepare_dev_client_script_generation(entries, workers_by_entry, bundle_entry)?;
    let current_output_filenames = prepared.output_filenames();
    let mut changed = commit_prepared_dev_client_script_generation(client_dir, prepared, writer)?;
    changed |= prune_unretained_dev_client_script_outputs(
        client_dir,
        prev_output_filenames,
        &current_output_filenames,
    );
    Ok((changed, current_output_filenames))
}

fn prune_unretained_dev_client_script_outputs(
    client_dir: &Path,
    previous: &std::collections::HashSet<String>,
    current: &std::collections::HashSet<String>,
) -> bool {
    let mut changed = false;
    let entries = match std::fs::read_dir(client_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(error) => {
            output::warn(format!(
                "client-scripts dev: failed to inspect output directory {}: {error}",
                client_dir.display()
            ));
            return false;
        }
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let filename = entry.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        // This directory is framework-owned and dev client outputs are flat
        // JavaScript files. Leave directories and any unrelated file type
        // alone rather than broadening the prune boundary.
        if Path::new(filename).extension().and_then(|ext| ext.to_str()) != Some("js")
            || previous.contains(filename)
            || current.contains(filename)
        {
            continue;
        }
        let stale_path = entry.path();
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

/// Outcome of a dev client-scripts bundle pass.
pub(crate) struct DevClientScriptsOutcome {
    /// `true` when at least one file was written with new or changed bytes
    /// (or any stale file was pruned). The dev-server wires this to a
    /// `ReloadEvent::Page`.
    pub(crate) changed: bool,
    /// Entry and worker output basenames that were just written — pass as
    /// `prev_output_filenames` on the next call.
    pub(crate) output_filenames: std::collections::HashSet<String>,
    /// The logical original terminal-target set for dev invalidation; the
    /// shared registry retains lexical + canonical aliases.
    pub(crate) raw_targets: std::collections::BTreeSet<PathBuf>,
    /// The complete first-party worker dependency closure; edits to any
    /// member must rerun the client-script pipeline.
    pub(crate) worker_targets: std::collections::BTreeSet<PathBuf>,
    /// Workspace-sibling plain modules (neither a `?raw` target nor a worker
    /// dependency) materialised into the preprocess stage (issue #1710) — an
    /// edit to any member must also rerun the client-script pipeline.
    pub(crate) client_script_siblings: std::collections::BTreeSet<PathBuf>,
}

#[cfg(test)]
pub(crate) fn build_dev_client_scripts_to_disk(
    project_root: &Path,
    // Where dev client scripts are written + served from (issue #1189: the
    // isolated `.zfb-build/dev-assets` root, NOT the build-shared `dist/`).
    assets_root: &Path,
    framework: crate::config::Framework,
    bundle_config: Option<&crate::config::BundleConfig>,
    prev_output_filenames: &std::collections::HashSet<String>,
    registered: &zfb_build::ClientEntryList,
) -> Result<DevClientScriptsOutcome> {
    build_dev_client_scripts_to_disk_with_plugin_config(
        project_root,
        assets_root,
        framework,
        bundle_config,
        prev_output_filenames,
        registered,
        &IslandsPluginConfig::default(),
    )
}

pub(crate) fn build_dev_client_scripts_to_disk_with_plugin_config(
    project_root: &Path,
    assets_root: &Path,
    framework: crate::config::Framework,
    bundle_config: Option<&crate::config::BundleConfig>,
    prev_output_filenames: &std::collections::HashSet<String>,
    registered: &zfb_build::ClientEntryList,
    plugin_config: &IslandsPluginConfig,
) -> Result<DevClientScriptsOutcome> {
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

    let jsx_import_source = match framework {
        crate::config::Framework::Preact => FrameworkKind::Preact,
        crate::config::Framework::React => FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_loaders = crate::config::resolve_bundle_loaders(bundle_config);
    let bundle_define = crate::config::resolve_bundle_define(bundle_config);
    let worker_build_context = zfb_build::ModuleWorkerBuildContext::new(
        false,
        &bundle_loaders,
        &bundle_define,
        jsx_import_source,
    )
    .with_plugins(
        plugin_config.alias_entries.clone(),
        plugin_config.virtual_modules.clone(),
    )
    .with_output_semantics(false, true);
    let preprocess_stage = if entries.is_empty() {
        None
    } else {
        stage_client_script_preprocessing_with_worker_context(
            project_root,
            &entries,
            &worker_build_context,
        )?
    };
    if entries.is_empty() {
        // Keep the immediately previous active set for one generation. Any
        // older retained extras can be removed now because already-published
        // HTML can name only `prev_output_filenames`.
        let any_changed = prune_unretained_dev_client_script_outputs(
            &client_dir,
            prev_output_filenames,
            &std::collections::HashSet::new(),
        );
        return Ok(DevClientScriptsOutcome {
            changed: any_changed,
            output_filenames: std::collections::HashSet::new(),
            raw_targets: std::collections::BTreeSet::new(),
            worker_targets: std::collections::BTreeSet::new(),
            client_script_siblings: std::collections::BTreeSet::new(),
        });
    }

    let (
        bundle_entries,
        bundler_working_dir,
        preserve_symlinks,
        raw_targets,
        worker_targets,
        client_script_siblings,
    ) = match preprocess_stage.as_ref() {
        Some(stage) => (
            stage.entries.as_slice(),
            stage.bundle_working_dir.clone(),
            stage.preserve_symlinks,
            stage.raw_targets.clone(),
            stage.worker_targets.clone(),
            stage.client_script_siblings.clone(),
        ),
        None => (
            entries.as_slice(),
            project_root.to_path_buf(),
            false,
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        ),
    };
    // The tsconfig search boundary stays the stage root even though esbuild's
    // cwd is the mirrored project dir (issue #1674).
    let client_tsconfig_boundary = preprocess_stage.as_ref().map(|stage| stage.root.clone());

    // Set up the esbuild subprocess — same wiring as `build_default_client_scripts_payloads`
    // but using `BundleConfig::dev()` (no minification, sourcemaps on).
    let _embedded_esbuild_handle: Option<tempfile::TempDir>;
    let _embedded_nm_handle: Option<tempfile::TempDir>;
    let mut esbuild_cfg = EsbuildSubprocessConfig::default().with_working_dir(bundler_working_dir);
    // Issue #1707: arm the guard (b) stage-escape audit for the client-script
    // stage when it widened past project_root (a pnpm-workspace build). Same
    // shape as the islands shadow — `stage.root` is the staged-input boundary;
    // esbuild's cwd (`bundle_working_dir`) nested below it is what #1705 audits
    // metafile keys against.
    if let Some(client_stage_root) = preprocess_stage.as_ref().map(|stage| stage.root.as_path()) {
        if let Some(policy) = stage_escape_audit_policy(
            project_root,
            &zfb_types::first_party_root_for(project_root),
            client_stage_root,
        ) {
            esbuild_cfg = esbuild_cfg.with_stage_audit(policy);
        }
    }
    let client_alias_entries = preprocess_stage
        .as_ref()
        .map(|stage| {
            remap_project_plugin_aliases_to_shadow(
                &zfb_types::first_party_root_for(project_root),
                &stage.root,
                &plugin_config.alias_entries,
            )
        })
        .unwrap_or_else(|| plugin_config.alias_entries.clone());
    if !client_alias_entries.is_empty() {
        esbuild_cfg = esbuild_cfg.with_alias_entries(client_alias_entries);
    }
    let client_virtual_modules = preprocess_stage
        .as_ref()
        .map(|stage| {
            remap_project_plugin_virtual_modules_to_shadow(
                project_root,
                &stage.root,
                &plugin_config.virtual_modules,
            )
        })
        .unwrap_or_else(|| plugin_config.virtual_modules.clone());
    if !client_virtual_modules.is_empty() {
        esbuild_cfg = esbuild_cfg.with_virtual_modules(client_virtual_modules);
    }
    if let Some(boundary) = client_tsconfig_boundary {
        esbuild_cfg = esbuild_cfg.with_tsconfig_search_boundary(boundary);
    }
    if detect_project_node_modules(project_root).is_some()
        || detect_project_node_modules(&zfb_types::first_party_root_for(project_root)).is_some()
    {
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
    let bundle_cfg = BundleConfig::dev()
        .with_outdir(assets_root.to_path_buf())
        .with_jsx_import_source(jsx_import_source)
        .with_loaders(bundle_loaders)
        .with_define(bundle_define)
        .with_preserve_symlinks(preserve_symlinks);

    let empty_workers = std::collections::BTreeMap::new();
    let workers_by_entry = preprocess_stage
        .as_ref()
        .map(|stage| &stage.workers_by_entry)
        .unwrap_or(&empty_workers);
    let mut writer = FsDevClientScriptAtomicWriter;
    let (any_changed, current_output_filenames) = bundle_and_commit_dev_client_script_generation(
        &client_dir,
        bundle_entries,
        workers_by_entry,
        prev_output_filenames,
        &mut writer,
        |entry, workers| {
            bundler.bundle_client_script_file_with_workers(
                &entry.entry_name,
                &entry.source_path,
                workers,
                &bundle_cfg,
            )
        },
    )?;

    Ok(DevClientScriptsOutcome {
        changed: any_changed,
        output_filenames: current_output_filenames,
        raw_targets,
        worker_targets,
        client_script_siblings,
    })
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
    let (prerender_map, ssr_request_param_findings) =
        build_prerender_map(routes, project_root, |msg| output::warn(msg));
    // SSR route-contract guard (#2354): warn-only under `zfb build` — a
    // build on this broken shape must still SUCCEED (the epic's
    // compatibility guarantee); `zfb check` is the surface that fails on
    // it.
    for finding in &ssr_request_param_findings {
        output::warn(crate::render_pipeline::render_ssr_request_param_finding(
            finding,
        ));
    }

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
    //
    // Epic #2421 — this is the ONE place `emitRenderArtifacts` arms the
    // render-metadata channel (#2423). The snapshot is built with the
    // switch on, its per-entry `{ headings, sourceDigest }` is DRAINED
    // into the writer's index, and only then serialized: draining leaves
    // every entry's `render_metadata` back at `None`, which
    // `skip_serializing_if` elides, so the JSON embedded in the worker
    // bundle is byte-identical to the flag-off build. One collection
    // walk, no bundle-byte drift.
    let mut render_metadata = crate::commands::render_artifact::RenderMetadataIndex::default();
    let content_snapshot_json = {
        let mut snapshot = build_content_snapshot(
            project_root,
            config,
            zfb_content::SnapshotOptions {
                render_metadata: config.emit_render_artifacts,
            },
        );
        if config.emit_render_artifacts {
            if let Some(snap) = snapshot.as_mut() {
                render_metadata.drain_snapshot(snap);
            }
            extend_render_metadata_with_direct_pages(
                &mut render_metadata,
                project_root,
                config,
                routes,
            );
        }
        snapshot.as_ref().and_then(serialize_content_snapshot)
    };

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
    let phase_started = build_phase_start(runner.timing_enabled());
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
    emit_build_phase_timing("vendor-extraction-and-bundler-input", phase_started);

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

    // #2220 — strict-content-bridge. `bundler_out.content_bridge_fallback_pages`
    // is populated UNCONDITIONALLY by the bundler (it never itself decides to
    // fail the build — see `MaterialiseCtx::content_bridge_fallbacks`'s doc
    // comment in `crates/zfb-build/src/bundler.rs`); this is the ONLY place
    // that ever turns a non-empty list into a build failure, and it does so
    // before the expensive runtime `paths()` evaluation / V8 render below.
    // Flag off (the default): this block is inert — the fallback already
    // warned to stderr during bundling and the build proceeds to exit 0,
    // byte-identical to before this field existed.
    if config.strict_content_bridge && !bundler_out.content_bridge_fallback_pages.is_empty() {
        let mut msg = format!(
            "{} content-bridge fallback(s) found — the page(s) below render via \
             <pre data-zfb-content-fallback> because their compiled JSX does not parse:\n",
            bundler_out.content_bridge_fallback_pages.len()
        );
        for page in &bundler_out.content_bridge_fallback_pages {
            msg.push_str(&format!("  {page}\n"));
        }
        msg.push_str(
            "Fix the offending Markdown/MDX source, or set strictContentBridge: false \
             (or pass --no-strict-content-bridge) to allow the fallback.",
        );
        anyhow::bail!(msg.trim_end().to_string());
    }

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

    // Global collision check (issue #1768): now that the static and both
    // dynamic-expansion phases are folded into one universe, fail the build if
    // any two routes share a canonical `url_path` or an `output_path` — one
    // would otherwise silently clobber the other on disk.
    if let Err(msg) = crate::render_pipeline::validate_no_route_collisions(&static_routes) {
        anyhow::bail!(msg);
    }

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

    // 3.6b. Optional render-artifact export (epic #2421).
    //
    // Slice each page's content region out, strip ALL sentinels in place,
    // and write `dist/__zfb/render/<derived>.json`. The position is the
    // contract: both URL rewrites above have already run, so the captured
    // fragment carries the SAME asset URLs and base-prefixed links the
    // shipped page does, while minification and every later pass below
    // see marker-free HTML. Off by default and a single boolean test when
    // off — nothing is read or written.
    let render_artifact_routes: std::collections::BTreeMap<std::path::PathBuf, String> = {
        let mut routes = std::collections::BTreeMap::new();
        if config.emit_render_artifacts {
            for (url, rel) in &route_universe_for_rewrite {
                // First-wins on a shared output path, matching
                // `build_prod_rendered_files`' dedup — a plain
                // `collect()` would silently let the LAST route claim
                // the artifact's `route` field.
                routes
                    .entry(outdir.join(rel))
                    .or_insert_with(|| url.clone());
            }
        }
        routes
    };
    crate::commands::render_artifact::export_render_artifacts(
        config.emit_render_artifacts,
        outdir,
        &post_processable_pages,
        &render_artifact_routes,
        &render_metadata,
    )
    .context("render artifact export failed")?;

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

    // 3. Assemble the adapter input.
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
    let adapter_input = if !adapter.is_none() {
        let mut runtime_bundler_input = bundler_input_for_runtime;
        // Epic #2421: never arm the render-region markers in the
        // runtime/worker bundle. The sentinel strip pass (step 3.6b)
        // only rewrites SSG files on disk — a marker emitted by the
        // deployed worker at request time would ship to browsers with
        // nothing to strip it, breaking the "enabling the flag never
        // changes the shipped page" contract for SSR responses.
        runtime_bundler_input.emit_render_artifacts = false;
        runtime_bundler_input.worker_only_routes = Some(ssr_route_keys_for_runtime_bundle);
        runtime_bundler_input.bundle_basename = Some("bundle-runtime.mjs".to_string());
        let runtime_bundler_out = runner
            .bundle(runtime_bundler_input)
            .context("runtime-only bundler step (for deploy adapter) failed")?;

        Some(AdapterBundleInput {
            project_root: project_root.to_path_buf(),
            input_bundle: runtime_bundler_out.bundle_path.clone(),
            outdir: outdir.to_path_buf(),
            emitted_wasm_assets: runtime_bundler_out.emitted_wasm_assets,
        })
    } else {
        None
    };

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

    // `_redirects` (issue #1543 / epic #1541 Preview Parity) is CONFIG
    // for the Cloudflare Workers Static Assets redirect engine
    // (`zfb_server::redirects`), not a served asset — Cloudflare
    // requires it at the deploy root. Special-copy it there directly,
    // ignoring `base`/`copy_public_with_base` entirely: unlike the rest
    // of `public/`, it must never end up under a base-path segment.
    copy_redirects_file(project_root, outdir, &config.public_dir)
        .context("_redirects copy step failed")?;

    // 5. Adapter dispatch.
    //
    // Dispatch after public files land so adapters can preserve custom
    // ignore entries and reject any public file that would overwrite a
    // generated deploy module.
    if let Some(adapter_in) = adapter_input {
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

pub(crate) fn is_html_output_path(path: &Path) -> bool {
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
    let snapshot = build_content_snapshot(
        project_root,
        config,
        zfb_content::SnapshotOptions::default(),
    )?;
    serialize_content_snapshot(&snapshot)
}

/// Serialize a built snapshot for embedding in the worker bundle.
///
/// Split out of [`build_content_snapshot_json`] so `zfb build` can hold
/// on to the `ContentSnapshot` itself (it drains the render-metadata
/// channel out of it, epic #2421) and still produce the same JSON.
pub(crate) fn serialize_content_snapshot(
    snapshot: &zfb_content::ContentSnapshot,
) -> Option<String> {
    match serde_json::to_string(snapshot) {
        Ok(json) => Some(json),
        Err(e) => {
            output::warn(format!(
                "content snapshot serialization failed ({e}); getCollection(...) will see empty collections"
            ));
            None
        }
    }
}

/// Build the content snapshot itself, with the caller's opt-in switches
/// applied. See [`build_content_snapshot_json`] for the shared contract.
pub(crate) fn build_content_snapshot(
    project_root: &Path,
    config: &Config,
    options: zfb_content::SnapshotOptions,
) -> Option<zfb_content::ContentSnapshot> {
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
    match zfb_content::build_snapshot_with_options(&collections, &snapshot_config, options) {
        Ok(snap) => Some(snap),
        Err(e) => {
            output::warn(format!(
                "content snapshot build failed ({e}); getCollection(...) will see empty collections"
            ));
            None
        }
    }
}

/// Cover direct `pages/*.md` routes in the render-metadata index (epic
/// #2421). Direct `pages/*.mdx` routes are deliberately excluded — the
/// epic's documented exclusion: their compiled module IS the route
/// module, so no shell ever instruments them and their metadata could
/// never be joined.
///
/// These are markdown-backed routes that are NOT collection members, so
/// they never appear in a `ContentSnapshot` and the snapshot channel
/// cannot reach them; `zfb_content::render_region_metadata` is the
/// direct-page half of the same channel (#2423).
///
/// The per-entry pipeline setup mirrors the bundler's `.md` page pass
/// (`materialise_pages` in `crates/zfb-build/src/bundler.rs`) exactly:
/// same `PipelineSpec`, `reset_per_entry()` before each file, and the
/// resolve-links source file set when a source map is configured. That is
/// load-bearing, not hygiene — `resolveMarkdownLinks` rewrites link URLs
/// inside the compiled JSX, and the specifier's `#<hash8>` is a hash OF
/// that JSX. A pipeline shaped differently from the bundler's would mint
/// a different specifier and the writer's join would miss on every page.
/// Matching it also makes the compile a hit on the shared process-global
/// MDX cache rather than a second full compile.
///
/// Failures are per-page warnings: a page with no metadata simply gets no
/// artifact, which is strictly better than failing a build over an
/// opt-in emission.
fn extend_render_metadata_with_direct_pages(
    index: &mut crate::commands::render_artifact::RenderMetadataIndex,
    project_root: &Path,
    config: &Config,
    routes: &[zfb_router::Route],
) {
    let mut sources: Vec<&Path> = routes
        .iter()
        .filter(|r| !r.static_html)
        .map(|r| r.source_path.as_path())
        .filter(|p| {
            // `.md` only: a direct `pages/*.mdx` page's compiled module
            // IS the route module (no `render_md_page_shell` seam), so
            // it is never instrumented and its metadata could never be
            // joined — compiling it here would be a wasted full MDX
            // compile per page, and its hash-less specifier could
            // spuriously mark a colliding collection key ambiguous.
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        })
        .collect();
    sources.sort_unstable();
    sources.dedup();
    if sources.is_empty() {
        return;
    }

    let spec = {
        let mut spec =
            crate::commands::bundler_input::pipeline_spec_from_config(project_root, config);
        spec.resolve_source_map = build_resolve_source_map_for_snapshot(project_root, config);
        spec
    };
    let mut pipeline = match spec.build_pipeline() {
        Ok(p) => p,
        Err(e) => {
            output::warn(format!(
                "render artifacts: could not build the markdown pipeline for direct pages ({e}); \
                 those routes will get no artifact"
            ));
            return;
        }
    };
    let cache = zfb_content::mdx_jsx_emit::MdxModuleCache::process_global();

    for source in sources {
        let raw = match std::fs::read_to_string(source) {
            Ok(raw) => raw,
            Err(e) => {
                output::warn(format!(
                    "render artifacts: could not read {} ({e}); it will get no artifact",
                    source.display()
                ));
                continue;
            }
        };
        pipeline.reset_per_entry();
        if spec.resolve_source_map.is_some() {
            pipeline.set_resolve_links_source_file(source.to_path_buf());
        }
        match zfb_content::render_region_metadata(source, &raw, Some(&mut pipeline), Some(cache)) {
            Ok((specifier, metadata)) => index.insert(&specifier, metadata),
            Err(e) => output::warn(format!(
                "render artifacts: could not derive region metadata for {} ({e}); \
                 it will get no artifact",
                source.display()
            )),
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
        // `_redirects` is reserved config (Cloudflare Static Assets subset),
        // not a servable asset. `copy_redirects_file` special-copies the
        // top-level `public/_redirects` to the OUTPUT ROOT; skip it here so a
        // custom `base` does not ALSO relocate it under the base segment
        // (where it would become a served `/base/_redirects` asset). #1543.
        if rel == std::path::Path::new("_redirects") {
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

/// Special-copy `<project_root>/<public_dir>/_redirects` to
/// `<outdir>/_redirects` verbatim.
///
/// `_redirects` is CONFIG for the Cloudflare Workers Static Assets
/// redirect engine, not a served asset — Cloudflare requires it to
/// live at the deploy root regardless of any base-path mount. Unlike
/// [`copy_public_dir`], this function never relocates the file under a
/// base segment, even when `copy_public_with_base` is on: callers pass
/// `outdir` as-is (custom `outDir` aware, since `outdir` is already the
/// fully-resolved output directory) and never a `base`-prefixed
/// sub-path.
///
/// `public_dir` is honoured (custom `publicDir` aware) — the source is
/// always `<public_dir>/_redirects`, matching where `copy_public_dir`
/// looks for the rest of `public/`. A missing `_redirects` file is a
/// no-op (not every project uses the feature).
fn copy_redirects_file(
    project_root: &Path,
    outdir: &Path,
    public_dir: &std::path::Path,
) -> Result<()> {
    let public_root = if public_dir.is_absolute() {
        public_dir.to_path_buf()
    } else {
        project_root.join(public_dir)
    };
    let src = public_root.join("_redirects");
    if !src.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(outdir)
        .with_context(|| format!("_redirects copy: create outdir {}", outdir.display()))?;
    let dest = outdir.join("_redirects");
    std::fs::copy(&src, &dest).with_context(|| {
        format!(
            "_redirects copy: copy {} → {}",
            src.display(),
            dest.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Serialises the two tests that touch the process-wide
    /// `ZFB_TAILWIND_BIN` variable: one `set_var`s a deliberately bogus path
    /// to force a hermetic Tailwind failure, the other is env-gated ON that
    /// variable pointing at a real binary. `cargo test` runs tests on
    /// parallel threads in ONE process, so a scope guard bounds the
    /// mutation in time but not across threads — without this lock the
    /// first can flake the second (issue #1799 review finding).
    static TAILWIND_BIN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    use zfb_build::bundler::{BundleManifest, BundlerOutput, RouteEntry};
    use zfb_build::renderer::{HttpResponseLike, RendererOutput, SsrManifest};
    use zfb_router::{Route, RouteKind, Segment};

    #[test]
    fn production_islands_payload_keeps_resource_companions_verbatim() {
        let payload = production_islands_asset_to_payload(zfb_islands::ProductionIslandsAsset {
            bytes: b"import './islands-resource-zfb_md_wasm_glue-AAAA.mjs';".to_vec(),
            relative_path: PathBuf::from("assets/islands.js"),
            stable_url: "/assets/islands.js".to_string(),
            chunks: vec![zfb_islands::IslandsChunk {
                filename: "islands-chunk-BBBB.js".to_string(),
                bytes: b"chunk".to_vec(),
            }],
            workers: vec![zfb_islands::IslandsChunk {
                filename: "worker-src-s-search-d-worker-d-ts.js".to_string(),
                bytes: b"worker".to_vec(),
            }],
            resources: vec![
                zfb_islands::IslandsResource {
                    filename: "islands-resource-zfb_md_wasm_glue-AAAA.mjs".to_string(),
                    bytes: b"glue bytes".to_vec(),
                },
                zfb_islands::IslandsResource {
                    filename: "islands-resource-zfb_md_wasm_bg-CCCC.wasm".to_string(),
                    bytes: vec![0, 97, 115, 109],
                },
            ],
        });

        let companions = payload
            .companions
            .into_iter()
            .map(|companion| (companion.filename, companion.bytes))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(companions.len(), 4);
        assert_eq!(
            companions["islands-resource-zfb_md_wasm_glue-AAAA.mjs"],
            b"glue bytes"
        );
        assert_eq!(
            companions["islands-resource-zfb_md_wasm_bg-CCCC.wasm"],
            [0, 97, 115, 109]
        );
        assert_eq!(companions["islands-chunk-BBBB.js"], b"chunk");
        assert_eq!(
            companions["worker-src-s-search-d-worker-d-ts.js"],
            b"worker"
        );
    }

    /// Stub [`CssEngine`] that returns canned utility CSS and a canned
    /// package-`url()` companion set on demand — lets a test exercise
    /// `run_css_emitter`'s companion conversion without a real Tailwind
    /// subprocess (companions only ever come from
    /// `TailwindSubprocessEngine`'s real-binary path, never its mock path;
    /// see [`zfb_css::engine::TailwindSubprocessConfig::with_mock_output`]).
    struct CompanionStubCssEngine {
        css: String,
        companions: RefCell<Vec<zfb_css::url_attribution::PackageUrlAsset>>,
    }

    impl CssEngine for CompanionStubCssEngine {
        fn produce_utility_css(&self, _sources: &[PathBuf]) -> Result<String> {
            Ok(self.css.clone())
        }

        fn take_package_url_companions(&self) -> Vec<zfb_css::url_attribution::PackageUrlAsset> {
            std::mem::take(&mut *self.companions.borrow_mut())
        }
    }

    /// Companion boundary crossing for CSS (issue #2318 review finding):
    /// `run_css_emitter` — the real CLI-layer function every `zfb build`
    /// invocation calls — must thread `CssEmitterOutput::companions`
    /// (`zfb_css::url_attribution::PackageUrlAsset`) into
    /// `AssetEmitterPayload::companions` (`zfb_build::pipeline::CompanionFile`)
    /// unchanged. This is the CSS-side twin of
    /// `production_islands_payload_keeps_resource_companions_verbatim` above.
    ///
    /// `prod_asset_graph_e2e.rs` (in `zfb-build`, which cannot depend on the
    /// `zfb` bin crate) proves the real Tailwind binary produces companions
    /// and that the real `apply_prod_asset_pipeline` ships them correctly —
    /// but it reimplements this exact conversion by hand rather than calling
    /// `run_css_emitter` (a deliberate, documented choice in that file: no
    /// real Tailwind subprocess is needed to prove wiring, and this crate is
    /// the one place the real function lives). This test closes that gap
    /// cheaply — no real Tailwind, no `#[ignore]` — by driving
    /// `run_css_emitter` itself through the [`CompanionStubCssEngine`]: if a
    /// future edit ever drops or mangles the `.companions` mapping at
    /// `run_css_emitter`'s call site, this test fails without needing the
    /// tailwindcss-v4 binary staged.
    #[test]
    fn run_css_emitter_threads_package_url_companions_into_asset_payload() {
        let project_root = tempdir().unwrap();
        let outdir = tempdir().unwrap();
        let engine = CompanionStubCssEngine {
            css: ".icon{background:url(./icon-abc12345.svg)}".to_string(),
            companions: RefCell::new(vec![zfb_css::url_attribution::PackageUrlAsset {
                filename: "icon-abc12345.svg".to_string(),
                bytes: b"<svg>icon</svg>".to_vec(),
            }]),
        };

        let payload = run_css_emitter(
            engine,
            project_root.path(),
            outdir.path(),
            Vec::new(),
            Vec::new(),
            None,
        )
        .expect("run_css_emitter must succeed with a stub engine");

        // `combine()` (pipeline.rs) may append a trailing newline when
        // joining the (empty) framework/modules blocks — its own exact
        // join shape is covered by pipeline.rs's `combine_*` tests, so
        // this test only pins that the engine's CSS text survives intact,
        // not the trailing-byte shape of the join.
        assert!(
            String::from_utf8(payload.bytes.clone())
                .unwrap()
                .contains(".icon{background:url(./icon-abc12345.svg)}"),
            "CSS bytes must pass through from the engine unchanged; got: {:?}",
            payload.bytes,
        );
        assert_eq!(
            payload.companions.len(),
            1,
            "expected exactly the one companion the stub engine returned",
        );
        assert_eq!(payload.companions[0].filename, "icon-abc12345.svg");
        assert_eq!(payload.companions[0].bytes, b"<svg>icon</svg>");
    }

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
        /// Bundle-relative Wasm assets returned from each fake bundle pass.
        emitted_wasm_assets: RefCell<Vec<PathBuf>>,
        /// Content-bridge fallback pages (issue #2220) returned from each
        /// fake bundle pass. Default = empty (parity with `DefaultRunner`
        /// on a project with no fallback); tests can preload entries to
        /// exercise `run_build`'s `strictContentBridge` bail check.
        content_bridge_fallback_pages: RefCell<Vec<String>>,
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
                emitted_wasm_assets: RefCell::new(Vec::new()),
                content_bridge_fallback_pages: RefCell::new(Vec::new()),
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

        fn with_emitted_wasm_assets(self, assets: Vec<PathBuf>) -> Self {
            *self.emitted_wasm_assets.borrow_mut() = assets;
            self
        }

        /// Declare content-bridge fallback pages (issue #2220) the fake
        /// bundle pass should report, simulating a project where one or
        /// more `.md`/`.mdx` entries failed to bridge.
        fn with_content_bridge_fallback_pages(self, pages: Vec<String>) -> Self {
            *self.content_bridge_fallback_pages.borrow_mut() = pages;
            self
        }
    }

    impl BuildRunner for FakeRunner {
        fn bundle(&self, input: BundlerInput) -> Result<BundlerOutput> {
            self.bundle_calls.borrow_mut().push(input.clone());
            std::fs::create_dir_all(self.mock_bundle_path.parent().unwrap()).ok();
            std::fs::write(&self.mock_bundle_path, "// mock\n").ok();
            let emitted_wasm_assets = self.emitted_wasm_assets.borrow().clone();
            for asset in &emitted_wasm_assets {
                let asset_path = if asset.is_absolute() {
                    asset.clone()
                } else {
                    self.mock_bundle_path.parent().unwrap().join(asset)
                };
                if let Some(parent) = asset_path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(asset_path, b"\0asm").ok();
            }
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
                emitted_wasm_assets,
                content_bridge_fallback_pages: self.content_bridge_fallback_pages.borrow().clone(),
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

    /// Adapter seam used to pin ordering around the public-directory copy.
    struct PublicCopyObservingAdapterRunner {
        assets_ignore_before_dispatch: RefCell<Option<String>>,
    }
    impl PublicCopyObservingAdapterRunner {
        fn new() -> Self {
            Self {
                assets_ignore_before_dispatch: RefCell::new(None),
            }
        }
    }
    impl AdapterRunner for PublicCopyObservingAdapterRunner {
        fn run(&self, _package: &str, input: &AdapterBundleInput) -> Result<AdapterBundleOutput> {
            *self.assets_ignore_before_dispatch.borrow_mut() =
                std::fs::read_to_string(input.outdir.join(".assetsignore")).ok();
            Ok(AdapterBundleOutput {
                stdout: String::new(),
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
    fn resolve_strict_broken_links_defaults_false_when_cli_and_config_omit() {
        let cfg = Config::default();
        assert!(!resolve_strict_broken_links(
            BuildStrictBrokenLinks::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_broken_links_uses_config_when_cli_omits_true() {
        let cfg = Config {
            strict_broken_links: true,
            ..Config::default()
        };
        assert!(resolve_strict_broken_links(
            BuildStrictBrokenLinks::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_broken_links_uses_config_when_cli_omits_false() {
        let cfg = Config {
            strict_broken_links: false,
            ..Config::default()
        };
        assert!(!resolve_strict_broken_links(
            BuildStrictBrokenLinks::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_broken_links_cli_enable_beats_config_false() {
        let cfg = Config::default();
        assert!(resolve_strict_broken_links(
            BuildStrictBrokenLinks::Enabled,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_broken_links_cli_disable_beats_config_true() {
        let cfg = Config {
            strict_broken_links: true,
            ..Config::default()
        };
        assert!(!resolve_strict_broken_links(
            BuildStrictBrokenLinks::Disabled,
            &cfg
        ));
    }

    // --- resolve_strict_content_bridge tri-state cases (#2220) ---

    #[test]
    fn resolve_strict_content_bridge_defaults_false_when_cli_and_config_omit() {
        let cfg = Config::default();
        assert!(!resolve_strict_content_bridge(
            BuildStrictContentBridge::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_content_bridge_uses_config_when_cli_omits_true() {
        let cfg = Config {
            strict_content_bridge: true,
            ..Config::default()
        };
        assert!(resolve_strict_content_bridge(
            BuildStrictContentBridge::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_content_bridge_uses_config_when_cli_omits_false() {
        let cfg = Config {
            strict_content_bridge: false,
            ..Config::default()
        };
        assert!(!resolve_strict_content_bridge(
            BuildStrictContentBridge::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_content_bridge_cli_enable_beats_config_false() {
        let cfg = Config::default();
        assert!(resolve_strict_content_bridge(
            BuildStrictContentBridge::Enabled,
            &cfg
        ));
    }

    #[test]
    fn resolve_strict_content_bridge_cli_disable_beats_config_true() {
        let cfg = Config {
            strict_content_bridge: true,
            ..Config::default()
        };
        assert!(!resolve_strict_content_bridge(
            BuildStrictContentBridge::Disabled,
            &cfg
        ));
    }

    // --- resolve_emit_render_artifacts tri-state cases (epic #2421) ---

    #[test]
    fn resolve_emit_render_artifacts_defaults_false_when_cli_and_config_omit() {
        let cfg = Config::default();
        assert!(!resolve_emit_render_artifacts(
            BuildEmitRenderArtifacts::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_emit_render_artifacts_uses_config_when_cli_omits_true() {
        let cfg = Config {
            emit_render_artifacts: true,
            ..Config::default()
        };
        assert!(resolve_emit_render_artifacts(
            BuildEmitRenderArtifacts::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_emit_render_artifacts_uses_config_when_cli_omits_false() {
        let cfg = Config {
            emit_render_artifacts: false,
            ..Config::default()
        };
        assert!(!resolve_emit_render_artifacts(
            BuildEmitRenderArtifacts::Unspecified,
            &cfg
        ));
    }

    #[test]
    fn resolve_emit_render_artifacts_cli_enable_beats_config_false() {
        let cfg = Config::default();
        assert!(resolve_emit_render_artifacts(
            BuildEmitRenderArtifacts::Enabled,
            &cfg
        ));
    }

    #[test]
    fn resolve_emit_render_artifacts_cli_disable_beats_config_true() {
        let cfg = Config {
            emit_render_artifacts: true,
            ..Config::default()
        };
        assert!(!resolve_emit_render_artifacts(
            BuildEmitRenderArtifacts::Disabled,
            &cfg
        ));
    }

    // --- apply_strict_broken_links_override mutation-shape cases (#2117) ---

    #[test]
    fn apply_strict_broken_links_override_creates_markdown_when_absent() {
        let mut cfg = Config {
            markdown: None,
            ..Config::default()
        };
        apply_strict_broken_links_override(&mut cfg);
        assert_eq!(
            cfg.markdown
                .as_ref()
                .and_then(|m| m.features.as_ref())
                .and_then(|f| f.link_validation.as_ref())
                .and_then(|lv| lv.fail_on_broken),
            Some(true)
        );
    }

    #[test]
    fn apply_strict_broken_links_override_creates_features_when_absent() {
        let mut cfg = Config {
            markdown: Some(crate::config::MarkdownConfig {
                features: None,
                ..crate::config::MarkdownConfig::default()
            }),
            ..Config::default()
        };
        apply_strict_broken_links_override(&mut cfg);
        assert_eq!(
            cfg.markdown
                .as_ref()
                .and_then(|m| m.features.as_ref())
                .and_then(|f| f.link_validation.as_ref())
                .and_then(|lv| lv.fail_on_broken),
            Some(true)
        );
    }

    #[test]
    fn apply_strict_broken_links_override_creates_link_validation_when_absent() {
        let mut cfg = Config {
            markdown: Some(crate::config::MarkdownConfig {
                features: Some(crate::config::MarkdownFeaturesConfig {
                    link_validation: None,
                    ..crate::config::MarkdownFeaturesConfig::default()
                }),
                ..crate::config::MarkdownConfig::default()
            }),
            ..Config::default()
        };
        apply_strict_broken_links_override(&mut cfg);
        assert_eq!(
            cfg.markdown
                .as_ref()
                .and_then(|m| m.features.as_ref())
                .and_then(|f| f.link_validation.as_ref())
                .and_then(|lv| lv.fail_on_broken),
            Some(true)
        );
    }

    #[test]
    fn apply_strict_broken_links_override_flips_existing_fail_on_broken_false() {
        let mut cfg = Config {
            markdown: Some(crate::config::MarkdownConfig {
                features: Some(crate::config::MarkdownFeaturesConfig {
                    link_validation: Some(crate::config::LinkValidationConfig {
                        fail_on_broken: Some(false),
                    }),
                    ..crate::config::MarkdownFeaturesConfig::default()
                }),
                ..crate::config::MarkdownConfig::default()
            }),
            ..Config::default()
        };
        apply_strict_broken_links_override(&mut cfg);
        assert_eq!(
            cfg.markdown
                .as_ref()
                .and_then(|m| m.features.as_ref())
                .and_then(|f| f.link_validation.as_ref())
                .and_then(|lv| lv.fail_on_broken),
            Some(true)
        );
    }

    #[test]
    fn apply_strict_broken_links_override_preserves_sibling_markdown_and_features_fields() {
        let mut cfg = Config {
            markdown: Some(crate::config::MarkdownConfig {
                gfm: Some(crate::config::GfmFlag::All(true)),
                hard_breaks: Some(true),
                features: Some(crate::config::MarkdownFeaturesConfig {
                    reading_time: Some(crate::config::ReadingTimeFeature::Bool(true)),
                    link_validation: Some(crate::config::LinkValidationConfig {
                        fail_on_broken: None,
                    }),
                    ..crate::config::MarkdownFeaturesConfig::default()
                }),
                ..crate::config::MarkdownConfig::default()
            }),
            ..Config::default()
        };
        let before = cfg.markdown.clone().unwrap();

        apply_strict_broken_links_override(&mut cfg);

        let after = cfg.markdown.clone().unwrap();
        assert_eq!(after.gfm, before.gfm);
        assert_eq!(after.hard_breaks, before.hard_breaks);
        assert_eq!(
            after.features.as_ref().and_then(|f| f.reading_time.clone()),
            before
                .features
                .as_ref()
                .and_then(|f| f.reading_time.clone())
        );
        assert_eq!(
            after
                .features
                .as_ref()
                .and_then(|f| f.link_validation.as_ref())
                .and_then(|lv| lv.fail_on_broken),
            Some(true)
        );
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

    // --- strictContentBridge bail check in run_build (#2220) ---

    #[test]
    fn run_build_content_bridge_fallback_does_not_fail_when_strict_is_off() {
        // Flag off (the default): a reported fallback must NOT change the
        // outcome — the build still succeeds and the fallback stays a
        // stderr-only warning (already emitted by the bundler itself, not
        // asserted here).
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_content_bridge_fallback_pages(vec!["content/docs/broken.mdx".to_string()]);
        let cfg = Config {
            strict_content_bridge: false,
            ..Config::default()
        };
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
        .expect(
            "a content-bridge fallback must not fail the build when strictContentBridge is off",
        );

        assert_eq!(pages, 1);
    }

    #[test]
    fn run_build_content_bridge_fallback_fails_when_strict_is_on() {
        // Flag on: a reported fallback must fail the build (non-zero exit
        // once this bubbles up to `main`) and the error must name the
        // offending page.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_content_bridge_fallback_pages(vec!["content/docs/broken.mdx".to_string()]);
        let cfg = Config {
            strict_content_bridge: true,
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
        .expect_err("a content-bridge fallback must fail the build when strictContentBridge is on");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("content/docs/broken.mdx"),
            "error message must name the offending page; got: {msg}"
        );

        // The render step must never run — the bail happens right after
        // the bundle step, before the (expensive) render phase.
        assert!(
            runner.render_calls.borrow().is_empty(),
            "render must not run once the strict-content-bridge bail fires"
        );
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
                    emitted_wasm_assets: Vec::new(),
                    content_bridge_fallback_pages: Vec::new(),
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
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_emitted_wasm_assets(vec![PathBuf::from("answer-1234abcd.wasm")]);
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
        assert_eq!(
            calls[0].1.emitted_wasm_assets,
            vec![PathBuf::from("answer-1234abcd.wasm")],
            "the runtime bundle's bundle-relative Wasm assets must reach the adapter"
        );
    }

    /// SSR route-contract guard (#2354), the epic's compatibility
    /// guarantee: `zfb build` on the broken `(request: Request)` handler
    /// shape must still SUCCEED — the finding is a warning, never a build
    /// failure. A regression here would break existing projects on
    /// upgrade. Mirrors the fixture shape of
    /// `run_build_with_adapter_set_invokes_adapter_runner_after_render`
    /// above, swapping the correct zero-parameter handler for the
    /// mistaken `Request`-annotated one.
    #[test]
    fn run_build_succeeds_on_broken_ssr_request_param_shape() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        std::fs::write(
            project_root.join("pages/api/foo.tsx"),
            "export const frontmatter = { title: \"Foo\" };\nexport const prerender = false;\nexport default async function Handler(request: Request) {\n  return new Response('ok');\n}\n",
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
        let result = run_build(BuildArgsResolved {
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
        });
        assert!(
            result.is_ok(),
            "a build on the broken SSR request-param shape must still succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_build_dispatches_adapter_after_public_assetsignore_copy() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();
        std::fs::write(
            project_root.join("pages/api/foo.tsx"),
            "export const frontmatter = { title: \"Foo\" };\nexport const prerender = false;\nexport default function() { return null; }\n",
        )
        .unwrap();
        std::fs::write(
            project_root.join("public/.assetsignore"),
            "custom-public-entry\n",
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
        let adapter_runner = PublicCopyObservingAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &adapter_runner,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        assert_eq!(
            adapter_runner
                .assets_ignore_before_dispatch
                .borrow()
                .as_deref(),
            Some("custom-public-entry\n"),
            "adapter dispatch must see the public copy and merge its entries"
        );
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
                    companions: vec![
                        CompanionFile {
                            filename: "islands-resource-zfb_md_wasm_glue-AAAA.mjs".to_string(),
                            bytes: b"glue".to_vec(),
                        },
                        CompanionFile {
                            filename: "islands-resource-zfb_md_wasm_bg-BBBB.wasm".to_string(),
                            bytes: vec![0, 97, 115, 109],
                        },
                    ],
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
        // Companions remain file-relative to the hashed entry; base changes
        // only the entry's public URL and must never rewrite their emitted
        // names or bytes.
        assert_eq!(
            inputs.islands.as_ref().unwrap().companions[0].filename,
            "islands-resource-zfb_md_wasm_glue-AAAA.mjs"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().companions[0].bytes,
            b"glue"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().companions[1].filename,
            "islands-resource-zfb_md_wasm_bg-BBBB.wasm"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().companions[1].bytes,
            [0, 97, 115, 109]
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

    // -----------------------------------------------------------------------
    // zfb#1534 — `role_classes_inline_sources` (the `@source inline(...)`
    // safelist feeding `build_default_css_payload`'s `tw_cfg`)
    // -----------------------------------------------------------------------

    /// Build a `CodeHighlightConfig` in class mode with the given
    /// `roleClasses` map — the only shape `role_classes_inline_sources`
    /// reads from.
    fn code_highlight_with_role_classes(
        role_classes: std::collections::BTreeMap<String, String>,
    ) -> crate::config::CodeHighlightConfig {
        crate::config::CodeHighlightConfig {
            theme: None,
            themes_dir: None,
            theme_light: None,
            theme_dark: None,
            mode: crate::config::CodeHighlightMode::Class,
            class_prefix: crate::config::default_class_prefix(),
            role_classes: Some(role_classes),
            default_stylesheet: true,
        }
    }

    /// No `codeHighlight` configured at all => empty safelist (the
    /// common case — must not error, must not fabricate entries).
    #[test]
    fn role_classes_inline_sources_empty_when_no_code_highlight() {
        let cfg = Config::default();
        assert_eq!(role_classes_inline_sources(&cfg), Vec::<String>::new());
    }

    /// `codeHighlight` set but `roleClasses` absent => empty safelist.
    #[test]
    fn role_classes_inline_sources_empty_when_role_classes_absent() {
        let cfg = Config {
            code_highlight: Some(crate::config::CodeHighlightConfig {
                theme: None,
                themes_dir: None,
                theme_light: None,
                theme_dark: None,
                mode: crate::config::CodeHighlightMode::Class,
                class_prefix: crate::config::default_class_prefix(),
                role_classes: None,
                default_stylesheet: true,
            }),
            ..Config::default()
        };
        assert_eq!(role_classes_inline_sources(&cfg), Vec::<String>::new());
    }

    /// A multi-class value (`"text-violet-600 dark:text-violet-400"`) is
    /// split on whitespace into two independent inline sources — each
    /// token becomes its own `@source inline(...)` directive.
    #[test]
    fn role_classes_inline_sources_splits_multi_class_values() {
        let mut role_classes = std::collections::BTreeMap::new();
        role_classes.insert(
            "keyword".to_string(),
            "text-violet-600 dark:text-violet-400".to_string(),
        );
        let cfg = Config {
            code_highlight: Some(code_highlight_with_role_classes(role_classes)),
            ..Config::default()
        };
        assert_eq!(
            role_classes_inline_sources(&cfg),
            vec![
                "dark:text-violet-400".to_string(),
                "text-violet-600".to_string(),
            ]
        );
    }

    /// A class shared by two roles (e.g. `keyword` and `operator` both
    /// mapped to the same utility) is de-duplicated to a single
    /// `@source inline(...)` directive.
    #[test]
    fn role_classes_inline_sources_dedupes_shared_classes() {
        let mut role_classes = std::collections::BTreeMap::new();
        role_classes.insert("keyword".to_string(), "text-violet-600".to_string());
        role_classes.insert("operator".to_string(), "text-violet-600".to_string());
        let cfg = Config {
            code_highlight: Some(code_highlight_with_role_classes(role_classes)),
            ..Config::default()
        };
        assert_eq!(
            role_classes_inline_sources(&cfg),
            vec!["text-violet-600".to_string()]
        );
    }

    /// Output is always sorted regardless of the `BTreeMap` role-key
    /// insertion order or the class token order within a value —
    /// determinism, since this feeds the synthesised entry CSS and
    /// hence the CSS asset hash (a config that hasn't changed must not
    /// churn the hash across builds).
    #[test]
    fn role_classes_inline_sources_is_sorted_and_deterministic() {
        let mut role_classes = std::collections::BTreeMap::new();
        role_classes.insert("keyword".to_string(), "text-violet-600".to_string());
        role_classes.insert("comment".to_string(), "text-zinc-500".to_string());
        role_classes.insert("string".to_string(), "text-green-600".to_string());
        let cfg = Config {
            code_highlight: Some(code_highlight_with_role_classes(role_classes)),
            ..Config::default()
        };
        let result = role_classes_inline_sources(&cfg);
        let mut sorted = result.clone();
        sorted.sort();
        assert_eq!(result, sorted, "output must already be sorted");
        assert_eq!(
            result,
            vec![
                "text-green-600".to_string(),
                "text-violet-600".to_string(),
                "text-zinc-500".to_string(),
            ]
        );
    }

    /// Same `Config` => byte-identical (`Vec`-identical) output across
    /// repeated calls — the determinism guarantee `role_classes_inline_sources`
    /// exists to provide.
    #[test]
    fn role_classes_inline_sources_is_stable_across_calls() {
        let mut role_classes = std::collections::BTreeMap::new();
        role_classes.insert(
            "keyword".to_string(),
            "text-violet-600 dark:text-violet-400".to_string(),
        );
        role_classes.insert("string".to_string(), "text-green-600".to_string());
        let cfg = Config {
            code_highlight: Some(code_highlight_with_role_classes(role_classes)),
            ..Config::default()
        };
        assert_eq!(
            role_classes_inline_sources(&cfg),
            role_classes_inline_sources(&cfg)
        );
    }

    /// Issue #1776: `assemble_css_content_globs` folds package-route
    /// entrypoint parent dirs AND Sibling Mirror mirror roots into the
    /// content-glob list, in that order, and de-dupes against the
    /// defaults (and against each other). Pure logic — no project tree,
    /// no Tailwind binary.
    ///
    /// Issue #1803: also asserts the companion `@source not` exclusion
    /// globs — one per `CSS_SIBLING_MIRROR_SKIP_DIRS` entry for the
    /// freshly appended mirror root (`/workspace/lib/shared`) only. The
    /// mirror root that dedupes away as a duplicate of a default
    /// (`/proj/pages`) never reaches the fresh-append branch, so it
    /// contributes none.
    #[test]
    fn assemble_css_content_globs_appends_mirror_roots_deduped_after_defaults() {
        let defaults = vec!["/proj/pages".to_string(), "/proj/components".to_string()];
        let package_route_entrypoints = vec![PathBuf::from("/proj/.zfb-package-routes/blog/page")];
        let sibling_mirror_roots = vec![
            // Duplicate of a default root — must not be re-added.
            PathBuf::from("/proj/pages"),
            PathBuf::from("/workspace/lib/shared"),
        ];

        let (globs, negative_globs) = assemble_css_content_globs(
            &defaults,
            &package_route_entrypoints,
            &sibling_mirror_roots,
        );

        assert_eq!(
            globs,
            vec![
                "/proj/pages".to_string(),
                "/proj/components".to_string(),
                "/proj/.zfb-package-routes/blog".to_string(),
                "/workspace/lib/shared".to_string(),
            ],
            "expected defaults, then the package-route entrypoint's parent dir, \
             then the sibling mirror root (dup of a default dropped): {globs:?}"
        );
        assert_eq!(
            negative_globs,
            CSS_SIBLING_MIRROR_SKIP_DIRS
                .iter()
                .map(|skip_dir| format!("/workspace/lib/shared/**/{skip_dir}/**"))
                .collect::<Vec<_>>(),
            "expected one @source not exclusion glob per CSS_SIBLING_MIRROR_SKIP_DIRS \
             entry for the freshly appended mirror root only: {negative_globs:?}"
        );
    }

    /// A mirror root that happens to coincide with a package-route
    /// entrypoint's parent dir is de-duped too — the `seen` set is shared
    /// across both extension passes, not reset between them.
    ///
    /// Issue #1803: since this mirror root dedupes away (it was already
    /// seen via the package-route entrypoint's parent dir), it never
    /// reaches the fresh-append branch either, so it carries no skip-dir
    /// exclusions — "scope: mirror roots only" leaves package-route dirs
    /// (and anything deduped against one) on their pre-#1803 behavior.
    #[test]
    fn assemble_css_content_globs_dedupes_mirror_root_against_package_route_dir() {
        let defaults = vec!["/proj/pages".to_string()];
        let package_route_entrypoints = vec![PathBuf::from("/workspace/lib/shared/page.tsx")];
        let sibling_mirror_roots = vec![PathBuf::from("/workspace/lib/shared")];

        let (globs, negative_globs) = assemble_css_content_globs(
            &defaults,
            &package_route_entrypoints,
            &sibling_mirror_roots,
        );

        assert_eq!(
            globs,
            vec![
                "/proj/pages".to_string(),
                "/workspace/lib/shared".to_string(),
            ],
            "the mirror root duplicating the package-route parent dir must \
             appear exactly once: {globs:?}"
        );
        assert!(
            negative_globs.is_empty(),
            "a mirror root deduped against a package-route dir must not gain \
             skip-dir exclusions: {negative_globs:?}"
        );
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
        let payload = build_default_css_payload(
            project_root,
            &project_root.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
        )
        .expect("should not error")
        .expect("expected Some payload: authored CSS + module must ship even with tailwind off");

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
        let maps =
            compute_css_module_class_maps(project_root, &[], &std::collections::BTreeSet::new())
                .expect("class maps");
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

    /// Locked authored-import contract (#2721): the real Tailwind-disabled
    /// build seam bundles local imports and leaves external imports for the
    /// pipeline's final hoist.
    #[test]
    fn css_payload_bundles_authored_imports_when_tailwind_disabled() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        std::fs::write(
            project_root.join("styles/global.css"),
            concat!(
                "@import \"./vendor.css\";\n",
                ".authored { color: rebeccapurple; }\n",
                "@import url(\"https://fonts.googleapis.com/css2?family=Noto+Sans+JP\");\n",
            ),
        )
        .unwrap();
        std::fs::write(
            project_root.join("styles/vendor.css"),
            ".vendor { display: grid; }\n",
        )
        .unwrap();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload = build_default_css_payload(
            project_root,
            &project_root.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
        )
        .expect("should not error")
        .expect("authored CSS must ship");
        let css = String::from_utf8(payload.bytes).unwrap();

        assert!(
            css.contains(".vendor"),
            "vendor rules must be inlined:\n{css}"
        );
        assert!(
            !css.contains("./vendor.css"),
            "resolvable local import must be absent:\n{css}"
        );
        let font_import = css
            .find("https://fonts.googleapis.com/css2?family=Noto+Sans+JP")
            .expect("external font import preserved");
        let first_rule = css.find(".vendor").expect("vendor rule emitted");
        assert!(
            font_import < first_rule,
            "external font import must be hoisted before rules:\n{css}"
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
        let payload = build_default_css_payload(
            project_root,
            &project_root.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
        )
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
        let payload = build_default_css_payload(
            project_root,
            &project_root.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
        )
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

    /// Highlight Tokens epic sub #1533: builds a `codeHighlight` config for
    /// the test matrix below without depending on `CodeHighlightConfig`
    /// implementing `Default` (it deliberately doesn't — every field is
    /// meaningful to the highlighting pipeline, see config.rs). `class_prefix`
    /// is parameterized so the classPrefix-rewrite test can pass a
    /// non-default prefix; pass `"hi-"` for the default-prefix cases.
    fn code_highlight_config(
        mode: CodeHighlightMode,
        default_stylesheet: bool,
        class_prefix: &str,
    ) -> crate::config::CodeHighlightConfig {
        crate::config::CodeHighlightConfig {
            theme: None,
            themes_dir: None,
            theme_light: None,
            theme_dark: None,
            mode,
            class_prefix: class_prefix.to_string(),
            role_classes: None,
            default_stylesheet,
        }
    }

    /// Highlight Tokens epic sub #1533: class mode + `defaultStylesheet`
    /// true (the default) ships `zfb_css::default_hi_css()` ahead of the
    /// authored CSS, even on the Tailwind-disabled (`build_authored_only_
    /// css_payload`) path — proving the wiring shared with the Tailwind
    /// path via `run_css_emitter`. Hermetic: runs through
    /// `AuthoredCssEngine`, no tailwind binary required.
    #[test]
    fn css_payload_ships_default_hi_stylesheet_in_class_mode() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            code_highlight: Some(code_highlight_config(CodeHighlightMode::Class, true, "hi-")),
            ..Config::default()
        };
        let payload = build_default_css_payload(project_root, &project_root.join("dist"), &cfg, &[], &[], &[])
            .expect("should not error")
            .expect(
                "expected Some payload: default_hi_css() is non-empty so class mode always ships a payload",
            );

        let css = String::from_utf8(payload.bytes).unwrap();
        assert!(
            css.contains("--zfb-hi-kw"),
            "class-mode default stylesheet must ship the --zfb-hi-kw token; got:\n{css}",
        );
        assert!(
            css.contains(".hi-kw"),
            "class-mode default stylesheet must ship the .hi-kw role class; got:\n{css}",
        );
    }

    /// `codeHighlight.defaultStylesheet: false` opts out — the framework
    /// block must be absent even in class mode.
    #[test]
    fn css_payload_omits_default_hi_stylesheet_when_opted_out() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        // An authored global keeps the payload `Some(..)` so the negative
        // assertion below is meaningful (an entirely empty payload would
        // trivially "not contain" the marker for the wrong reason).
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        std::fs::write(
            project_root.join("styles/global.css"),
            ".authored-global { color: rebeccapurple; }\n",
        )
        .unwrap();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            code_highlight: Some(code_highlight_config(
                CodeHighlightMode::Class,
                false,
                "hi-",
            )),
            ..Config::default()
        };
        let payload = build_default_css_payload(
            project_root,
            &project_root.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
        )
        .expect("should not error")
        .expect("authored global CSS keeps the payload non-empty");

        let css = String::from_utf8(payload.bytes).unwrap();
        assert!(
            !css.contains("--zfb-hi-kw"),
            "defaultStylesheet:false must omit the framework block; got:\n{css}",
        );
    }

    /// Inline mode (the default `codeHighlight.mode`) never ships the
    /// class-mode token stylesheet, even though `defaultStylesheet`
    /// defaults to `true` — the field is documented as "only meaningful
    /// in class mode" (config.rs).
    #[test]
    fn css_payload_omits_default_hi_stylesheet_in_inline_mode() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        std::fs::write(
            project_root.join("styles/global.css"),
            ".authored-global { color: rebeccapurple; }\n",
        )
        .unwrap();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            // code_highlight: None => mode defaults to Inline.
            ..Config::default()
        };
        let payload = build_default_css_payload(
            project_root,
            &project_root.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
        )
        .expect("should not error")
        .expect("authored global CSS keeps the payload non-empty");

        let css = String::from_utf8(payload.bytes).unwrap();
        assert!(
            !css.contains("--zfb-hi-kw"),
            "inline mode (default) must never ship the class-mode token stylesheet; got:\n{css}",
        );
    }

    // ── Sibling Mirror (issue #1691/#1696): sibling `.module.css` class
    // maps + CSS emission through the claim plan ─────────────────────────

    /// Workspace + claimed-sibling-alias fixture shared by the sibling CSS
    /// Modules tests below. Layout:
    ///
    /// ```text
    /// <ws>/pnpm-workspace.yaml              packages: ['.', 'sub-packages/*']
    /// <ws>/sub-packages/host/                the project (host)
    /// <ws>/sub-packages/host/tsconfig.json   "@shared/*" -> "../../lib/shared/*"
    /// <ws>/lib/shared/Button.tsx             relatively imports ./Button.module.css
    /// <ws>/lib/shared/Button.module.css      the CLAIMED sibling module
    /// ```
    ///
    /// The wildcard tsconfig alias is claim source (b) of
    /// `zfb_build::SiblingMirrorPlan` — no `package.json` sits between
    /// `lib/shared` and the workspace root, so the mirror root resolves to
    /// the claim's own directory (`resolve_mirror_root`'s bare-dir branch).
    /// Returns `(TempDir, project_root)`; the guard must stay alive for the
    /// fixture files to keep existing.
    fn sibling_css_workspace_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().unwrap();
        let ws = tmp.path();
        std::fs::write(
            ws.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();

        let project = ws.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@shared/*":["../../lib/shared/*"]}}}"#,
        )
        .unwrap();

        std::fs::create_dir_all(ws.join("lib/shared")).unwrap();
        std::fs::write(
            ws.join("lib/shared/Button.tsx"),
            "import styles from \"./Button.module.css\";\n\
             export default function Button() { return styles.root; }\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("lib/shared/Button.module.css"),
            ".root { color: blue; }\n",
        )
        .unwrap();

        (tmp, project)
    }

    /// Acceptance: a sibling `.module.css` reached through a claimed
    /// tsconfig wildcard alias (`@shared/*`) gets a class map keyed by its
    /// physical path — `discover_css_source_files` walked the sibling's own
    /// `Button.tsx` (since it sits under a claimed
    /// `SiblingMirrorPlan` mirror root), and the scanner resolved its
    /// relative `./Button.module.css` import from there.
    #[test]
    fn compute_css_module_class_maps_includes_claimed_sibling_module() {
        let (_tmp, project) = sibling_css_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();
        let sibling_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/shared/Button.module.css"));

        let maps = compute_css_module_class_maps(&project, &[], &std::collections::BTreeSet::new())
            .expect("class maps");
        let names = maps.get(&sibling_module).unwrap_or_else(|| {
            panic!(
                "claimed sibling .module.css must get a class map keyed by its physical path \
                 {}; got keys: {:?}",
                sibling_module.display(),
                maps.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            names.contains_key("root"),
            "scoped class for `.root` must appear in the sibling's class map; got: {names:?}",
        );
    }

    /// Acceptance: with Tailwind disabled (hermetic — no tailwind binary
    /// required), the emitted stylesheet contains the scoped sibling class
    /// AND its name matches the one `compute_css_module_class_maps`
    /// (the bundler's JSX-rewrite producer) computed — proving the CSS
    /// emission path and the class-map path agree on a claimed sibling,
    /// driven end-to-end through the real command-layer functions (not a
    /// manually-supplied map).
    #[test]
    fn css_payload_emits_claimed_sibling_module_css_and_matches_class_map() {
        let (_tmp, project) = sibling_css_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload =
            build_default_css_payload(&project, &project.join("dist"), &cfg, &[], &[], &[])
                .expect("should not error")
                .expect("claimed sibling module must ship a non-empty payload");
        let css = String::from_utf8(payload.bytes).unwrap();

        let sibling_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/shared/Button.module.css"));
        let maps = compute_css_module_class_maps(&project, &[], &std::collections::BTreeSet::new())
            .expect("class maps");
        let scoped = maps
            .get(&sibling_module)
            .and_then(|names| names.get("root"))
            .cloned()
            .expect("bundler map must contain the scoped `.root` class for the sibling module");

        assert!(
            css.contains(&format!(".{scoped}")),
            "emitted CSS must contain the scoped sibling class `.{scoped}`; got:\n{css}",
        );
    }

    /// Acceptance: an UNCLAIMED sibling `.module.css` — no tsconfig/plugin
    /// alias targets its directory, so no `SiblingMirrorPlan` mirror root
    /// covers it — must not change CSS output. `discover_css_source_files`
    /// never walks it (nothing claims the directory), so it never becomes a
    /// scan source and the class map stays exactly as if the file did not
    /// exist. The CLAIMED sibling from the shared fixture is asserted
    /// present too, so the negative result is provably due to the missing
    /// claim, not a wholesale regression.
    #[test]
    fn compute_css_module_class_maps_ignores_unclaimed_sibling_module() {
        let (_tmp, project) = sibling_css_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap().to_path_buf();

        std::fs::create_dir_all(ws.join("lib/other")).unwrap();
        std::fs::write(
            ws.join("lib/other/Widget.tsx"),
            "import styles from \"./Widget.module.css\";\n\
             export default function Widget() { return styles.root; }\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("lib/other/Widget.module.css"),
            ".root { color: green; }\n",
        )
        .unwrap();

        let maps = compute_css_module_class_maps(&project, &[], &std::collections::BTreeSet::new())
            .expect("class maps");

        let unclaimed_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/other/Widget.module.css"));
        assert!(
            !maps.contains_key(&unclaimed_module),
            "an unclaimed sibling .module.css must not appear in the class map; got keys: {:?}",
            maps.keys().collect::<Vec<_>>()
        );

        let claimed_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/shared/Button.module.css"));
        assert!(
            maps.contains_key(&claimed_module),
            "the claimed sibling from the fixture must still be present; got keys: {:?}",
            maps.keys().collect::<Vec<_>>()
        );
    }

    /// Workspace fixture for the virtual-module CSS discovery tests (issue
    /// #1775): a sibling component reached ONLY through a registered plugin
    /// virtual module (claim source c) — deliberately NO tsconfig/plugin
    /// alias targets its directory (claim source b stays unused), so a
    /// passing test here can only be explained by the virtual-module
    /// discovery wiring, not the pre-existing alias claim path already
    /// covered by `sibling_css_workspace_fixture` above.
    ///
    /// Returns `(TempDir, project_root)`; the guard must stay alive for the
    /// fixture files to keep existing.
    fn sibling_css_virtual_module_workspace_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().unwrap();
        let ws = tmp.path();
        std::fs::write(
            ws.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();

        let project = ws.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();
        // Deliberately NO tsconfig.json / alias here — the sibling below is
        // reached ONLY via a registered virtual module in these tests.

        std::fs::create_dir_all(ws.join("lib/vshared")).unwrap();
        std::fs::write(
            ws.join("lib/vshared/Panel.tsx"),
            "import styles from \"./Panel.module.css\";\n\
             export default function Panel() { return styles.root; }\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("lib/vshared/Panel.module.css"),
            ".root { color: green; }\n",
        )
        .unwrap();

        (tmp, project)
    }

    /// Build a registered-virtual-module list of one entry, `virtual:panel`,
    /// whose source is a bare re-export of the fixture's sibling `Panel.tsx`
    /// — the same shape a real plugin's `addVirtualModule` callback returns.
    fn virtual_panel_module(ws: &Path) -> Vec<(String, String)> {
        let sibling_tsx = zfb_types::normalize_path_lexical(&ws.join("lib/vshared/Panel.tsx"));
        // `serde_json::to_string` (not manual quoting) so the path round-trips
        // as a valid JS string literal even on Windows, where a raw
        // `to_string_lossy()` path contains `\` that would otherwise land as
        // unintended escapes inside the double-quoted source below.
        let sibling_tsx_js_literal =
            serde_json::to_string(&sibling_tsx.to_string_lossy()).expect("path must serialize");
        vec![(
            "virtual:panel".to_string(),
            format!("export {{ default }} from {sibling_tsx_js_literal};\n"),
        )]
    }

    /// Acceptance (issue #1775): `discover_css_plugin_virtual_files` reaches
    /// a sibling source file through a registered virtual module alone —
    /// the resolver-backed discovery the islands/client-script bundlers
    /// already feed their own `SiblingMirrorPlan` claims with, reused here
    /// unchanged for CSS.
    #[test]
    fn discover_css_plugin_virtual_files_reaches_virtual_module_sibling() {
        let (_tmp, project) = sibling_css_virtual_module_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();
        let sibling_tsx = zfb_types::normalize_path_lexical(&ws.join("lib/vshared/Panel.tsx"));

        let worker_context = module_worker_build_context(
            false,
            crate::config::Framework::Preact,
            None,
            &[],
            &virtual_panel_module(ws),
        );
        let discovered = discover_css_plugin_virtual_files(&project, &worker_context)
            .expect("virtual discovery should not fail");
        assert!(
            discovered.contains(&sibling_tsx),
            "virtual-module discovery must reach the sibling source file {}; got: {:?}",
            sibling_tsx.display(),
            discovered
        );
    }

    /// Acceptance (issue #1775): a sibling `.module.css` reached ONLY
    /// through a registered virtual module (no direct alias) gets a class
    /// map keyed by its physical path — the same outcome
    /// `compute_css_module_class_maps_includes_claimed_sibling_module`
    /// proves for the alias claim path, now proven for claim source c.
    #[test]
    fn compute_css_module_class_maps_includes_virtual_only_sibling_module() {
        let (_tmp, project) = sibling_css_virtual_module_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();
        let sibling_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/vshared/Panel.module.css"));

        let worker_context = module_worker_build_context(
            false,
            crate::config::Framework::Preact,
            None,
            &[],
            &virtual_panel_module(ws),
        );
        let discovered_graph_files = discover_css_plugin_virtual_files(&project, &worker_context)
            .expect("virtual discovery should not fail");

        // No alias entries at all — proves the class map does not depend on
        // claim source b for this fixture.
        let maps = compute_css_module_class_maps(&project, &[], &discovered_graph_files)
            .expect("class maps");
        let names = maps.get(&sibling_module).unwrap_or_else(|| {
            panic!(
                "virtual-only sibling .module.css must get a class map keyed by its physical \
                 path {}; got keys: {:?}",
                sibling_module.display(),
                maps.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            names.contains_key("root"),
            "scoped class for `.root` must appear in the virtual-only sibling's class map; \
             got: {names:?}",
        );
    }

    /// Acceptance (issue #1775): the Tailwind-DISABLED path
    /// (`build_authored_only_css_payload`) must also discover a
    /// virtual-only sibling CSS Module — `enabled: false` opts out of
    /// Tailwind, not out of CSS (issue #824), and that invariant must hold
    /// for claim source c too, not just the alias claim path
    /// `css_payload_emits_claimed_sibling_module_css_and_matches_class_map`
    /// already covers.
    #[test]
    fn css_payload_emits_virtual_only_sibling_module_css_with_tailwind_disabled() {
        let (_tmp, project) = sibling_css_virtual_module_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();
        let plugin_virtual_modules = virtual_panel_module(ws);

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload = build_default_css_payload(
            &project,
            &project.join("dist"),
            &cfg,
            &[],
            &[],
            &plugin_virtual_modules,
        )
        .expect("should not error")
        .expect("virtual-only sibling module must ship a non-empty payload");
        let css = String::from_utf8(payload.bytes).unwrap();

        let worker_context = module_worker_build_context(
            false,
            crate::config::Framework::Preact,
            None,
            &[],
            &plugin_virtual_modules,
        );
        let discovered_graph_files = discover_css_plugin_virtual_files(&project, &worker_context)
            .expect("virtual discovery should not fail");
        let maps = compute_css_module_class_maps(&project, &[], &discovered_graph_files)
            .expect("class maps");
        let sibling_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/vshared/Panel.module.css"));
        let scoped = maps
            .get(&sibling_module)
            .and_then(|names| names.get("root"))
            .cloned()
            .expect("class map must contain the scoped `.root` class for the virtual-only sibling");

        assert!(
            css.contains(&format!(".{scoped}")),
            "emitted CSS (Tailwind disabled) must contain the scoped virtual-only sibling class \
             `.{scoped}`; got:\n{css}",
        );
    }

    /// Workspace fixture for the DIRECT virtual→`.module.css` case (issue
    /// #1775 follow-up): a sibling `.module.css` with NO intermediate JS/TS
    /// component importing it — the ONLY path that reaches it is a registered
    /// virtual module whose source imports the CSS directly. Deliberately no
    /// `.tsx` sibling and no tsconfig alias, so a passing test can only be
    /// explained by the direct-CSS discovery wiring, not the mirror-root
    /// scan-source walk or the alias claim path.
    fn direct_virtual_css_module_workspace_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().unwrap();
        let ws = tmp.path();
        std::fs::write(
            ws.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();

        let project = ws.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();

        std::fs::create_dir_all(ws.join("lib/vdirect")).unwrap();
        std::fs::write(
            ws.join("lib/vdirect/styles.module.css"),
            ".root { color: rebeccapurple; }\n",
        )
        .unwrap();

        (tmp, project)
    }

    /// Register one virtual module, `virtual:direct-styles`, whose source
    /// imports the fixture's sibling `.module.css` DIRECTLY — no intermediate
    /// component. Mirrors what a real plugin's `addVirtualModule` callback
    /// returns for a virtual module that re-exports a sibling stylesheet's
    /// class map.
    fn virtual_direct_css_module(ws: &Path) -> Vec<(String, String)> {
        let sibling_css =
            zfb_types::normalize_path_lexical(&ws.join("lib/vdirect/styles.module.css"));
        let sibling_css_js_literal =
            serde_json::to_string(&sibling_css.to_string_lossy()).expect("path must serialize");
        vec![(
            "virtual:direct-styles".to_string(),
            format!("import styles from {sibling_css_js_literal};\nexport default styles;\n"),
        )]
    }

    /// Regression (issue #1775 follow-up): a registered virtual module whose
    /// source imports a sibling `.module.css` DIRECTLY (no intermediate JS/TS
    /// component) must land the CSS file in the discovery graph AND get a
    /// class map keyed by its physical path. Before the fix,
    /// `discover_css_source_files` returned only JS/TS/MD sources and the
    /// in-memory virtual source was never scanned, so the module silently
    /// dropped out of the class map while the bundler rewrote it to
    /// `export default {}`.
    #[test]
    fn compute_css_module_class_maps_includes_direct_virtual_css_module() {
        let (_tmp, project) = direct_virtual_css_module_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();
        let sibling_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/vdirect/styles.module.css"));

        let worker_context = module_worker_build_context(
            false,
            crate::config::Framework::Preact,
            None,
            &[],
            &virtual_direct_css_module(ws),
        );
        let discovered_graph_files = discover_css_plugin_virtual_files(&project, &worker_context)
            .expect("virtual discovery should not fail");
        assert!(
            discovered_graph_files.contains(&sibling_module),
            "virtual-module discovery must reach the directly-imported .module.css {}; got: {:?}",
            sibling_module.display(),
            discovered_graph_files
        );

        let maps = compute_css_module_class_maps(&project, &[], &discovered_graph_files)
            .expect("class maps");
        let names = maps.get(&sibling_module).unwrap_or_else(|| {
            panic!(
                "direct virtual→CSS import must get a class map keyed by its physical path {}; \
                 got keys: {:?}",
                sibling_module.display(),
                maps.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            names.contains_key("root"),
            "scoped class for `.root` must appear in the direct virtual CSS module's class map; \
             got: {names:?}",
        );
    }

    /// Regression (issue #1775 follow-up), Tailwind-DISABLED variant: the
    /// authored-only path (`build_authored_only_css_payload`) must also
    /// compile and emit a directly-imported virtual-only CSS Module —
    /// `enabled: false` opts out of Tailwind, not out of CSS (issue #824). The
    /// emitted scoped class must match the one `compute_css_module_class_maps`
    /// produces, proving the emission and class-map paths agree on the direct
    /// virtual CSS import.
    #[test]
    fn css_payload_emits_direct_virtual_css_module_with_tailwind_disabled() {
        let (_tmp, project) = direct_virtual_css_module_workspace_fixture();
        let ws = project.parent().unwrap().parent().unwrap();
        let plugin_virtual_modules = virtual_direct_css_module(ws);

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload = build_default_css_payload(
            &project,
            &project.join("dist"),
            &cfg,
            &[],
            &[],
            &plugin_virtual_modules,
        )
        .expect("should not error")
        .expect("direct virtual CSS module must ship a non-empty payload");
        let css = String::from_utf8(payload.bytes).unwrap();

        let worker_context = module_worker_build_context(
            false,
            crate::config::Framework::Preact,
            None,
            &[],
            &plugin_virtual_modules,
        );
        let discovered_graph_files = discover_css_plugin_virtual_files(&project, &worker_context)
            .expect("virtual discovery should not fail");
        let maps = compute_css_module_class_maps(&project, &[], &discovered_graph_files)
            .expect("class maps");
        let sibling_module =
            zfb_types::normalize_path_lexical(&ws.join("lib/vdirect/styles.module.css"));
        let scoped = maps
            .get(&sibling_module)
            .and_then(|names| names.get("root"))
            .cloned()
            .expect(
                "class map must contain the scoped `.root` class for the direct virtual module",
            );

        assert!(
            css.contains(&format!(".{scoped}")),
            "emitted CSS (Tailwind disabled) must contain the scoped direct virtual CSS module \
             class `.{scoped}`; got:\n{css}",
        );
    }

    /// Explicit empty-plugin parity test (issue #1775): with no registered
    /// virtual modules, `discover_css_plugin_virtual_files` contributes
    /// nothing even when an (unrelated) alias registration IS present —
    /// claim source c never fires without an actual virtual-module
    /// registration, so a project on the pre-#1775 alias-only path stays
    /// byte-identical.
    #[test]
    fn discover_css_plugin_virtual_files_is_empty_with_no_virtual_modules() {
        let (_tmp, project) = sibling_css_workspace_fixture();
        let worker_context = module_worker_build_context(
            true,
            crate::config::Framework::Preact,
            None,
            &[("@shared/*".to_string(), "unused".to_string())],
            &[],
        );
        let discovered = discover_css_plugin_virtual_files(&project, &worker_context)
            .expect("virtual discovery should not fail");
        assert!(
            discovered.is_empty(),
            "no registered virtual modules must contribute nothing to CSS discovery; got: {discovered:?}",
        );
    }

    /// Pure-logic coverage (Level 1) of the `resolve_framework_css` gating
    /// predicate itself, isolated from the heavier payload-builder tests
    /// above: the 3 meaningfully distinct `(mode, default_stylesheet)`
    /// combinations.
    #[test]
    fn resolve_framework_css_gating_matrix() {
        let class_on = Config {
            code_highlight: Some(code_highlight_config(CodeHighlightMode::Class, true, "hi-")),
            ..Config::default()
        };
        assert!(
            resolve_framework_css(&class_on).is_some(),
            "class mode + defaultStylesheet:true must inject the framework block",
        );

        let class_off = Config {
            code_highlight: Some(code_highlight_config(
                CodeHighlightMode::Class,
                false,
                "hi-",
            )),
            ..Config::default()
        };
        assert!(
            resolve_framework_css(&class_off).is_none(),
            "class mode + defaultStylesheet:false must NOT inject the framework block",
        );

        let no_code_highlight = Config::default();
        assert!(
            resolve_framework_css(&no_code_highlight).is_none(),
            "absent codeHighlight config (mode defaults to inline) must NOT inject the framework block",
        );
    }

    /// Highlight Tokens epic sub #1533: a custom `codeHighlight.classPrefix`
    /// must rewrite the default stylesheet's `.hi-*` role SELECTORS to the
    /// configured prefix (otherwise the `{classPrefix}{role}` spans the
    /// emitter produces would match nothing — a silent dead stylesheet). The
    /// `--zfb-hi-*` custom PROPERTIES are namespaced independently of
    /// classPrefix and must stay `--zfb-hi-*`.
    #[test]
    fn resolve_framework_css_rewrites_selectors_for_custom_class_prefix() {
        let cfg = Config {
            code_highlight: Some(code_highlight_config(
                CodeHighlightMode::Class,
                true,
                "syn-",
            )),
            ..Config::default()
        };
        let css = resolve_framework_css(&cfg)
            .expect("class mode + defaultStylesheet:true must inject the framework block");

        assert!(
            css.contains(".syn-kw"),
            "the role selector must use the configured classPrefix; got:\n{css}",
        );
        assert!(
            !css.contains(".hi-kw"),
            "the default `.hi-` selector must be rewritten away; got:\n{css}",
        );
        // Custom properties are prefix-independent — both the declaration and
        // the `var()` reference must stay `--zfb-hi-*`.
        assert!(
            css.contains("--zfb-hi-kw:"),
            "the --zfb-hi-* property declaration must stay intact; got:\n{css}",
        );
        assert!(
            css.contains("var(--zfb-hi-kw)"),
            "the var(--zfb-hi-*) reference must stay intact; got:\n{css}",
        );
    }

    /// Highlight Tokens epic confirm sub (zfb#1535), check 6, part 2 of 2:
    /// the full three-way role-taxonomy parity assertion. `zfb` is the
    /// only crate that depends on BOTH `zfb-content` (the classifier,
    /// `HiRole::ALL` / zfb#1529) and `zfb-css` (the stylesheet,
    /// `default_hi_css()` / zfb#1531), and it owns the config validation
    /// list ([`CODE_HIGHLIGHT_ROLES`]) itself — so this is the one test
    /// that can see all three legs in the same process:
    ///
    /// 1. classifier short/full names (`zfb_content::hi_roles::HiRole`)
    /// 2. config validation list (`CODE_HIGHLIGHT_ROLES`, full names)
    /// 3. stylesheet suffixes (`zfb_css::default_hi_css()`, short names)
    ///
    /// A PARTIAL of this (legs 1 and 3 only — classifier <-> stylesheet)
    /// lives in `crates/zfb-css/tests/hi_role_parity.rs`; that crate
    /// cannot see `CODE_HIGHLIGHT_ROLES` without depending on `zfb`, which
    /// would be a cycle. This full three-way test is `zfb`-tier: the `zfb`
    /// crate's test binaries link the embedded V8 host (the `embed_v8`
    /// default feature), so they run in CI via `health.yml`'s
    /// `cargo nextest run --workspace` rather than in a lightweight local
    /// subset. The `zfb-css` partial above is V8-free and covers the
    /// classifier<->stylesheet legs on every local `cargo test -p zfb-css`.
    #[test]
    fn hi_role_taxonomy_parity_across_classifier_config_and_stylesheet() {
        use zfb_content::hi_roles::HiRole;

        assert_eq!(
            HiRole::ALL.len(),
            crate::config::CODE_HIGHLIGHT_ROLES.len(),
            "classifier role count must match the config validation list length"
        );

        let css = zfb_css::default_hi_css();

        for (role, config_name) in HiRole::ALL
            .iter()
            .zip(crate::config::CODE_HIGHLIGHT_ROLES.iter())
        {
            // Leg 1 <-> Leg 2: classifier `full_name()` — the key the
            // class-mode emitter resolves `roleClasses` overrides by — must
            // equal the config validation list entry at the SAME taxonomy
            // index. `CODE_HIGHLIGHT_ROLES`'s doc comment promises it
            // "matches the #1529 table" in order, not just as an unordered
            // set. This is load-bearing: if `full_name()` drifts from the
            // config key, overrides silently miss (zfb#1528 deep-review fix).
            assert_eq!(
                role.full_name(),
                *config_name,
                "classifier role {role:?} full_name() must match CODE_HIGHLIGHT_ROLES \
                 entry {config_name:?} at the same taxonomy index",
            );

            // Leg 1 <-> Leg 3: classifier short_name() must have a
            // declared --zfb-hi-<suffix> property AND a .hi-<suffix>
            // selector in the shipped stylesheet.
            let suffix = role.short_name();
            assert!(
                css.contains(&format!("--zfb-hi-{suffix}:")),
                "stylesheet must declare --zfb-hi-{suffix} for role {role:?}; got:\n{css}",
            );
            assert!(
                css.contains(&format!(".hi-{suffix} {{")),
                "stylesheet must declare .hi-{suffix} selector for role {role:?}; got:\n{css}",
            );
        }
    }

    /// Parse only a line-anchored semantic-role alias and its bounded union
    /// members. This deliberately stops at that declaration's semicolon
    /// instead of scanning arbitrary TypeScript string literals elsewhere in
    /// the file.
    fn parse_highlight_role_union(
        source: &str,
        alias: &str,
    ) -> std::result::Result<BTreeSet<String>, String> {
        let mut lines = source.lines().enumerate();
        let (alias_line, _) = lines
            .find(|(_, line)| line.trim() == alias)
            .ok_or_else(|| format!("missing line-anchored `{alias}` declaration"))?;

        let mut roles = BTreeSet::new();
        for (line_number, line) in lines {
            let member = line.trim();
            if member.is_empty() {
                continue;
            }

            let (member, terminates_declaration) = match member.strip_suffix(';') {
                Some(member) => (member.trim_end(), true),
                None => (member, false),
            };
            let member = member.strip_prefix('|').map(str::trim).ok_or_else(|| {
                format!(
                    "expected a `| \"role\"` member in {alias} at line {}",
                    line_number + 1
                )
            })?;
            let role = member
                .strip_prefix('"')
                .and_then(|role| role.strip_suffix('"'))
                .filter(|role| !role.is_empty())
                .ok_or_else(|| {
                    format!(
                        "expected a quoted role in {alias} at line {}",
                        line_number + 1
                    )
                })?;
            if !roles.insert(role.to_string()) {
                return Err(format!(
                    "duplicate {alias} member {role:?} at line {}",
                    line_number + 1
                ));
            }
            if terminates_declaration {
                return Ok(roles);
            }
        }

        Err(format!(
            "{alias} declaration starting at line {} is missing its terminating semicolon",
            alias_line + 1
        ))
    }

    /// The TypeScript config helper has a hand-authored public union, while
    /// Rust owns the canonical config-validation names. Keep both legs in
    /// lockstep without relying on the process working directory.
    #[test]
    fn typescript_code_highlight_role_union_matches_rust_canonical_roles() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(|crates_dir| crates_dir.parent())
            .expect("zfb crate must live under the workspace crates directory")
            .to_path_buf();
        let typescript_path = workspace_root.join("packages/zfb/src/config.ts");
        let source = std::fs::read_to_string(&typescript_path).unwrap_or_else(|error| {
            panic!(
                "read TypeScript CodeHighlightRole at {}: {error}",
                typescript_path.display()
            )
        });
        let typescript_roles =
            parse_highlight_role_union(&source, "export type CodeHighlightRole =").unwrap_or_else(
                |error| {
                    panic!(
                        "parse TypeScript CodeHighlightRole at {}: {error}",
                        typescript_path.display()
                    )
                },
            );
        let rust_roles: BTreeSet<String> = crate::config::CODE_HIGHLIGHT_ROLES
            .iter()
            .map(|role| (*role).to_string())
            .collect();

        let missing_from_typescript: Vec<&String> =
            rust_roles.difference(&typescript_roles).collect();
        let extra_in_typescript: Vec<&String> = typescript_roles.difference(&rust_roles).collect();
        assert!(
            missing_from_typescript.is_empty() && extra_in_typescript.is_empty(),
            "TypeScript CodeHighlightRole in {} must match Rust CODE_HIGHLIGHT_ROLES; \
             missing from TypeScript CodeHighlightRole: {missing_from_typescript:?}; \
             extra in TypeScript CodeHighlightRole: {extra_in_typescript:?}",
            typescript_path.display(),
        );
    }

    /// The published `@takazudo/zfb-md-wasm` direct API has its own exported
    /// `HighlightRole` union. It necessarily spells out the public TypeScript
    /// literals, so guard it against Rust's canonical taxonomy just as we do
    /// the config helper rather than allowing a second untracked role list.
    #[test]
    fn typescript_wasm_highlight_role_union_matches_rust_canonical_roles() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(|crates_dir| crates_dir.parent())
            .expect("zfb crate must live under the workspace crates directory")
            .to_path_buf();
        let typescript_path = workspace_root.join("crates/zfb-md-wasm/npm/src/types.ts");
        let source = std::fs::read_to_string(&typescript_path).unwrap_or_else(|error| {
            panic!(
                "read TypeScript HighlightRole at {}: {error}",
                typescript_path.display()
            )
        });
        let typescript_roles = parse_highlight_role_union(&source, "export type HighlightRole =")
            .unwrap_or_else(|error| {
                panic!(
                    "parse TypeScript HighlightRole at {}: {error}",
                    typescript_path.display()
                )
            });
        let rust_roles: BTreeSet<String> = crate::config::CODE_HIGHLIGHT_ROLES
            .iter()
            .map(|role| (*role).to_string())
            .collect();

        let missing_from_typescript: Vec<&String> =
            rust_roles.difference(&typescript_roles).collect();
        let extra_in_typescript: Vec<&String> = typescript_roles.difference(&rust_roles).collect();
        assert!(
            missing_from_typescript.is_empty() && extra_in_typescript.is_empty(),
            "TypeScript HighlightRole in {} must match Rust CODE_HIGHLIGHT_ROLES; \
             missing from TypeScript HighlightRole: {missing_from_typescript:?}; \
             extra in TypeScript HighlightRole: {extra_in_typescript:?}",
            typescript_path.display(),
        );
    }

    /// Regression (zfb#1528 deep-review): a `codeHighlight.roleClasses`
    /// override — keyed by the FULL role name, the only form config accepts —
    /// must survive the config -> `PipelineSpec` lowering with its key intact
    /// (NOT rekeyed to the short suffix). The class-mode emitter resolves
    /// overrides by `HiRole::full_name()` (`"keyword"`), so a lowering that
    /// dropped or renamed the key would silently disable every override.
    /// Pairs with the content-crate test
    /// `pipeline_spec_class_mode_applies_role_classes_override`, which proves
    /// the key is then honored end-to-end in the emitted HTML.
    #[test]
    fn role_classes_full_name_key_survives_config_to_spec_lowering() {
        let config: crate::config::Config = serde_json::from_str(
            r#"{"codeHighlight":{"mode":"class","roleClasses":{"keyword":"text-violet-600 dark:text-violet-400"}}}"#,
        )
        .expect("config parses");
        let spec = crate::commands::bundler_input::pipeline_spec_from_config(
            std::path::Path::new("."),
            &config,
        );
        assert_eq!(
            spec.code_highlight_role_classes
                .get("keyword")
                .map(String::as_str),
            Some("text-violet-600 dark:text-violet-400"),
            "the full-name roleClasses key must reach PipelineSpec unchanged; got {:?}",
            spec.code_highlight_role_classes,
        );
        assert!(
            !spec.code_highlight_role_classes.contains_key("kw"),
            "lowering must not rekey the override to the short suffix",
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
            workspace_package_edges_from_islands: Vec::new(),
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

    /// Issue #1703, Stage Escape Guards — Guard (a): once this shadow is
    /// materialised at all (some other preprocessing need — glob, `?raw`,
    /// or a module worker — already made staging necessary), any bare
    /// package-name import of a workspace sibling recorded on `scan_meta`
    /// must hard-error naming the offending specifier and importer. The
    /// wholesale `node_modules` symlink this shadow creates below would
    /// otherwise let the import resolve straight to unprocessed source,
    /// silently bypassing every rewrite the shadow exists to stage.
    /// `scan_meta.workspace_package_edges_from_islands` is set directly
    /// here (rather than produced by a real scan) since production only
    /// ever calls this function once the caller's own glob/raw/worker gate
    /// already determined staging is needed — this test exercises Guard
    /// (a)'s check in isolation.
    #[test]
    fn materialise_islands_shadow_hard_errors_on_workspace_package_edge() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("components")).unwrap();
        let island_src = project_root.join("components/gallery.tsx");
        std::fs::write(
            &island_src,
            "\"use client\";\nexport function Gallery() { return null; }\n",
        )
        .unwrap();

        let islands = vec![zfb_islands::Island::new("Gallery", island_src.clone())];
        let scan_meta = zfb_islands::ScanMeta {
            uses_client_router: false,
            near_miss_candidates: 0,
            glob_reachable_from_islands: Vec::new(),
            island_reachable_modules: vec![island_src.clone()],
            raw_import_edges_from_islands: Vec::new(),
            module_worker_edges_from_islands: Vec::new(),
            workspace_package_edges_from_islands: vec![zfb_islands::WorkspacePackageImportEdge {
                importer: island_src.clone(),
                specifier: "@acme/shared".to_string(),
                package_dir: project_root.join("node_modules/@acme/shared"),
            }],
        };

        // `.err().expect(...)` rather than `.unwrap_err()`: the Ok type
        // `IslandsShadowOutcome` is not `Debug`, which `Result::unwrap_err`
        // would require.
        let error = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .err()
            .expect("Guard (a) must reject the workspace-package edge once staging is active");
        let message = format!("{error:#}");
        assert!(message.contains("@acme/shared"), "{message}");
        assert!(
            message.contains(island_src.display().to_string().as_str()),
            "{message}"
        );
        assert!(
            message.contains("not supported once staging is active"),
            "{message}"
        );
    }

    /// Issue #1708, Stage Escape Guards confirm pass — Guard (a)
    /// end-to-end: the test above hand-builds `ScanMeta` to exercise the
    /// check in isolation; this test instead drives the REAL islands
    /// scanner (`scan_islands_with_meta`) against a genuine
    /// `node_modules` workspace-package symlink fixture (the same
    /// `link_workspace_package` helper the client-script counterpart below
    /// uses), so `workspace_package_edges_from_islands` is populated by
    /// production code, not injected by the test. The island bare-imports
    /// `@acme/shared` and also needs `?raw` staging, so this proves the
    /// full real-scan → real-staging-check path a real build takes — and
    /// it never reaches esbuild.
    #[cfg(unix)]
    #[test]
    fn materialise_islands_shadow_hard_errors_on_workspace_package_edge_from_real_scan() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        link_workspace_package(root);
        std::fs::create_dir_all(root.join("components")).unwrap();
        let island_src = root.join("components/gallery.tsx");
        std::fs::write(
            &island_src,
            "'use client';\n\
             import { helper } from '@acme/shared';\n\
             import text from './message.txt?raw';\n\
             export function Gallery() { console.log(helper, text); return null; }\n",
        )
        .unwrap();
        std::fs::write(root.join("components/message.txt"), "hello").unwrap();

        let (islands, scan_meta) =
            scan_islands_with_meta(std::slice::from_ref(&island_src), &FsResolver::new()).unwrap();
        assert_eq!(
            islands.len(),
            1,
            "the \"use client\" component must be discovered as a real island"
        );
        assert_eq!(
            scan_meta.workspace_package_edges_from_islands.len(),
            1,
            "the real scanner must record the bare @acme/shared import as a workspace-package \
             edge"
        );

        let error = materialise_islands_shadow(root, &islands, &scan_meta)
            .err()
            .expect("Guard (a) must reject the real-scanned workspace-package edge");
        let message = format!("{error:#}");
        assert!(message.contains("@acme/shared"), "{message}");
        assert!(
            message.contains(island_src.display().to_string().as_str()),
            "{message}"
        );
        assert!(
            message.contains("not supported once staging is active"),
            "{message}"
        );
    }

    /// Issue #1731 / #2161: an `npm link` / `file:` dependency whose
    /// `node_modules/<pkg>` symlink resolves OUTSIDE the project's
    /// first-party boundary is a legitimate external dependency, not a
    /// workspace sibling — unlike the genuine in-root sibling fixture
    /// above (which must keep hard-erroring), this package lives in a
    /// wholly separate temp directory. Driving the exact production call
    /// shape `build_default_islands_payload_with_bundle_options` uses
    /// (`scan_islands_with_meta_and_first_party_root` scoped to
    /// `first_party_root`, then `materialise_islands_shadow`) must
    /// SUCCEED rather than hard-erroring — a scanner-only test cannot
    /// prove this, since Guard (a)'s hard-fail lives here, at the build
    /// layer, and only fires once `materialise_islands_shadow_with_worker_context`
    /// actually runs.
    #[cfg(unix)]
    #[test]
    fn materialise_islands_shadow_accepts_external_linked_package_outside_first_party_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let external_tmp = tempdir().unwrap();
        let external_pkg = external_tmp.path().join("linked");
        std::fs::create_dir_all(external_pkg.join("src")).unwrap();
        std::fs::write(
            external_pkg.join("package.json"),
            r#"{"name":"@acme/external","source":"src/index.ts"}"#,
        )
        .unwrap();
        std::fs::write(
            external_pkg.join("src/index.ts"),
            "export const helper = 1;\n",
        )
        .unwrap();
        let scope_dir = root.join("node_modules/@acme");
        std::fs::create_dir_all(&scope_dir).unwrap();
        std::os::unix::fs::symlink(&external_pkg, scope_dir.join("external")).unwrap();

        std::fs::create_dir_all(root.join("components")).unwrap();
        let island_src = root.join("components/gallery.tsx");
        std::fs::write(
            &island_src,
            "'use client';\n\
             import { helper } from '@acme/external';\n\
             import text from './message.txt?raw';\n\
             export function Gallery() { console.log(helper, text); return null; }\n",
        )
        .unwrap();
        std::fs::write(root.join("components/message.txt"), "hello").unwrap();

        let (islands, scan_meta) = scan_islands_with_meta_and_first_party_root(
            std::slice::from_ref(&island_src),
            &FsResolver::new(),
            Some(root),
        )
        .unwrap();
        assert_eq!(
            islands.len(),
            1,
            "the \"use client\" component must still be discovered as a real island"
        );
        assert!(
            scan_meta.workspace_package_edges_from_islands.is_empty(),
            "an npm-link/file: dependency outside first_party_root must not be recorded as a \
             workspace-package edge: {:?}",
            scan_meta.workspace_package_edges_from_islands
        );

        materialise_islands_shadow(root, &islands, &scan_meta).expect(
            "Guard (a) must not reject an external-linked dependency outside \
             first_party_root, even though the closure needs ?raw staging",
        );
    }

    #[test]
    fn materialise_islands_shadow_copies_nearest_config_and_relative_extends_chain() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("components/feature")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/tsconfig.base.json"),
            r#"{"compilerOptions":{"baseUrl":"..","paths":{"@feature/*":["components/feature/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("components/feature/tsconfig.json"),
            r#"{"extends":"../../config/tsconfig.base.json"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("components/feature/jsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@wrong/*":["missing/*"]}}}"#,
        )
        .unwrap();
        let island = root.join("components/feature/Island.tsx");
        let worker = root.join("components/feature/worker.ts");
        std::fs::write(
            &island,
            "'use client'; export function Island() { new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }); return null; }\n",
        )
        .unwrap();
        std::fs::write(&worker, "self.postMessage('ready');\n").unwrap();

        let (islands, scan_meta) =
            scan_islands_with_meta(std::slice::from_ref(&island), &FsResolver::new()).unwrap();
        let shadow = match materialise_islands_shadow(root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("supported worker must materialise: {offenders:?}")
            }
        };
        let shadow_root = shadow._tempdir.path();
        let nested = shadow_root.join("components/feature/tsconfig.json");
        assert_eq!(
            std::fs::read_to_string(&nested).unwrap(),
            r#"{"extends":"../../config/tsconfig.base.json"}"#,
            "an internal relative extends edge keeps its layout and spelling"
        );
        assert!(
            shadow_root.join("config/tsconfig.base.json").is_file(),
            "the selected leaf's relative extends chain must be copied"
        );
        assert!(
            !shadow_root
                .join("components/feature/jsconfig.json")
                .exists(),
            "tsconfig.json wins over a same-directory jsconfig.json, matching esbuild"
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&nested);
        assert_eq!(
            paths["@feature/*"][0],
            shadow_root.join("components/feature/*").to_string_lossy(),
            "the copied chain must resolve aliases into the shadow"
        );
    }

    fn write_standalone_shadow_config_fixture(
        config: &Path,
        base_url: &str,
        scope: &str,
        local_target: &str,
        external_target: &str,
        absolute_external_target: &str,
    ) {
        let value = serde_json::json!({
            "compilerOptions": {
                "baseUrl": base_url,
                "paths": {
                    (format!("@{scope}-local/*")): [local_target],
                    (format!("@{scope}-external/*")): [external_target],
                    (format!("@{scope}-absolute/*")): [absolute_external_target]
                }
            }
        });
        std::fs::write(config, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }

    fn assert_standalone_shadow_config_rebased(
        config: &Path,
        expected_base_url: &Path,
        scope: &str,
        expected_local_target: &Path,
        expected_external_target: &Path,
        absolute_external_target: &str,
    ) {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
        assert_eq!(
            value["compilerOptions"]["baseUrl"].as_str(),
            Some(expected_base_url.to_string_lossy().as_ref())
        );
        assert_eq!(
            value["compilerOptions"]["paths"][format!("@{scope}-local/*")][0].as_str(),
            Some(expected_local_target.to_string_lossy().as_ref())
        );
        assert_eq!(
            value["compilerOptions"]["paths"][format!("@{scope}-external/*")][0].as_str(),
            Some(expected_external_target.to_string_lossy().as_ref())
        );
        assert_eq!(
            value["compilerOptions"]["paths"][format!("@{scope}-absolute/*")][0].as_str(),
            Some(absolute_external_target),
            "already-absolute external aliases must retain their authored spelling"
        );
    }

    #[test]
    fn materialise_islands_shadow_rebases_standalone_root_and_nested_configs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let external = tmp.path().join("external");
        for dir in [
            root.join("pages"),
            root.join("components/feature"),
            root.join("src/root-local"),
            root.join("src/nested-local"),
            external.join("root-lib"),
            external.join("nested-lib"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let absolute_external = external
            .join("../external/absolute/*")
            .to_string_lossy()
            .into_owned();
        write_standalone_shadow_config_fixture(
            &root.join("tsconfig.json"),
            "../external",
            "root",
            "../project/src/root-local/*",
            "root-lib/*",
            &absolute_external,
        );
        write_standalone_shadow_config_fixture(
            &root.join("components/feature/jsconfig.json"),
            "../../../external",
            "nested",
            "../project/src/nested-local/*",
            "nested-lib/*",
            &absolute_external,
        );

        let root_island = root.join("RootIsland.tsx");
        let nested_island = root.join("components/feature/NestedIsland.tsx");
        std::fs::write(
            root.join("pages/index.tsx"),
            "import { RootIsland } from '../RootIsland';\n\
             import { NestedIsland } from '../components/feature/NestedIsland';\n\
             export default function Page() { return RootIsland() + NestedIsland(); }\n",
        )
        .unwrap();
        std::fs::write(
            &root_island,
            "'use client'; import raw from './root.txt?raw'; export function RootIsland() { return raw; }\n",
        )
        .unwrap();
        std::fs::write(root.join("root.txt"), "root raw").unwrap();
        std::fs::write(
            &nested_island,
            "'use client'; import raw from './nested.txt?raw'; export function NestedIsland() { return raw; }\n",
        )
        .unwrap();
        std::fs::write(root.join("components/feature/nested.txt"), "nested raw").unwrap();

        let (islands, scan_meta) =
            scan_islands_with_meta(&[root.join("pages/index.tsx")], &FsResolver::new()).unwrap();
        let shadow = match materialise_islands_shadow(&root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("standalone-config raw islands must materialise: {offenders:?}")
            }
        };
        let shadow_root = shadow._tempdir.path();
        assert_standalone_shadow_config_rebased(
            &shadow_root.join("tsconfig.json"),
            &external,
            "root",
            &shadow_root.join("src/root-local/*"),
            &external.join("root-lib/*"),
            &absolute_external,
        );
        assert_standalone_shadow_config_rebased(
            &shadow_root.join("components/feature/jsconfig.json"),
            &external,
            "nested",
            &shadow_root.join("src/nested-local/*"),
            &external.join("nested-lib/*"),
            &absolute_external,
        );
    }

    #[test]
    fn materialise_islands_shadow_rewrites_external_relative_extends_absolute() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let shared_name = format!(
            "{}-external-config",
            tmp.path().file_name().unwrap().to_string_lossy()
        );
        let shared = tmp.path().join(&shared_name);
        let escaped_shadow_target = tmp
            .path()
            .parent()
            .unwrap()
            .join(&shared_name)
            .join("tsconfig.base.json");
        assert!(!escaped_shadow_target.exists());
        std::fs::create_dir_all(root.join("components")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        let external_config = shared.join("tsconfig.base.json");
        std::fs::write(
            &external_config,
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@shared/*":["src/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            format!(r#"{{"extends":"../{shared_name}/tsconfig.base.json"}}"#),
        )
        .unwrap();
        let island = root.join("components/Island.tsx");
        let worker = root.join("components/worker.ts");
        std::fs::write(
            &island,
            "'use client'; export function Island() { new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }); return null; }\n",
        )
        .unwrap();
        std::fs::write(&worker, "self.postMessage('ready');\n").unwrap();

        let (islands, scan_meta) =
            scan_islands_with_meta(std::slice::from_ref(&island), &FsResolver::new()).unwrap();
        let shadow = match materialise_islands_shadow(&root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("supported worker must materialise: {offenders:?}")
            }
        };
        let copied = shadow._tempdir.path().join("tsconfig.json");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&copied).unwrap()).unwrap();
        let canonical_external_config = external_config.canonicalize().unwrap();
        assert_eq!(
            json["extends"].as_str(),
            Some(canonical_external_config.to_string_lossy().as_ref()),
            "an external relative edge must remain valid after the leaf moves into a tempdir"
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&copied);
        assert_eq!(
            paths["@shared/*"][0],
            shared
                .canonicalize()
                .unwrap()
                .join("src/*")
                .to_string_lossy(),
            "the rewritten absolute extends path must remain readable"
        );
        assert!(
            !escaped_shadow_target.exists(),
            "config mirroring must never write through a lexical `..` outside the shadow"
        );
    }

    #[test]
    fn materialise_shadow_typescript_configs_preserves_extends_array_semantics() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let shadow = tmp.path().join("shadow");
        let shared = tmp.path().join("shared");
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("components")).unwrap();
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(
            root.join("config/paths.json"),
            r#"{"compilerOptions":{"paths":{"@merged/*":["feature/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("config/base-url.json"),
            r#"{"compilerOptions":{"baseUrl":"../later-base"}}"#,
        )
        .unwrap();
        let external = shared.join("external.json");
        std::fs::write(&external, "{}").unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":["./config/paths.json","./config/base-url.json","../shared/external.json"]}"#,
        )
        .unwrap();
        let source = root.join("components/Island.tsx");
        std::fs::write(&source, "export {};").unwrap();

        let configs = collect_islands_shadow_configs(&root, [&source]).unwrap();
        materialise_shadow_typescript_configs(&root, &shadow, &configs).unwrap();

        assert!(shadow.join("config/paths.json").is_file());
        assert!(shadow.join("config/base-url.json").is_file());
        let leaf = shadow.join("tsconfig.json");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&leaf).unwrap()).unwrap();
        let extends = json["extends"].as_array().unwrap();
        assert_eq!(extends[0].as_str(), Some("./config/paths.json"));
        assert_eq!(extends[1].as_str(), Some("./config/base-url.json"));
        assert_eq!(
            extends[2].as_str(),
            Some(external.canonicalize().unwrap().to_string_lossy().as_ref()),
            "only the external member of an extends array is rewritten"
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&leaf);
        assert_eq!(
            paths["@merged/*"][0],
            shadow.join("later-base/feature/*").to_string_lossy(),
            "later-parent baseUrl must re-anchor an earlier-parent paths table, matching esbuild"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialise_shadow_typescript_configs_preserves_internal_symlink_extends_spelling() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let shadow = tmp.path().join("shadow");
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::write(
            root.join("config/base.json"),
            r#"{"compilerOptions":{"paths":{"@linked/*":["src/*"]}}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink("config/base.json", root.join("config-link.json")).unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"./config-link.json"}"#,
        )
        .unwrap();
        let source = root.join("src/entry.ts");
        std::fs::write(&source, "export {};").unwrap();

        let configs = collect_islands_shadow_configs(&root, [&source]).unwrap();
        materialise_shadow_typescript_configs(&root, &shadow, &configs).unwrap();

        assert_eq!(
            std::fs::read_to_string(shadow.join("tsconfig.json")).unwrap(),
            r#"{"extends":"./config-link.json"}"#
        );
        let mirrored_link = shadow.join("config-link.json");
        assert!(mirrored_link.is_file());
        assert!(
            !std::fs::symlink_metadata(&mirrored_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the authored symlink name must be materialized as a safe shadow-local file"
        );
        let paths =
            zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&shadow.join("tsconfig.json"));
        assert_eq!(
            paths["@linked/*"][0],
            shadow.join("src/*").to_string_lossy()
        );
    }

    #[test]
    fn materialise_shadow_typescript_configs_rebases_project_ancestor_and_blocks_ambient_config() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let project = workspace.join("apps/site");
        let ambient = tmp.path().join("ambient");
        let shadow = ambient.join("stage");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::write(
            workspace.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"./apps/site/src","paths":{"@ancestor/*":["*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            ambient.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@ambient/*":["wrong/*"]}}}"#,
        )
        .unwrap();

        materialise_shadow_typescript_configs(
            &project,
            &shadow,
            &std::collections::BTreeSet::new(),
        )
        .unwrap();

        let boundary = shadow.join("tsconfig.json");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&boundary).unwrap()).unwrap();
        assert_eq!(
            json["extends"].as_str(),
            Some(
                workspace
                    .join("tsconfig.json")
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert_eq!(
            json["compilerOptions"]["baseUrl"].as_str(),
            Some(shadow.join("src").to_string_lossy().as_ref()),
            "an ancestor baseUrl pointing into the project must be rebased into the shadow"
        );
        assert_eq!(
            json["compilerOptions"]["paths"]["@ancestor/*"][0].as_str(),
            Some(shadow.join("src/*").to_string_lossy().as_ref())
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&boundary);
        assert!(paths.contains_key("@ancestor/*"), "{paths:?}");
        assert!(
            !paths.contains_key("@ambient/*"),
            "the explicit shadow-root boundary must hide unrelated tempdir ancestor configs"
        );
    }

    #[test]
    fn materialise_shadow_typescript_configs_resolves_hoisted_package_extends() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let project = workspace.join("apps/site");
        let package = workspace.join("node_modules/@scope/tsconfig-web");
        let shadow = tmp.path().join("shadow");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@scope/tsconfig-web","tsconfig":"base.json"}"#,
        )
        .unwrap();
        let package_config = package.join("base.json");
        std::fs::write(
            &package_config,
            r#"{"compilerOptions":{"baseUrl":"../../../apps/site","paths":{"@hoisted/*":["src/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            project.join("tsconfig.json"),
            r#"{"extends":"@scope/tsconfig-web"}"#,
        )
        .unwrap();
        let source = project.join("src/entry.ts");
        std::fs::write(&source, "export {};").unwrap();

        let configs = collect_islands_shadow_configs(&project, [&source]).unwrap();
        materialise_shadow_typescript_configs(&project, &shadow, &configs).unwrap();

        let leaf = shadow.join("tsconfig.json");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&leaf).unwrap()).unwrap();
        assert_eq!(
            json["extends"].as_str(),
            Some(
                package_config
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            ),
            "a hoisted package config must not depend on tempdir ancestor node_modules"
        );
        assert_eq!(
            json["compilerOptions"]["baseUrl"].as_str(),
            Some(shadow.to_string_lossy().as_ref())
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&leaf);
        assert_eq!(
            paths["@hoisted/*"][0],
            shadow.join("src/*").to_string_lossy()
        );
    }

    #[test]
    fn materialise_shadow_typescript_configs_excludes_nested_workspace_node_modules_config() {
        // Issue #2300: production runs `collect_islands_shadow_configs` /
        // `materialise_shadow_typescript_configs` against the WIDENED
        // workspace root, so a package config under a nested project's
        // `node_modules` (e.g. `apps/site/node_modules/@scope/pkg/...`) has
        // `apps` as its first path component, not `node_modules` — the old
        // `.next()`-only check in `internal_shadow_config_path` never
        // excluded it. This is the un-migrated twin of the `usable_rel` fix
        // (issues #2051/#2128/#2146) — see the comment on
        // `internal_shadow_config_path` for why this is a conscious,
        // recorded migration to `zfb_types::has_node_modules_segment`.
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let project = workspace.join("apps/site");
        let package = project.join("node_modules/@scope/tsconfig-web");
        let shadow = tmp.path().join("shadow");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@scope/tsconfig-web","tsconfig":"base.json"}"#,
        )
        .unwrap();
        let package_config = package.join("base.json");
        std::fs::write(
            &package_config,
            r#"{"compilerOptions":{"paths":{"@nested/*":["src/*"]}}}"#,
        )
        .unwrap();
        let project_config = project.join("tsconfig.json");
        std::fs::write(&project_config, r#"{"extends":"@scope/tsconfig-web"}"#).unwrap();
        let source = project.join("src/entry.ts");
        std::fs::write(&source, "export {};").unwrap();

        // Widened root, matching production (root cause confirmed in #2300).
        let configs = collect_islands_shadow_configs(&workspace, [&source]).unwrap();

        assert!(
            configs.contains(&project_config),
            "the project's own leaf config must still be collected: {configs:?}"
        );
        assert!(
            !configs.contains(&package_config),
            "a config reached through a node_modules component anywhere in its \
             root-relative path must be excluded, not just when node_modules \
             is the first component: {configs:?}"
        );

        // Copy-mode guard: `shadow_config_scope_uses_paths` walks the leaf's
        // own extends chain via `read_tsconfig_paths_file_into_map`, so
        // `paths` inherited from the excluded package config must stay
        // visible even though the package config itself is gone from the
        // collected set — the gatekeeper excludes shadow MIRRORING, not
        // esbuild-visible path resolution.
        assert!(
            shadow_config_scope_uses_paths(&workspace, &configs),
            "paths inherited through the excluded nested package config's \
             extends chain must still be detected"
        );

        materialise_shadow_typescript_configs(&workspace, &shadow, &configs).unwrap();
        assert!(
            !shadow.join("apps/site/node_modules").exists(),
            "materialise must not create a real directory shadowing where the \
             project node_modules symlink needs to land"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialise_islands_shadow_mirrors_config_through_symlinked_project_root() {
        let tmp = tempdir().unwrap();
        let physical_root = tmp.path().join("physical-project");
        let linked_root = tmp.path().join("linked-project");
        std::fs::create_dir_all(physical_root.join("components")).unwrap();
        std::os::unix::fs::symlink(&physical_root, &linked_root).unwrap();
        std::fs::write(
            linked_root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["components/*"]}}}"#,
        )
        .unwrap();
        let island = linked_root.join("components/Island.tsx");
        let worker = linked_root.join("components/worker.ts");
        std::fs::write(
            &island,
            "'use client'; export function Island() { new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }); return null; }\n",
        )
        .unwrap();
        std::fs::write(&worker, "self.postMessage('ready');\n").unwrap();

        let (islands, scan_meta) =
            scan_islands_with_meta(std::slice::from_ref(&island), &FsResolver::new()).unwrap();
        let shadow = match materialise_islands_shadow(&linked_root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("symlink-root worker must materialise: {offenders:?}")
            }
        };
        assert!(shadow._tempdir.path().join("tsconfig.json").is_file());
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(
            &shadow._tempdir.path().join("tsconfig.json"),
        );
        assert!(
            paths["@/*"][0].starts_with(shadow._tempdir.path().to_string_lossy().as_ref()),
            "canonical scanner paths must still select the linked root's shadow config: {paths:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preprocessing_importers_dedup_logical_and_canonical_project_paths() {
        let tmp = tempdir().unwrap();
        let physical_root = tmp.path().join("physical");
        let linked_root = tmp.path().join("linked");
        std::fs::create_dir_all(physical_root.join("src")).unwrap();
        std::fs::write(physical_root.join("src/importer.ts"), "export {};\n").unwrap();
        std::os::unix::fs::symlink(&physical_root, &linked_root).unwrap();

        let logical = linked_root.join("src/importer.ts");
        let canonical = logical.canonicalize().unwrap();
        let paths = IslandsShadowPaths::new(&linked_root);
        let deduped = dedup_shadow_paths(&paths, [logical, canonical]);
        assert_eq!(
            deduped.len(),
            1,
            "one importer reached through logical and canonical spellings must be rewritten once"
        );
    }

    #[test]
    #[ignore = "env-gate: esbuild binary — cargo test -p zfb --lib \
                commands::build::tests::preprocessing_shadows_bundle_nested_alias_raw_and_workers_with_real_esbuild \
                -- --ignored (ZFB_ESBUILD_BIN or staged workspace slot)"]
    fn preprocessing_shadows_bundle_nested_alias_raw_and_workers_with_real_esbuild() {
        let Some(_esbuild) = zfb_test_utils::locate_esbuild() else {
            panic!(
                "preprocessing shadow regression requires a pinned real esbuild binary; set \
                 ZFB_ESBUILD_BIN or stage crates/zfb/binaries/esbuild/esbuild"
            );
        };

        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for dir in [
            "pages/client",
            "components/feature",
            "config",
            "src/wrong",
            "src/workers",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(
            root.join("config/tsconfig.base.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": "..",
                "paths": {
                  "@feature/*": ["src/*"],
                  "@worker/*": ["src/workers/*"]
                }
              }
            }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"./config/tsconfig.base.json"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("components/feature/tsconfig.json"),
            r#"{"extends":"../../tsconfig.json"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("components/feature/jsconfig.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": "../..",
                "paths": { "@feature/*": ["src/wrong/*"] }
              }
            }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pages/client/jsconfig.json"),
            r#"{"extends":"../../tsconfig.json"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/tsconfig.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": ".",
                "paths": { "@worker/*": ["workers/*"] }
              }
            }"#,
        )
        .unwrap();

        std::fs::write(
            root.join("pages/index.tsx"),
            "import { ShadowIsland } from '../components/feature/ShadowIsland';\n\
             export default ShadowIsland;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("components/feature/ShadowIsland.tsx"),
            "'use client';\n\
             import { makeMarker } from '@feature/island-helper';\n\
             import { pluginMarker } from 'plugin:shadow-marker';\n\
             import { virtualMarker } from 'virtual:shadow-marker';\n\
             export function ShadowIsland() {\n\
               return makeMarker() + pluginMarker + virtualMarker;\n\
             }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/island-helper.ts"),
            "import payload from './island-payload.txt?raw';\n\
             export function makeMarker() {\n\
               new Worker(new URL('./primary.worker.ts', import.meta.url), { type: 'module' });\n\
               return payload;\n\
             }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/wrong/island-helper.ts"),
            "export function makeMarker() { return 'ZFB_JSCONFIG_WRONG_PRECEDENCE'; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/island-payload.txt"),
            "ZFB_SHADOW_ALIAS_RAW_MARKER",
        )
        .unwrap();
        std::fs::write(
            root.join("src/primary.worker.ts"),
            "import { nestedMarker } from '@worker/nested-helper';\n\
             import { pluginMarker } from 'plugin:shadow-marker';\n\
             import { virtualMarker } from 'virtual:shadow-marker';\n\
             self.postMessage('ZFB_PRIMARY_WORKER:' + nestedMarker + pluginMarker + virtualMarker);\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/workers/nested-helper.ts"),
            "import nestedPayload from './nested-payload.txt?raw';\n\
             new Worker(new URL('./nested.worker.ts', import.meta.url), { type: 'module' });\n\
             export const nestedMarker = 'ZFB_NESTED_HELPER:' + nestedPayload;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/workers/nested-payload.txt"),
            "ZFB_NESTED_RAW_MARKER",
        )
        .unwrap();
        std::fs::write(
            root.join("src/workers/nested.worker.ts"),
            "self.postMessage('ZFB_NESTED_WORKER');\n",
        )
        .unwrap();
        std::fs::write(
            root.join("pages/client/widget.client.ts"),
            "import { makeMarker } from '@feature/island-helper';\n\
             import { pluginMarker } from 'plugin:shadow-marker';\n\
             import { virtualMarker } from 'virtual:shadow-marker';\n\
             console.log('ZFB_CLIENT_ENTRY:' + makeMarker() + pluginMarker + virtualMarker);\n",
        )
        .unwrap();
        let plugin_target = root.join("plugin-shadow-marker.ts");
        std::fs::write(
            &plugin_target,
            "import pluginPayload from './plugin-shadow-payload.txt?raw';\n\
             new Worker(new URL('./plugin-shadow.worker.ts', import.meta.url), { type: 'module' });\n\
             export const pluginMarker = 'ZFB_PLUGIN_ALIAS_MARKER:' + pluginPayload;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("plugin-shadow-payload.txt"),
            "ZFB_PLUGIN_ALIAS_RAW_MARKER",
        )
        .unwrap();
        std::fs::write(
            root.join("plugin-shadow.worker.ts"),
            "self.postMessage('ZFB_PLUGIN_ALIAS_WORKER_MARKER');\n",
        )
        .unwrap();

        let primary_worker = root.join("src/primary.worker.ts");
        let nested_worker = root.join("src/workers/nested.worker.ts");
        let plugin_worker = root.join("plugin-shadow.worker.ts");
        let primary_filename = zfb_types::module_worker_filename(root, &primary_worker).unwrap();
        let nested_filename = zfb_types::module_worker_filename(root, &nested_worker).unwrap();
        let plugin_worker_filename =
            zfb_types::module_worker_filename(root, &plugin_worker).unwrap();
        let plugin_config = IslandsPluginConfig {
            alias_entries: vec![(
                "plugin:shadow-marker".to_string(),
                plugin_target.to_string_lossy().into_owned(),
            )],
            virtual_modules: vec![(
                "virtual:shadow-marker".to_string(),
                "export const virtualMarker = 'ZFB_PLUGIN_VIRTUAL_MARKER';\n".to_string(),
            )],
        };
        let outdir = root.join("dist");
        let (islands_payload, names) = build_default_islands_payload_with_bundle_options(
            root,
            &root.join("pages"),
            &[],
            &outdir,
            crate::config::Framework::Preact,
            None,
            zfb_islands::BundleMode::Development,
            &plugin_config,
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect("command-layer islands preprocessing shadow must bundle");
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["ShadowIsland".to_string()])
        );
        let islands_payload = islands_payload.expect("islands payload");
        let islands_js = String::from_utf8(islands_payload.bytes).unwrap();
        for marker in [
            "ZFB_SHADOW_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_ALIAS_MARKER",
            "ZFB_PLUGIN_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_VIRTUAL_MARKER",
        ] {
            assert!(
                islands_js.contains(marker),
                "main islands entry/plugin/NODE_PATH path lost {marker}: {islands_js}"
            );
        }
        assert!(!islands_js.contains("ZFB_JSCONFIG_WRONG_PRECEDENCE"));
        assert!(islands_js.contains(&primary_filename), "{islands_js}");
        assert!(islands_js.contains(&plugin_worker_filename), "{islands_js}");
        let island_companions = islands_payload
            .companions
            .into_iter()
            .map(|companion| (companion.filename, companion.bytes))
            .collect::<std::collections::BTreeMap<_, _>>();
        let primary_js = String::from_utf8(island_companions[&primary_filename].clone()).unwrap();
        assert!(primary_js.contains("ZFB_NESTED_RAW_MARKER"), "{primary_js}");
        assert!(primary_js.contains(&nested_filename), "{primary_js}");
        assert!(
            primary_js.contains("ZFB_PLUGIN_ALIAS_RAW_MARKER"),
            "{primary_js}"
        );
        assert!(primary_js.contains(&plugin_worker_filename), "{primary_js}");
        assert!(
            String::from_utf8(island_companions[&nested_filename].clone())
                .unwrap()
                .contains("ZFB_NESTED_WORKER")
        );
        assert!(
            String::from_utf8(island_companions[&plugin_worker_filename].clone())
                .unwrap()
                .contains("ZFB_PLUGIN_ALIAS_WORKER_MARKER")
        );

        let client_payloads = build_default_client_scripts_payloads_with_plugin_config(
            root,
            &outdir,
            crate::config::Framework::Preact,
            &zfb_build::ClientEntryList::new(),
            None,
            &plugin_config,
        )
        .expect("command-layer client preprocessing shadow must bundle");
        let widget = client_payloads
            .into_iter()
            .find(|payload| payload.relative_path.ends_with("widget.js"))
            .expect("widget client payload");
        let widget_js = String::from_utf8(widget.bytes).unwrap();
        for marker in [
            "ZFB_SHADOW_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_ALIAS_MARKER",
            "ZFB_PLUGIN_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_VIRTUAL_MARKER",
        ] {
            assert!(
                widget_js.contains(marker),
                "production client entry lost {marker}: {widget_js}"
            );
        }
        assert!(widget_js.contains(&primary_filename), "{widget_js}");
        let client_companions = widget
            .companions
            .into_iter()
            .map(|companion| (companion.filename, companion.bytes))
            .collect::<std::collections::BTreeMap<_, _>>();
        let client_primary_js =
            String::from_utf8(client_companions[&primary_filename].clone()).unwrap();
        for marker in [
            "ZFB_NESTED_RAW_MARKER",
            "ZFB_PLUGIN_ALIAS_MARKER",
            "ZFB_PLUGIN_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_VIRTUAL_MARKER",
        ] {
            assert!(
                client_primary_js.contains(marker),
                "production client worker lost {marker}: {client_primary_js}"
            );
        }
        assert!(client_companions.contains_key(&nested_filename));
        assert!(client_companions.contains_key(&plugin_worker_filename));

        let dev_assets_root = root.join("dev-assets");
        let dev_outcome = build_dev_client_scripts_to_disk_with_plugin_config(
            root,
            &dev_assets_root,
            crate::config::Framework::Preact,
            None,
            &std::collections::HashSet::new(),
            &zfb_build::ClientEntryList::new(),
            &plugin_config,
        )
        .expect("dev client preprocessing shadow must bundle with plugins");
        let dev_changed = dev_outcome.changed;
        let dev_outputs = dev_outcome.output_filenames;
        let dev_raw_targets = dev_outcome.raw_targets;
        let dev_worker_targets = dev_outcome.worker_targets;
        assert!(dev_changed);
        assert!(dev_outputs.contains("widget.js"));
        assert!(dev_outputs.contains(&primary_filename));
        assert!(dev_outputs.contains(&nested_filename));
        assert!(dev_outputs.contains(&plugin_worker_filename));
        assert!(dev_raw_targets.contains(&root.join("src/island-payload.txt")));
        assert!(dev_raw_targets.contains(&root.join("src/workers/nested-payload.txt")));
        assert!(dev_raw_targets.contains(&root.join("plugin-shadow-payload.txt")));
        assert!(dev_worker_targets.contains(&primary_worker));
        assert!(dev_worker_targets.contains(&nested_worker));
        assert!(dev_worker_targets.contains(&plugin_worker));

        let dev_client_dir = dev_assets_root
            .join(zfb_types::DIST_ASSETS_DIR)
            .join(zfb_types::DIST_CLIENT_SCRIPTS_DIR);
        let dev_widget_js = std::fs::read_to_string(dev_client_dir.join("widget.js")).unwrap();
        for marker in [
            "ZFB_SHADOW_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_ALIAS_MARKER",
            "ZFB_PLUGIN_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_VIRTUAL_MARKER",
        ] {
            assert!(
                dev_widget_js.contains(marker),
                "dev client entry lost {marker}: {dev_widget_js}"
            );
        }
        assert!(dev_widget_js.contains(&primary_filename), "{dev_widget_js}");

        let dev_primary_js =
            std::fs::read_to_string(dev_client_dir.join(&primary_filename)).unwrap();
        for marker in [
            "ZFB_NESTED_RAW_MARKER",
            "ZFB_PLUGIN_ALIAS_MARKER",
            "ZFB_PLUGIN_ALIAS_RAW_MARKER",
            "ZFB_PLUGIN_VIRTUAL_MARKER",
        ] {
            assert!(
                dev_primary_js.contains(marker),
                "dev client worker lost {marker}: {dev_primary_js}"
            );
        }
        assert!(
            dev_primary_js.contains(&nested_filename),
            "{dev_primary_js}"
        );
        assert!(dev_client_dir.join(&nested_filename).is_file());
        assert!(dev_client_dir.join(&plugin_worker_filename).is_file());
    }

    /// Issue #1987 (Wave 5, epic #1982) — the islands/client half of #1984's
    /// red test, now GREEN.
    ///
    /// When `project_root` is itself claimed by `pnpm-workspace.yaml`
    /// (`packages: ['.', 'packages/*']`), `first_party_root_for(project_root)`
    /// maps it to itself (see
    /// `zfb_types::first_party::project_root_that_is_the_workspace_root_maps_to_itself`),
    /// so the old widened-stage proxy read that as "not a workspace" and
    /// `stage_escape_audit_policy` returned `None` — the islands/client
    /// metafile stage-escape audit never armed (issue #1984 proved this
    /// exact defect). Wave 5 swaps that proxy for
    /// `zfb_types::stage_escape_audit_eligibility` (issue #1986), which reads
    /// the reachable `node_modules/@scope/child -> packages/child` link as
    /// eligible regardless of widening.
    ///
    /// The child package here declares neither `exports` nor `main` — an
    /// UNDECLARED entry, deliberately distinct from #2040's "consume from
    /// source" carve-out (a declared `exports`/`main` entry root, e.g. a
    /// bare `main: "index.js"` declaring the package root itself, is
    /// legitimate and stays accepted; see
    /// `bundler_consume_from_source_esbuild_regression.rs`). A "use client"
    /// island reaches it through a plain `require(...)` call — an edge shape
    /// guard (a)'s static `import`/`export` scanner (`collect_import_edges`,
    /// `zfb-islands/src/scanner.rs`) never records as a
    /// `WorkspacePackageImportEdge` — resolving through the symlink straight
    /// to the LIVE, unmirrored child source, esbuild's case-2 shape with no
    /// declared entry root. Now that guard (b) (the metafile stage-escape
    /// audit) is armed here, it is exactly this scanner-missed edge's
    /// backstop: the build must now FAIL with a stage-escape error instead
    /// of silently succeeding.
    #[cfg(unix)]
    #[test]
    #[ignore = "env-gate: esbuild binary — cargo test -p zfb --lib \
                commands::build::tests::root_workspace_islands_stage_escape_audit_is_now_armed_and_undeclared_child_escape_is_rejected \
                -- --ignored (ZFB_ESBUILD_BIN or staged workspace slot)"]
    fn root_workspace_islands_stage_escape_audit_is_now_armed_and_undeclared_child_escape_is_rejected(
    ) {
        if zfb_test_utils::locate_esbuild().is_none() {
            panic!(
                "root-workspace islands stage-escape regression requires a pinned real esbuild \
                 binary; set ZFB_ESBUILD_BIN or stage crates/zfb/binaries/esbuild/esbuild"
            );
        }

        let tmp = tempdir().unwrap();
        let root = tmp.path();

        // `project_root` IS the workspace root: both `.` and `packages/*` are
        // explicitly claimed, matching #1730's repro shape.
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'packages/*'\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"host","private":true}"#,
        )
        .unwrap();

        // A first-party CHILD package under `packages/*`, undeclared
        // (neither `exports` nor `main`) — the same "no staged spelling was
        // ever declared as an entry root" shape #2040's negative test uses
        // for its deep-import case — reached only through a `node_modules`
        // symlink the way a real pnpm install produces one.
        let child_pkg = root.join("packages/child");
        std::fs::create_dir_all(&child_pkg).unwrap();
        std::fs::write(
            child_pkg.join("package.json"),
            r#"{"name":"@scope/child","private":true}"#,
        )
        .unwrap();
        std::fs::write(
            child_pkg.join("index.js"),
            "module.exports.childMarker = 'CHILD_PACKAGE_ESCAPE_MARKER';\n",
        )
        .unwrap();

        // Minimal islands runtime deps — the synthesized islands entry
        // always imports these (mirrors `stage_minimal_node_modules` in
        // `crates/zfb-islands/tests/integration.rs`).
        let nm = root.join("node_modules");
        let zfb_runtime = nm.join("@takazudo/zfb");
        std::fs::create_dir_all(&zfb_runtime).unwrap();
        std::fs::write(
            zfb_runtime.join("package.json"),
            r#"{"name":"@takazudo/zfb","version":"0.0.0","exports":{"./runtime":"./runtime.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            zfb_runtime.join("runtime.js"),
            "export function mountIslands() {}\n",
        )
        .unwrap();
        let preact = nm.join("preact");
        std::fs::create_dir_all(&preact).unwrap();
        std::fs::write(
            preact.join("package.json"),
            r#"{"name":"preact","version":"10.0.0","main":"index.js"}"#,
        )
        .unwrap();
        std::fs::write(
            preact.join("index.js"),
            "export function h() {}\nexport function hydrate() {}\nexport function render() {}\n",
        )
        .unwrap();
        // The genuine pnpm-style symlink into the first-party child package.
        let scope_dir = nm.join("@scope");
        std::fs::create_dir_all(&scope_dir).unwrap();
        std::os::unix::fs::symlink(&child_pkg, scope_dir.join("child")).unwrap();

        // Evidence #1 — the eligibility predicate is now armed at the root.
        // `first_party_root_for` still maps a root-claimed project_root to
        // itself (the widened-stage proxy alone would still read this as
        // "not a workspace"), but the reachable `node_modules/@scope/child`
        // link now arms `stage_escape_audit_policy` via
        // `zfb_types::stage_escape_audit_eligibility`'s row 3. `nm` stands in
        // for the wholesale `<stage>/node_modules` symlink every real call
        // site sets up (both point at the same live tree, and the predicate
        // canonicalises before scanning).
        let first_party_root = zfb_types::first_party_root_for(root);
        assert_eq!(
            first_party_root,
            zfb_types::normalize_path_lexical(root),
            "a root-claimed workspace member must map first_party_root to itself"
        );
        assert!(
            stage_escape_audit_policy(root, &first_party_root, root).is_some(),
            "a reachable first-party node_modules link at a root-claimed workspace must now \
             arm the islands/client stage-escape audit"
        );

        // The unrecorded edge: a plain `require(...)` call, which
        // `collect_import_edges` (guard (a)'s scanner) never visits — it
        // only collects `import`/`export ... from`/`export *`/string-literal
        // dynamic `import()`.
        //
        // A harmless `?raw` import sits alongside it. `_islands_shadow`
        // (this function's staged copy of the island-reachable closure) is
        // only materialised when the scan finds a glob/raw/worker/plugin
        // preprocessing need (see the `if !scan_meta...is_empty() ...`
        // gate a few lines above `materialise_islands_shadow_with_worker_context`
        // in this file) — a plain `require()`-only island never triggers
        // staging at all, which would make node_modules resolution merely
        // ordinary (no stage to escape from), not a proof of anything. The
        // raw import forces that staged shadow to exist, WITH its own
        // wholesale `node_modules` symlink (see
        // `materialise_islands_shadow_with_worker_context`'s
        // `first_party_node_modules` wiring), so the `require()` edge is
        // proven to escape a REAL, active stage.
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("components")).unwrap();
        std::fs::write(
            root.join("pages/index.tsx"),
            "import { ChildIsland } from '../components/ChildIsland';\n\
             export default ChildIsland;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("components/payload.txt"),
            "HARMLESS_RAW_PAYLOAD_MARKER",
        )
        .unwrap();
        std::fs::write(
            root.join("components/ChildIsland.tsx"),
            "'use client';\n\
             import payload from './payload.txt?raw';\n\
             const { childMarker } = require('@scope/child');\n\
             export function ChildIsland() { return childMarker + ':' + payload; }\n",
        )
        .unwrap();

        let outdir = root.join("dist");
        let plugin_config = IslandsPluginConfig::default();
        let error = build_default_islands_payload_with_bundle_options(
            root,
            &root.join("pages"),
            &[],
            &outdir,
            crate::config::Framework::Preact,
            None,
            zfb_islands::BundleMode::Production,
            &plugin_config,
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect_err(
            "the require()-reached child package resolves through node_modules to LIVE, \
             unmirrored, UNDECLARED source — now that the islands/client audit is armed here, \
             this must be rejected as a stage escape instead of silently succeeding",
        );
        let message = format!("{error:#}");
        assert!(
            message.contains("stage-escape audit"),
            "expected a stage-escape audit rejection, got: {message}"
        );
        assert!(
            message.contains("@scope/child"),
            "the offending package import must name the escaping child package: {message}"
        );
    }

    /// Issue #2083 (epic #2078, Wave 1) — the RED test for #2048's silent
    /// islands/client shape, **flipped by issue #2090** (Wave 4, Sub 10c).
    ///
    /// # DELIBERATE, DOCUMENTED ASSERTION REWRITE (#2090)
    ///
    /// As authored by #2083 this test asserted the DESIRED outcome of a full
    /// acceptance⇒enrolment coupling: the sibling's `import.meta.glob`
    /// EXPANDS, and the emitted bundle embeds the matched file's content
    /// (`GLOB_SIBLING_DATA_MARKER`). #2090 invoked epic #2078's **sanctioned
    /// loud-failure fallback** instead, so — exactly as #2083's own header
    /// anticipated and authorised — the two assertions below were REWRITTEN
    /// to the loud-failure form: the build now FAILS with a diagnostic naming
    /// the package and the unexpanded macro. This is a deliberate, documented
    /// downgrade recorded in #2090's PR body and an epic-issue comment, NOT a
    /// silent weakening. The reason the coupling was not taken: acceptance is
    /// a property of esbuild's metafile, so coupling here would need a SECOND
    /// esbuild pass plus a curated `node_modules` layer to redirect the bare
    /// specifier away from the shadow's wholesale live-`node_modules` symlink
    /// — new machinery of exactly the kind the epic's stop condition reserves
    /// the fallback for. See `audit_unenrolled_first_party_macro_leak`.
    ///
    /// What the fallback does NOT change: the sibling is still ACCEPTED (it
    /// is a legitimate declared consume-from-source first-party package, not
    /// a stage escape), and it is still not mirror-enrolled. What changes is
    /// only that the resulting leak is now loud instead of silent.
    ///
    /// **Scope note:** the fix this test guards targets NESTED workspace
    /// members only. `SiblingMirrorPlan::compute`'s early-return
    /// (`crates/zfb-build/src/bundler.rs` :5167) and the
    /// `mirror_sibling_root` gate (:3009) put root-claimed topology
    /// (`first_party_root == project_root`) explicitly out of scope,
    /// matching #2048's own scoping — this fixture is deliberately a NESTED
    /// member (`apps/demo`, under a `pnpm-workspace.yaml` claiming `apps/*` +
    /// `packages/*`, never `.`).
    ///
    /// ## Why this is a command-layer test, not a `zfb-build`-only one
    ///
    /// #2048's interaction is real for the SSR pipeline too (a bare-package
    /// consume-from-source sibling is never mirrored, so its own macro ships
    /// literally there as well), but the SSR shape is not what this sub
    /// guards — epic #2078's Wave 4 restructure notes an SSR-executed path
    /// that reaches an unexpanded macro throws at RENDER time (loud-ish),
    /// while the islands/client pipeline ships the literal macro to the
    /// browser with a fully green BUILD and no render ever happens
    /// build-side. Driving `build_default_islands_payload_with_bundle_options`
    /// (the same command-layer entry point `zfb build` itself calls for the
    /// production islands bundle) is what actually reaches that silent
    /// no-stage islands/client path.
    ///
    /// ## The repro
    ///
    /// `@acme/glob-sibling` is a first-party, consume-from-source sibling
    /// package (#2040's declared-entry exemption: its `package.json`
    /// `exports` map declares `./glob-source` as an entry root pointing
    /// straight at un-built `./index.ts`, no `dist`). `index.ts` calls
    /// `import.meta.glob('./data/*.json')` over its own sibling-local
    /// `data/` directory.
    ///
    /// The host island (`GlobWidget.tsx`) reaches the sibling via
    /// `require('@acme/glob-sibling/glob-source')` — deliberately CommonJS,
    /// not `import`, because `collect_import_edges` (the islands scanner's
    /// edge collector) only records `import` / `export ... from` / `export
    /// *` / string-literal dynamic `import()` edges; a query-free
    /// `require(...)` call produces NO edge at all (see
    /// `find_unsupported_query_call`, which only flags QUERY-BEARING
    /// require/dynamic-import calls — mirrors the same technique the
    /// root-workspace regression test above uses to bypass guard (a)'s
    /// scanner). The sibling's `index.ts` is therefore NEVER visited by
    /// `scan_islands_with_meta`'s DFS: its `import.meta.glob` call stays
    /// invisible to `glob_by_path`, so `scan_meta.glob_reachable_from_islands`
    /// stays empty. With no OTHER raw/worker/glob signal anywhere in this
    /// fixture, `build_default_islands_payload_with_bundle_options`'s
    /// shadow-staging precondition (~build.rs:3732,
    /// `!scan_meta.glob_reachable_from_islands.is_empty() || …`) is never
    /// met, `_islands_shadow` stays `None`, and BOTH guard (a) (the
    /// workspace-package-edge check inside
    /// `materialise_islands_shadow_with_worker_context`, ~build.rs:2874 —
    /// never even called) and guard (b) (`stage_escape_audit_policy`, only
    /// ever armed `if let Some(islands_stage_root) = _islands_shadow…`) are
    /// skipped entirely. esbuild bundles straight from `project_root`,
    /// resolves the `require()` call through the REAL
    /// `node_modules/@acme/glob-sibling` symlink to LIVE, unprocessed
    /// source, and the literal, unexpanded `import.meta.glob(...)` call text
    /// ships in the production islands bundle with a GREEN build — exactly
    /// #2048's silent-wrongness window.
    ///
    /// Post-#2090 behavior (what the assertions below now pin): the leak is
    /// caught after the bundle is produced, by scanning the browser-bound
    /// bytes for the macro and then attributing it — through Sub #2088's
    /// declared-data query — to the claimed workspace member whose DECLARED
    /// entry source carries it. `zfb build` (`IslandsGlobPolicy::HardError`,
    /// what this test drives) fails with that diagnostic.
    ///
    /// This test is the proof that the fallback fires on the **NO-STAGE**
    /// path specifically: as traced above, this fixture materialises no
    /// shadow, so neither guard (a) nor guard (b) runs and there is no
    /// `--metafile` anywhere. A check bolted onto the existing stage-escape
    /// audit call site could not possibly fire here.
    ///
    /// ### Flip protocol
    ///
    /// This test needs a staged real esbuild binary (env-gate), so #2090
    /// REPLACED the `pending-feature` tag below with the normal
    /// `#[ignore = "env-gate: esbuild — …"]` tag — never a bare delete —
    /// per epic #2078's corrected flip protocol, and dropped the matching
    /// `--skip` from `health.yml`'s `commands::build:: -- --ignored` step in
    /// the same commit so the flipped test genuinely runs in CI again.
    #[cfg(unix)]
    #[test]
    #[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --lib commands::build::tests::bare_package_consume_from_source_sibling_glob_macro_reaches_islands_bundle_unexpanded -- --ignored"]
    fn bare_package_consume_from_source_sibling_glob_macro_reaches_islands_bundle_unexpanded() {
        if zfb_test_utils::locate_esbuild().is_none() {
            panic!(
                "bare-package consume-from-source glob-sibling regression requires a pinned \
                 real esbuild binary; set ZFB_ESBUILD_BIN or stage \
                 crates/zfb/binaries/esbuild/esbuild"
            );
        }

        let tmp = tempdir().unwrap();
        let root = tmp.path();

        // Nested-member workspace: `.` is deliberately NOT claimed (unlike
        // the root-claimed fixture above) — #2048's fix scope explicitly
        // excludes the root-claimed topology.
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"workspace-root","private":true}"#,
        )
        .unwrap();

        // The consume-from-source sibling (#2040's declared-entry
        // exemption): its own `package.json` declares `./glob-source` as an
        // entry root pointing straight at un-built `.ts` source, and it
        // carries its own `import.meta.glob` over sibling-local files.
        let sibling = root.join("packages/glob-sibling");
        std::fs::create_dir_all(sibling.join("data")).unwrap();
        std::fs::write(
            sibling.join("package.json"),
            r#"{"name":"@acme/glob-sibling","exports":{"./glob-source":"./index.ts"}}"#,
        )
        .unwrap();
        std::fs::write(
            sibling.join("index.ts"),
            "export const modules = import.meta.glob('./data/*.json');\n",
        )
        .unwrap();
        std::fs::write(
            sibling.join("data/entry.json"),
            r#"{"value":"GLOB_SIBLING_DATA_MARKER"}"#,
        )
        .unwrap();

        // Minimal islands runtime deps — the synthesized islands entry
        // always imports these (mirrors `stage_minimal_node_modules` in
        // `crates/zfb-islands/tests/integration.rs` and the root-workspace
        // regression test above). Hoisted at the WORKSPACE ROOT
        // `node_modules`, matching real pnpm-workspace hoisting for a
        // nested member.
        let nm = root.join("node_modules");
        let zfb_runtime = nm.join("@takazudo/zfb");
        std::fs::create_dir_all(&zfb_runtime).unwrap();
        std::fs::write(
            zfb_runtime.join("package.json"),
            r#"{"name":"@takazudo/zfb","version":"0.0.0","exports":{"./runtime":"./runtime.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            zfb_runtime.join("runtime.js"),
            "export function mountIslands() {}\n",
        )
        .unwrap();
        let preact = nm.join("preact");
        std::fs::create_dir_all(&preact).unwrap();
        std::fs::write(
            preact.join("package.json"),
            r#"{"name":"preact","version":"10.0.0","main":"index.js"}"#,
        )
        .unwrap();
        std::fs::write(
            preact.join("index.js"),
            "export function h() {}\nexport function hydrate() {}\nexport function render() {}\n",
        )
        .unwrap();
        // The genuine pnpm-style symlink into the first-party
        // consume-from-source sibling package.
        let scope_dir = nm.join("@acme");
        std::fs::create_dir_all(&scope_dir).unwrap();
        std::os::unix::fs::symlink(&sibling, scope_dir.join("glob-sibling")).unwrap();

        // The nested host member.
        let project = root.join("apps/demo");
        std::fs::create_dir_all(project.join("pages")).unwrap();
        std::fs::create_dir_all(project.join("components")).unwrap();
        std::fs::write(
            project.join("package.json"),
            r#"{"name":"demo","private":true}"#,
        )
        .unwrap();
        std::fs::write(
            project.join("pages/index.tsx"),
            "import { GlobWidget } from '../components/GlobWidget';\n\
             export default GlobWidget;\n",
        )
        .unwrap();
        // The unrecorded edge: a plain, query-free `require(...)` call,
        // which `collect_import_edges` (guard (a)'s scanner) never visits —
        // see this test's header comment. Deliberately NO `?raw` / worker /
        // direct-`import` glob edge sits alongside it: this fixture must
        // trigger NO OTHER preprocessing signal anywhere, so the
        // shadow-staging precondition is never met and the no-shadow fast
        // path is taken.
        std::fs::write(
            project.join("components/GlobWidget.tsx"),
            "'use client';\n\
             const sibling = require('@acme/glob-sibling/glob-source');\n\
             export function GlobWidget() { return sibling.modules ? 'HAS_MODULES' : 'NO_MODULES'; }\n",
        )
        .unwrap();

        let outdir = project.join("dist");
        let plugin_config = IslandsPluginConfig::default();
        let error = build_default_islands_payload_with_bundle_options(
            &project,
            &project.join("pages"),
            &[],
            &outdir,
            crate::config::Framework::Preact,
            None,
            zfb_islands::BundleMode::Production,
            &plugin_config,
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect_err(
            "the require()-reached consume-from-source sibling is an ACCEPTED case-2 input \
             (declared entry root, claimed by pnpm-workspace.yaml) that is never mirror-enrolled, \
             so its own import.meta.glob reaches the browser bundle unexpanded — under #2090's \
             sanctioned loud-failure fallback that must now FAIL the build loudly instead of \
             shipping silently",
        );
        let message = format!("{error:#}");

        assert!(
            message.contains("import.meta.glob("),
            "the diagnostic must name the unexpanded macro that leaked: {message}"
        );
        assert!(
            message.contains("@acme/glob-sibling"),
            "the diagnostic must name the consume-from-source package the macro came from: \
             {message}"
        );
    }

    #[test]
    fn client_virtual_module_preprocessing_syntax_is_an_explicit_command_error() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages/client")).unwrap();
        std::fs::write(
            root.join("pages/client/widget.client.ts"),
            "import value from 'virtual:raw-widget'; console.log(value);\n",
        )
        .unwrap();
        let plugin_config = IslandsPluginConfig {
            alias_entries: Vec::new(),
            virtual_modules: vec![(
                "virtual:raw-widget".to_string(),
                "import value from './payload.txt?raw'; export default value;".to_string(),
            )],
        };

        let error = build_default_client_scripts_payloads_with_plugin_config(
            root,
            &root.join("dist"),
            crate::config::Framework::Preact,
            &zfb_build::ClientEntryList::new(),
            None,
            &plugin_config,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(
                "query-bearing import \"./payload.txt?raw\" inside plugin virtual module \"virtual:raw-widget\" is unsupported"
            ),
            "{message}"
        );
        assert!(
            message.contains("virtual sources cannot be rewritten into the preprocessing shadow"),
            "{message}"
        );
    }

    #[test]
    #[ignore = "env-gate: esbuild binary — plugin alias is the only preprocessing edge"]
    fn plugin_alias_only_client_preprocessing_triggers_shadow_with_real_esbuild() {
        let Some(_esbuild) = zfb_test_utils::locate_esbuild() else {
            panic!("plugin-alias preprocessing regression requires a pinned real esbuild binary");
        };
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages/client")).unwrap();
        std::fs::write(
            root.join("pages/client/alias-only.client.ts"),
            "import { value } from 'plugin:only-preprocess'; console.log(value);\n",
        )
        .unwrap();
        let alias = root.join("plugin-only.ts");
        let worker = root.join("plugin-only.worker.ts");
        std::fs::write(
            &alias,
            "import payload from './plugin-only.txt?raw';\n\
             new Worker(new URL('./plugin-only.worker.ts', import.meta.url), { type: 'module' });\n\
             export const value = 'ZFB_PLUGIN_ONLY_ENTRY:' + payload;\n",
        )
        .unwrap();
        std::fs::write(root.join("plugin-only.txt"), "ZFB_PLUGIN_ONLY_RAW").unwrap();
        std::fs::write(&worker, "self.postMessage('ZFB_PLUGIN_ONLY_WORKER');\n").unwrap();
        let worker_filename = zfb_types::module_worker_filename(root, &worker).unwrap();
        let plugin_config = IslandsPluginConfig {
            alias_entries: vec![(
                "plugin:only-preprocess".to_string(),
                alias.to_string_lossy().into_owned(),
            )],
            virtual_modules: Vec::new(),
        };

        let payloads = build_default_client_scripts_payloads_with_plugin_config(
            root,
            &root.join("dist"),
            crate::config::Framework::Preact,
            &zfb_build::ClientEntryList::new(),
            None,
            &plugin_config,
        )
        .expect("plugin-only preprocessing graph must trigger a client shadow");
        let payload = payloads
            .into_iter()
            .find(|payload| payload.relative_path.ends_with("alias-only.js"))
            .expect("alias-only client payload");
        let js = String::from_utf8(payload.bytes).unwrap();
        assert!(js.contains("ZFB_PLUGIN_ONLY_RAW"), "{js}");
        assert!(js.contains(&worker_filename), "{js}");
        let worker_js = payload
            .companions
            .into_iter()
            .find(|companion| companion.filename == worker_filename)
            .map(|companion| String::from_utf8(companion.bytes).unwrap())
            .expect("plugin-only worker companion");
        assert!(worker_js.contains("ZFB_PLUGIN_ONLY_WORKER"), "{worker_js}");
    }

    #[test]
    fn ancestor_only_worker_config_stays_in_client_and_islands_closures() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let external_config = tmp.path().join("tsconfig.json");
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("components")).unwrap();
        std::fs::write(
            &external_config,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        )
        .unwrap();

        let client_entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &client_entry,
            "new Worker(new URL('../src/client.worker.ts', import.meta.url), { type: 'module' });\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/client.worker.ts"),
            "self.postMessage('client');\n",
        )
        .unwrap();
        let client_stage = stage_client_script_preprocessing_with_worker_context(
            &root,
            &[zfb_islands::client_scripts::ClientScriptEntry {
                entry_name: "widget".to_string(),
                source_path: client_entry,
            }],
            &zfb_build::ModuleWorkerBuildContext::default(),
        )
        .unwrap()
        .expect("client worker requires preprocessing");
        assert!(client_stage.worker_targets.contains(&external_config));

        let page = root.join("pages/index.tsx");
        let island = root.join("components/Island.tsx");
        std::fs::write(
            &page,
            "import { Island } from '../components/Island'; export default Island;\n",
        )
        .unwrap();
        std::fs::write(
            &island,
            "'use client'; export function Island() { new Worker(new URL('./island.worker.ts', import.meta.url), { type: 'module' }); return null; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("components/island.worker.ts"),
            "self.postMessage('island');\n",
        )
        .unwrap();
        let (islands, scan_meta) = scan_islands_with_meta(&[page], &FsResolver::new()).unwrap();
        let shadow = match materialise_islands_shadow(&root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("islands worker config closure must materialise: {offenders:?}")
            }
        };
        assert!(
            shadow.module_worker_dependencies.contains(&external_config),
            "external config parent must remain in the islands invalidation closure"
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
    fn materialise_islands_shadow_tracks_aliased_raw_import_for_dev_invalidation() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::create_dir_all(project_root.join("components")).unwrap();
        std::fs::create_dir_all(project_root.join("src/content")).unwrap();
        std::fs::write(
            project_root.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": ".",
                "paths": { "@/*": ["src/*"] }
              }
            }"#,
        )
        .unwrap();

        let page = project_root.join("pages/index.tsx");
        let island_src = project_root.join("components/Shader.tsx");
        let raw_target = project_root.join("src/content/shader.txt");
        std::fs::write(
            &page,
            "import { Shader } from '../components/Shader';\nexport default Shader;\n",
        )
        .unwrap();
        std::fs::write(
            &island_src,
            "\"use client\";\nimport source from '@/content/shader.txt?raw';\n\
             export function Shader() { return source; }\n",
        )
        .unwrap();
        std::fs::write(&raw_target, "aliased shader payload\n").unwrap();

        let resolver = FsResolver::new().with_project_root(project_root);
        let (islands, scan_meta) = scan_islands_with_meta(&[page], &resolver).unwrap();
        assert_eq!(scan_meta.raw_import_edges_from_islands.len(), 1);
        assert_eq!(
            scan_meta.raw_import_edges_from_islands[0].importer,
            island_src.canonicalize().unwrap()
        );
        assert_eq!(
            scan_meta.raw_import_edges_from_islands[0].target,
            raw_target
        );

        let shadow = match materialise_islands_shadow(project_root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("aliased terminal raw target must materialise: {offenders:?}")
            }
        };
        assert_eq!(
            shadow.raw_targets,
            std::collections::BTreeSet::from([raw_target.clone()])
        );

        let invalidation = zfb_build::RawImportInvalidation::default();
        invalidation.replace_islands(shadow.raw_targets.clone());
        assert!(
            invalidation.is_islands_target(&raw_target),
            "aliased raw target must remain in dev invalidation inputs"
        );
    }

    #[test]
    fn materialise_islands_shadow_rejects_node_modules_raw_importer_from_package_route() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let package = root.join("node_modules/@scope/preset/dist");
        let routes = package.join("routes");
        std::fs::create_dir_all(&routes).unwrap();
        let route = routes.join("_chrome.tsx");
        let island = package.join("PresetIsland.tsx");
        let raw_target = package.join("payload.txt");
        std::fs::write(
            &route,
            "import { PresetIsland } from '../PresetIsland';\nexport default PresetIsland;\n",
        )
        .unwrap();
        std::fs::write(
            &island,
            "\"use client\";\nimport payload from './payload.txt?raw';\n\
             export function PresetIsland() { return payload; }\n",
        )
        .unwrap();
        std::fs::write(&raw_target, "package raw payload\n").unwrap();

        let resolver = FsResolver::new()
            .with_project_root(root)
            .with_injected_route_roots([&route]);
        let (islands, scan_meta) =
            scan_islands_with_meta(std::slice::from_ref(&route), &resolver).unwrap();
        assert_eq!(scan_meta.raw_import_edges_from_islands.len(), 1);
        assert_eq!(
            scan_meta.raw_import_edges_from_islands[0].target,
            raw_target.canonicalize().unwrap()
        );

        let outcome = materialise_islands_shadow(root, &islands, &scan_meta).unwrap();
        let IslandsShadowOutcome::KeepStopgap(offenders) = outcome else {
            panic!("node_modules raw importers must stay outside materialisation scope")
        };
        let message = offenders.join("\n");
        assert!(message.contains("node_modules"), "{message}");
        assert!(
            message.contains("outside the mirrorable project tree"),
            "{message}"
        );
    }

    #[test]
    fn materialise_islands_shadow_rewrites_nested_worker_urls_without_importing_entries() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages/workers")).unwrap();
        std::fs::create_dir_all(root.join("components")).unwrap();
        std::fs::create_dir_all(root.join("lib/workers")).unwrap();
        let page = root.join("pages/index.tsx");
        let island = root.join("components/Island.tsx");
        // Keep the constructor helper and most of the worker graph outside
        // the default `components/` / `src/` islands roots. These exact
        // logical paths must be retained for live invalidation.
        let helper = root.join("lib/start-worker.ts");
        let worker = root.join("pages/workers/search.ts");
        let nested = root.join("lib/workers/tokenize.ts");
        let worker_payload = root.join("lib/search.txt");
        let nested_payload = root.join("lib/workers/tokenize.txt");
        let worker_css = root.join("lib/worker.css");
        std::fs::write(
            &page,
            "import { Island } from '../components/Island'; export default Island;\n",
        )
        .unwrap();
        std::fs::write(
            &island,
            "'use client'; import { start } from '../lib/start-worker'; export function Island() { start(); return null; }\n",
        )
        .unwrap();
        std::fs::write(
            &helper,
            "export const start = () => new Worker(new URL('../pages/workers/search.ts', import.meta.url), { type: 'module' });\n",
        )
        .unwrap();
        std::fs::write(
            &worker,
            "import '../../lib/worker.css'; import text from '../../lib/search.txt?raw'; new Worker(new URL('../../lib/workers/tokenize.ts', import.meta.url), { type: 'module' }); self.postMessage(text);\n",
        )
        .unwrap();
        std::fs::write(
            &nested,
            "import text from './tokenize.txt?raw'; self.postMessage(text);\n",
        )
        .unwrap();
        std::fs::write(&worker_payload, "search payload").unwrap();
        std::fs::write(&nested_payload, "tokenize payload").unwrap();
        std::fs::write(&worker_css, ".worker { color: rebeccapurple; }").unwrap();

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
            std::fs::read_to_string(shadow_root.join("lib/start-worker.ts")).unwrap();
        assert!(
            rewritten_helper.contains("new URL(\"./worker-pages-s-workers-s-search-d-ts.js?v="),
            "{rewritten_helper}"
        );
        assert!(rewritten_helper.contains(".js?v="), "{rewritten_helper}");
        assert!(
            !rewritten_helper.contains("import '../pages/workers/search.ts'"),
            "worker entry must not become an SSR/islands import: {rewritten_helper}"
        );
        let rewritten_worker =
            std::fs::read_to_string(shadow_root.join("pages/workers/search.ts")).unwrap();
        assert!(
            rewritten_worker.contains("new URL(\"./worker-lib-s-workers-s-tokenize-d-ts.js?v="),
            "{rewritten_worker}"
        );
        assert!(!rewritten_worker.contains("?raw"), "{rewritten_worker}");
        let rewritten_nested =
            std::fs::read_to_string(shadow_root.join("lib/workers/tokenize.ts")).unwrap();
        assert!(!rewritten_nested.contains("?raw"), "{rewritten_nested}");
        assert!(
            shadow_root.join("lib/workers/tokenize.ts").exists(),
            "nested worker entry is mirrored for the later emission pass"
        );
        assert_eq!(
            shadow.raw_targets,
            std::collections::BTreeSet::from([worker_payload.clone(), nested_payload.clone()])
        );
        let expected_worker_closure = std::collections::BTreeSet::from([
            helper,
            worker,
            nested,
            worker_payload,
            nested_payload,
            worker_css,
        ]);
        assert!(
            expected_worker_closure.is_subset(&shadow.module_worker_dependencies),
            "full worker closure must stay available to dev invalidation: {:?}",
            shadow.module_worker_dependencies
        );
        for config_candidate in [root.join("tsconfig.json"), root.join("jsconfig.json")] {
            assert!(
                shadow
                    .module_worker_dependencies
                    .contains(&config_candidate),
                "absent nearest-config candidates must remain observable: {}",
                config_candidate.display()
            );
        }
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
    fn client_script_stage_uses_copy_mode_with_nested_jsconfig_paths() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("pages/jsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"..","paths":{"@/*":["src/*"]}}}"#,
        )
        .unwrap();
        let entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &entry,
            "import text from '../src/message.txt?raw'; console.log(text);\n",
        )
        .unwrap();
        std::fs::write(root.join("src/message.txt"), "copy mode").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("raw graph needs a stage");
        assert!(
            !stage.preserve_symlinks,
            "nested jsconfig paths plus project node_modules must select copy mode"
        );
        let metadata = std::fs::symlink_metadata(&stage.entries[0].source_path).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    }

    #[test]
    fn client_script_stage_rewrites_external_relative_config_extends() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let shared_name = format!(
            "{}-client-external-config",
            tmp.path().file_name().unwrap().to_string_lossy()
        );
        let shared = tmp.path().join(&shared_name);
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(shared.join("src")).unwrap();
        let external = shared.join("tsconfig.base.json");
        std::fs::write(
            &external,
            r#"{"compilerOptions":{"baseUrl":"../project","paths":{"@project/*":["src/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            format!(r#"{{"extends":"../{shared_name}/tsconfig.base.json"}}"#),
        )
        .unwrap();
        let entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &entry,
            "import text from '../src/message.txt?raw'; console.log(text);\n",
        )
        .unwrap();
        std::fs::write(root.join("src/message.txt"), "external config stage").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let stage = stage_client_script_preprocessing(&root, &entries)
            .unwrap()
            .expect("raw client graph needs a stage");
        let staged_config = stage.root.join("tsconfig.json");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&staged_config).unwrap()).unwrap();
        assert_eq!(
            json["extends"].as_str(),
            Some(external.canonicalize().unwrap().to_string_lossy().as_ref())
        );
        assert_eq!(
            json["compilerOptions"]["baseUrl"].as_str(),
            Some(stage.root.to_string_lossy().as_ref()),
            "an external parent baseUrl that points into the project must be rebased"
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&staged_config);
        assert_eq!(
            paths["@project/*"][0],
            stage.root.join("src/*").to_string_lossy(),
            "external-parent aliases back into the project must stay inside the stage"
        );
        assert!(
            !stage.root.parent().unwrap().join(&shared_name).exists(),
            "client staging must not write through the authored `..` outside its temp root"
        );
    }

    #[test]
    fn client_script_stage_rewrites_transitive_module_config_extends() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let shared = tmp.path().join("shared");
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src/feature")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        let external = shared.join("tsconfig.base.json");
        std::fs::write(
            &external,
            r#"{"compilerOptions":{"baseUrl":"../project/src/feature","paths":{"@feature/*":["*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/feature/tsconfig.json"),
            r#"{"extends":"../../../shared/tsconfig.base.json"}"#,
        )
        .unwrap();
        let entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &entry,
            "import { message } from '../src/feature/helper'; console.log(message);\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/feature/helper.ts"),
            "import text from './message.txt?raw'; export const message = text;\n",
        )
        .unwrap();
        std::fs::write(root.join("src/feature/message.txt"), "transitive config").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let stage = stage_client_script_preprocessing(&root, &entries)
            .unwrap()
            .expect("transitive raw client graph needs a stage");
        let nested = stage.root.join("src/feature/tsconfig.json");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&nested).unwrap()).unwrap();
        assert_eq!(
            json["extends"].as_str(),
            Some(external.canonicalize().unwrap().to_string_lossy().as_ref()),
            "every transitive module config must be rewritten, not only entry/worker configs"
        );
        assert_eq!(
            json["compilerOptions"]["baseUrl"].as_str(),
            Some(stage.root.join("src/feature").to_string_lossy().as_ref())
        );
        let paths = zfb_plugin_resolver::read_tsconfig_paths_file_into_map(&nested);
        assert_eq!(
            paths["@feature/*"][0],
            stage.root.join("src/feature/*").to_string_lossy()
        );
    }

    #[test]
    fn client_script_stage_rebases_standalone_root_and_nested_configs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("project");
        let external = tmp.path().join("external");
        for dir in [
            root.join("pages"),
            root.join("src/feature"),
            root.join("src/root-local"),
            root.join("src/nested-local"),
            external.join("root-lib"),
            external.join("nested-lib"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let absolute_external = external
            .join("../external/absolute/*")
            .to_string_lossy()
            .into_owned();
        write_standalone_shadow_config_fixture(
            &root.join("jsconfig.json"),
            "../external",
            "root-client",
            "../project/src/root-local/*",
            "root-lib/*",
            &absolute_external,
        );
        write_standalone_shadow_config_fixture(
            &root.join("src/feature/tsconfig.json"),
            "../../../external",
            "nested-client",
            "../project/src/nested-local/*",
            "nested-lib/*",
            &absolute_external,
        );

        let root_entry = root.join("pages/root.client.ts");
        let nested_entry = root.join("src/feature/nested.client.ts");
        std::fs::write(
            &root_entry,
            "import raw from './root.txt?raw'; console.log(raw);\n",
        )
        .unwrap();
        std::fs::write(root.join("pages/root.txt"), "root client raw").unwrap();
        std::fs::write(
            &nested_entry,
            "import raw from './nested.txt?raw'; console.log(raw);\n",
        )
        .unwrap();
        std::fs::write(root.join("src/feature/nested.txt"), "nested client raw").unwrap();
        let entries = vec![
            zfb_islands::client_scripts::ClientScriptEntry {
                entry_name: "root".into(),
                source_path: root_entry,
            },
            zfb_islands::client_scripts::ClientScriptEntry {
                entry_name: "nested".into(),
                source_path: nested_entry,
            },
        ];

        let stage = stage_client_script_preprocessing(&root, &entries)
            .unwrap()
            .expect("standalone-config raw client entries need a stage");
        assert_standalone_shadow_config_rebased(
            &stage.root.join("jsconfig.json"),
            &external,
            "root-client",
            &stage.root.join("src/root-local/*"),
            &external.join("root-lib/*"),
            &absolute_external,
        );
        assert_standalone_shadow_config_rebased(
            &stage.root.join("src/feature/tsconfig.json"),
            &external,
            "nested-client",
            &stage.root.join("src/nested-local/*"),
            &external.join("nested-lib/*"),
            &absolute_external,
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
        assert!(stage.worker_targets.contains(&helper));
        assert!(stage.worker_targets.contains(&worker));
        assert!(stage.worker_targets.contains(&nested_worker));
        assert!(stage.worker_targets.contains(&worker_helper));
        assert!(stage.worker_targets.contains(&payload));
    }

    #[derive(Default)]
    struct RecordingDevClientScriptWriter {
        writes: Vec<PathBuf>,
    }

    impl DevClientScriptAtomicWriter for RecordingDevClientScriptWriter {
        fn atomic_write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
            self.writes.push(path.to_path_buf());
            zfb_build::atomic_write(path, bytes)
        }
    }

    /// Aggregate-review regression: the old dev loop published each stable
    /// entry immediately, so a later entry's bundle error left the earlier
    /// served bytes from a generation the caller subsequently aborted.
    #[test]
    fn dev_client_later_entry_bundle_failure_keeps_all_served_bytes_unchanged() {
        let tmp = tempdir().unwrap();
        let client_dir = tmp.path().join("assets/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let alpha_path = client_dir.join("alpha.js");
        std::fs::write(&alpha_path, b"ALPHA_PREVIOUS").unwrap();

        let entries = vec![
            ClientScriptEntry {
                entry_name: "alpha".into(),
                source_path: PathBuf::from("pages/alpha.client.ts"),
            },
            ClientScriptEntry {
                entry_name: "beta".into(),
                source_path: PathBuf::from("pages/beta.client.ts"),
            },
        ];
        let workers = BTreeMap::new();
        let previous = std::collections::HashSet::from(["alpha.js".to_string()]);
        let mut writer = RecordingDevClientScriptWriter::default();

        let error = bundle_and_commit_dev_client_script_generation(
            &client_dir,
            &entries,
            &workers,
            &previous,
            &mut writer,
            |entry, _| match entry.entry_name.as_str() {
                "alpha" => Ok(ClientScriptBundleOutput {
                    js: "import './worker-alpha.js';\nALPHA_NEXT".into(),
                    companions: vec![zfb_islands::BundleChunk {
                        filename: "worker-alpha.js".into(),
                        bytes: b"WORKER_ALPHA_NEXT".to_vec(),
                    }],
                }),
                "beta" => Err(anyhow!("injected later-entry bundle failure")),
                other => panic!("unexpected entry {other}"),
            },
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("injected later-entry bundle failure"),
            "{error:#}"
        );
        assert!(
            writer.writes.is_empty(),
            "the complete generation must bundle before its first write"
        );
        assert_eq!(std::fs::read(alpha_path).unwrap(), b"ALPHA_PREVIOUS");
        assert!(
            !client_dir.join("worker-alpha.js").exists(),
            "a companion prepared for an aborted generation must never appear"
        );
    }

    struct FailOnceAfterAtomicWrite {
        fail_on_publish_write: usize,
        publish_writes: usize,
        failed: bool,
        published_paths: Vec<PathBuf>,
    }

    impl DevClientScriptAtomicWriter for FailOnceAfterAtomicWrite {
        fn atomic_write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
            zfb_build::atomic_write(path, bytes)?;
            if self.failed {
                // Rollback uses the same production-shaped atomic writer. The
                // injected durability failure fires once, not process-wide.
                return Ok(());
            }
            self.publish_writes += 1;
            self.published_paths.push(path.to_path_buf());
            if self.publish_writes == self.fail_on_publish_write {
                self.failed = true;
                return Err(anyhow!("injected post-replacement durability failure"));
            }
            Ok(())
        }
    }

    /// Exercise the harder failure shape: the late writer has already
    /// replaced/created its destination before reporting an error. Rollback
    /// must restore old bytes and remove every new candidate, including that
    /// failing path itself.
    #[test]
    fn dev_client_late_commit_failure_rolls_back_complete_generation() {
        let tmp = tempdir().unwrap();
        let client_dir = tmp.path().join("assets/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(client_dir.join("alpha.js"), b"ALPHA_PREVIOUS").unwrap();
        std::fs::write(client_dir.join("worker-a-existing.js"), b"WORKER_PREVIOUS").unwrap();

        let entries = vec![
            ClientScriptEntry {
                entry_name: "alpha".into(),
                source_path: PathBuf::from("pages/alpha.client.ts"),
            },
            ClientScriptEntry {
                entry_name: "beta".into(),
                source_path: PathBuf::from("pages/beta.client.ts"),
            },
        ];
        let workers = BTreeMap::new();
        let previous = std::collections::HashSet::from([
            "alpha.js".to_string(),
            "worker-a-existing.js".to_string(),
        ]);
        let mut writer = FailOnceAfterAtomicWrite {
            fail_on_publish_write: 4,
            publish_writes: 0,
            failed: false,
            published_paths: Vec::new(),
        };

        let error = bundle_and_commit_dev_client_script_generation(
            &client_dir,
            &entries,
            &workers,
            &previous,
            &mut writer,
            |entry, _| {
                Ok(match entry.entry_name.as_str() {
                    "alpha" => ClientScriptBundleOutput {
                        js: "import './worker-a-existing.js';\nimport './worker-z-new.js';\nALPHA_NEXT"
                            .into(),
                        companions: vec![
                            zfb_islands::BundleChunk {
                                filename: "worker-z-new.js".into(),
                                bytes: b"WORKER_NEW".to_vec(),
                            },
                            zfb_islands::BundleChunk {
                                filename: "worker-a-existing.js".into(),
                                bytes: b"WORKER_NEXT".to_vec(),
                            },
                        ],
                    },
                    "beta" => ClientScriptBundleOutput {
                        js: "BETA_NEXT".into(),
                        companions: Vec::new(),
                    },
                    other => panic!("unexpected entry {other}"),
                })
            },
        )
        .unwrap_err();

        let error_text = format!("{error:#}");
        assert!(
            error_text.contains("injected post-replacement durability failure"),
            "{error_text}"
        );
        assert!(
            error_text.contains("restored previous generation"),
            "{error_text}"
        );
        assert_eq!(
            writer
                .published_paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "worker-a-existing.js",
                "worker-z-new.js",
                "alpha.js",
                "beta.js",
            ],
            "every companion must commit before the first stable entry"
        );
        assert_eq!(
            std::fs::read(client_dir.join("worker-a-existing.js")).unwrap(),
            b"WORKER_PREVIOUS"
        );
        assert_eq!(
            std::fs::read(client_dir.join("alpha.js")).unwrap(),
            b"ALPHA_PREVIOUS"
        );
        assert!(!client_dir.join("worker-z-new.js").exists());
        assert!(!client_dir.join("beta.js").exists());
    }

    /// A companion failure is earlier than the stable-entry publication
    /// barrier. Even when that companion was replaced before the error was
    /// reported, the rollback must complete without attempting the entry.
    #[test]
    fn dev_client_companion_commit_failure_never_touches_stable_entry() {
        let tmp = tempdir().unwrap();
        let client_dir = tmp.path().join("assets/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(client_dir.join("alpha.js"), b"ALPHA_PREVIOUS").unwrap();
        std::fs::write(client_dir.join("worker-a-existing.js"), b"WORKER_PREVIOUS").unwrap();

        let entries = vec![ClientScriptEntry {
            entry_name: "alpha".into(),
            source_path: PathBuf::from("pages/alpha.client.ts"),
        }];
        let mut writer = FailOnceAfterAtomicWrite {
            fail_on_publish_write: 2,
            publish_writes: 0,
            failed: false,
            published_paths: Vec::new(),
        };

        let error = bundle_and_commit_dev_client_script_generation(
            &client_dir,
            &entries,
            &BTreeMap::new(),
            &std::collections::HashSet::from([
                "alpha.js".to_string(),
                "worker-a-existing.js".to_string(),
            ]),
            &mut writer,
            |_entry, _| {
                Ok(ClientScriptBundleOutput {
                    js: "import './worker-a-existing.js';\nimport './worker-z-new.js';\nALPHA_NEXT"
                        .into(),
                    companions: vec![
                        zfb_islands::BundleChunk {
                            filename: "worker-z-new.js".into(),
                            bytes: b"WORKER_NEW".to_vec(),
                        },
                        zfb_islands::BundleChunk {
                            filename: "worker-a-existing.js".into(),
                            bytes: b"WORKER_NEXT".to_vec(),
                        },
                    ],
                })
            },
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("restored previous generation"),
            "{error:#}"
        );
        assert_eq!(
            writer
                .published_paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["worker-a-existing.js", "worker-z-new.js"],
            "the companion error must stop before stable-entry publication"
        );
        assert_eq!(
            std::fs::read(client_dir.join("worker-a-existing.js")).unwrap(),
            b"WORKER_PREVIOUS"
        );
        assert!(!client_dir.join("worker-z-new.js").exists());
        assert_eq!(
            std::fs::read(client_dir.join("alpha.js")).unwrap(),
            b"ALPHA_PREVIOUS"
        );
    }

    struct FailPublicationAndRollbackWriter {
        calls: usize,
    }

    impl DevClientScriptAtomicWriter for FailPublicationAndRollbackWriter {
        fn atomic_write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
            self.calls += 1;
            match self.calls {
                1 => {
                    zfb_build::atomic_write(path, bytes)?;
                    Err(anyhow!("injected publication durability failure"))
                }
                2 => Err(anyhow!("injected rollback write failure")),
                call => panic!("unexpected writer call {call}"),
            }
        }
    }

    /// A failed compensating write leaves disk unsafe for the old readiness
    /// state. Surface a typed marker so the outer document transaction can
    /// conservatively leave publication pending instead of claiming rollback.
    #[test]
    fn dev_client_rollback_failure_returns_distinguishable_marker() {
        let tmp = tempdir().unwrap();
        let client_dir = tmp.path().join("assets/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let alpha_path = client_dir.join("alpha.js");
        std::fs::write(&alpha_path, b"ALPHA_PREVIOUS").unwrap();

        let entries = vec![ClientScriptEntry {
            entry_name: "alpha".into(),
            source_path: PathBuf::from("pages/alpha.client.ts"),
        }];
        let mut writer = FailPublicationAndRollbackWriter { calls: 0 };
        let error = bundle_and_commit_dev_client_script_generation(
            &client_dir,
            &entries,
            &BTreeMap::new(),
            &std::collections::HashSet::from(["alpha.js".to_string()]),
            &mut writer,
            |_entry, _| {
                Ok(ClientScriptBundleOutput {
                    js: "ALPHA_UNSAFE_PARTIAL_GENERATION".into(),
                    companions: Vec::new(),
                })
            },
        )
        .unwrap_err();

        let marker = error
            .downcast_ref::<DevClientScriptRollbackError>()
            .unwrap_or_else(|| {
                panic!("rollback failure must remain downcastable through anyhow: {error:#}")
            });
        assert_eq!(
            marker.uncertain_output_filenames().collect::<Vec<_>>(),
            ["alpha.js"]
        );
        let error_text = format!("{error:#}");
        assert!(
            error_text.contains("injected publication durability failure"),
            "{error_text}"
        );
        assert!(
            error_text.contains("injected rollback write failure"),
            "{error_text}"
        );
        assert_eq!(writer.calls, 2);
        assert_eq!(
            std::fs::read(alpha_path).unwrap(),
            b"ALPHA_UNSAFE_PARTIAL_GENERATION",
            "the marker identifies exactly the case where prior-ready is unsafe"
        );
    }

    #[test]
    fn removed_client_entry_is_retained_for_one_generation_then_pruned() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let entry_source = root.join("pages/widget.client.ts");
        std::fs::write(&entry_source, "console.log('widget');\n").unwrap();
        let assets_root = root.join("dev-assets");
        let registered = zfb_build::ClientEntryList::new();

        let first = build_dev_client_scripts_to_disk(
            root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &std::collections::HashSet::new(),
            &registered,
        )
        .unwrap();
        let client_dir = assets_root
            .join(zfb_types::DIST_ASSETS_DIR)
            .join(zfb_types::DIST_CLIENT_SCRIPTS_DIR);
        let entry_output = client_dir.join("widget.js");
        assert!(first.output_filenames.contains("widget.js"));
        assert!(entry_output.exists());

        std::fs::remove_file(entry_source).unwrap();
        let second = build_dev_client_scripts_to_disk(
            root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &first.output_filenames,
            &registered,
        )
        .unwrap();
        assert!(
            second.output_filenames.is_empty(),
            "publication state reports only the current declared set"
        );
        assert!(
            entry_output.exists(),
            "the removed entry remains servable through the transition generation"
        );

        let third = build_dev_client_scripts_to_disk(
            root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &second.output_filenames,
            &registered,
        )
        .unwrap();
        assert!(
            third.changed,
            "pruning the retained entry is an asset change"
        );
        assert!(
            !entry_output.exists(),
            "the entry is pruned once no served generation can declare it"
        );
    }

    #[test]
    fn client_script_worker_importer_removal_retains_then_prunes_companion() {
        struct PlanOnlyPipeline;

        impl zfb_build::AssetPipeline for PlanOnlyPipeline {
            fn apply(
                &self,
                _plan: &zfb_build::RebuildPlan,
                _ctx: &zfb_build::BuildContext,
            ) -> anyhow::Result<zfb_build::BuildOutcome> {
                unreachable!("the regression exercises planning only")
            }
        }

        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let entry_source = root.join("pages/widget.client.ts");
        let importer = root.join("src/start.ts");
        let worker_source = root.join("src/search.worker.ts");
        std::fs::write(
            &entry_source,
            "import { start } from '../src/start'; start();\n",
        )
        .unwrap();
        std::fs::write(
            &importer,
            "export const start = () => new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });\n",
        )
        .unwrap();
        std::fs::write(&worker_source, "self.postMessage('ready');\n").unwrap();

        let assets_root = root.join("dev-assets");
        let registered = zfb_build::ClientEntryList::new();
        let first_outcome = build_dev_client_scripts_to_disk(
            root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &std::collections::HashSet::new(),
            &registered,
        )
        .unwrap();
        let first_changed = first_outcome.changed;
        let first_outputs = first_outcome.output_filenames;
        let first_raw = first_outcome.raw_targets;
        let first_worker_targets = first_outcome.worker_targets;
        assert!(first_changed);
        assert!(first_raw.is_empty());
        assert!(first_worker_targets.contains(&importer));
        assert!(first_worker_targets.contains(&worker_source));

        let worker_filename = zfb_types::module_worker_filename(root, &worker_source).unwrap();
        let client_dir = assets_root
            .join(zfb_types::DIST_ASSETS_DIR)
            .join(zfb_types::DIST_CLIENT_SCRIPTS_DIR);
        let worker_output = client_dir.join(&worker_filename);
        assert!(first_outputs.contains(&worker_filename));
        assert!(worker_output.exists());

        let invalidation = zfb_build::RawImportInvalidation::default();
        invalidation.replace_client_script_workers(first_worker_targets);
        let policy = zfb_build::GranularityPolicy::default()
            .with_raw_import_invalidation(invalidation.clone());
        let orchestrator = zfb_build::BuildOrchestrator::new(
            zfb_build::OrchestratorConfig::new(
                root,
                vec![PathBuf::from("pages"), PathBuf::from("src")],
            )
            .with_policy(policy),
            std::sync::Arc::new(std::sync::Mutex::new(zfb_graph::DependencyGraph::new())),
            PlanOnlyPipeline,
        );

        // The second watcher tick edits the transitive importer itself and
        // removes the constructor. Planning must still schedule one final
        // client-script run based on the previous successful graph.
        std::fs::write(&importer, "export const start = () => undefined;\n").unwrap();
        let plan = orchestrator.plan_for_changes([importer.clone()]);
        assert!(
            plan.rerun_client_scripts,
            "constructor importer must remain an invalidation target until the next build"
        );

        let second_outcome = build_dev_client_scripts_to_disk(
            root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &first_outputs,
            &registered,
        )
        .unwrap();
        let second_changed = second_outcome.changed;
        let second_outputs = second_outcome.output_filenames;
        let second_raw = second_outcome.raw_targets;
        let second_worker_targets = second_outcome.worker_targets;
        assert!(
            second_changed,
            "removing the constructor changes the current entry bundle bytes"
        );
        assert!(second_raw.is_empty());
        assert!(second_worker_targets.is_empty());
        assert!(!second_outputs.contains(&worker_filename));
        assert!(
            worker_output.exists(),
            "stale worker companion must remain servable for the transition generation"
        );

        // Replacing the registry after the successful second tick prevents
        // the removed edge from triggering client work forever.
        invalidation.replace_client_script_workers(second_worker_targets);
        let settled = orchestrator.plan_for_changes([importer]);
        assert!(!settled.rerun_client_scripts);

        let third_outcome = build_dev_client_scripts_to_disk(
            root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &second_outputs,
            &registered,
        )
        .unwrap();
        assert!(
            third_outcome.changed,
            "pruning the retained worker companion is an asset change"
        );
        assert!(
            !worker_output.exists(),
            "worker companion is pruned after its one-generation retention"
        );
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
        std::fs::write(&entry, "import { url } from './x.txt?url';\n").unwrap();
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

    // ---- Issue #1703, Stage Escape Guards — Guard (a): a bare
    // package-name import of a workspace sibling. --------------------------

    /// Set up `<root>/node_modules/@acme/shared` as a pnpm-workspace-style
    /// symlink into `<root>/workspace/shared` (a real directory outside
    /// `node_modules`), mirroring the fixture
    /// `pnpm_workspace_consumer_fixture_yields_workspace_package_islands`
    /// sets up at install time in `crates/zfb-islands/tests/integration.rs`
    /// — enough for `FsResolver`'s bare-specifier probe to resolve
    /// `@acme/shared` as a genuine workspace package (symlink whose
    /// canonical target carries no `node_modules` path segment).
    #[cfg(unix)]
    fn link_workspace_package(root: &Path) {
        let pkg = root.join("workspace/shared");
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name":"@acme/shared","source":"src/index.ts"}"#,
        )
        .unwrap();
        std::fs::write(pkg.join("src/index.ts"), "export const helper = 1;\n").unwrap();
        let scope_dir = root.join("node_modules/@acme");
        std::fs::create_dir_all(&scope_dir).unwrap();
        std::os::unix::fs::symlink(&pkg, scope_dir.join("shared")).unwrap();
    }

    /// Acceptance criterion (issue #1703): a package-name import of a
    /// workspace sibling with NO `?raw`/module-worker preprocessing active
    /// for the closure keeps its historical behaviour unchanged — the
    /// existing fast path returns `Ok(None)` before Guard (a)'s check ever
    /// runs, so nothing is staged and the plain `node_modules` symlink an
    /// unshadowed build already relies on stays the supported path.
    #[cfg(unix)]
    #[test]
    fn client_script_bare_workspace_package_import_without_raw_stays_supported() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        link_workspace_package(root);
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &entry,
            "import { helper } from '@acme/shared';\nconsole.log(helper);\n",
        )
        .unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        assert!(stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .is_none());
    }

    /// Acceptance criterion (issue #1703): the same package-name import,
    /// but this closure ALSO needs `?raw` preprocessing — so the stage IS
    /// materialised, and the wholesale `node_modules` symlink it creates
    /// would otherwise let `@acme/shared` resolve straight to unprocessed
    /// source, silently bypassing the staged `?raw` rewrite. Guard (a) must
    /// hard-error naming the offending specifier before any stage is
    /// written to disk.
    #[cfg(unix)]
    #[test]
    fn client_script_bare_workspace_package_import_hard_errors_once_raw_staging_is_active() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        link_workspace_package(root);
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &entry,
            "import { helper } from '@acme/shared';\n\
             import text from '../src/message.txt?raw';\n\
             console.log(helper, text);\n",
        )
        .unwrap();
        std::fs::write(root.join("src/message.txt"), "hello").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let error = stage_client_script_preprocessing(root, &entries).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("@acme/shared"), "{message}");
        assert!(
            message.contains("not supported once staging is active"),
            "{message}"
        );
    }

    // ---- Issue #1674: workspace-first-party re-rooting of the client stage. --

    fn write_reroot_ws_file(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    /// A genuinely-CLAIMED pnpm-workspace sub-package host whose client entry
    /// reaches a SIBLING workspace helper (normal import + a sibling `?raw`),
    /// modeled on `crates/zfb-build/tests/module_worker_workspace_first_party_roots.rs`.
    /// Returns `(project_root, entries)`.
    fn write_reroot_workspace_client_fixture(
        workspace: &Path,
    ) -> (PathBuf, Vec<zfb_islands::client_scripts::ClientScriptEntry>) {
        write_reroot_ws_file(
            workspace,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        );
        write_reroot_ws_file(
            workspace,
            "lib/shared/panel.frag",
            "ZFB_REROOT_SIBLING_RAW_PAYLOAD\n",
        );
        write_reroot_ws_file(
            workspace,
            "lib/shared/plain.ts",
            "export const plain = 'ZFB_REROOT_SIBLING_PLAIN';\n",
        );
        write_reroot_ws_file(
            workspace,
            "lib/shared/helper.ts",
            "import { plain } from './plain';\nimport text from './panel.frag?raw';\nexport const shared = plain + text;\n",
        );
        let entry = write_reroot_ws_file(
            workspace,
            "sub-packages/host/pages/widget.client.ts",
            "import { shared } from '../../../lib/shared/helper';\nconsole.log(shared);\n",
        );
        let project_root = workspace.join("sub-packages/host");
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];
        (project_root, entries)
    }

    #[test]
    fn client_script_stage_reroots_at_workspace_and_materialises_sibling_closure_as_copy() {
        // Issue #2163: this test used to assert the staged sibling was a
        // SYMLINK in the no-node_modules, no-tsconfig-paths mode — the
        // fixture (`write_reroot_workspace_client_fixture`) ships neither.
        // The sibling-closure copy-mode disjunct now forces real-file
        // materialisation whenever the closure reaches a workspace sibling
        // at all, regardless of node_modules/tsconfig paths, so the
        // assertion is inverted here (not merely relaxed) — this is the
        // direct counter-assertion to that fix, recorded here and in the PR
        // body per the project's "never game the gate silently" rule.
        let workspace = tempdir().unwrap();
        let (project_root, entries) = write_reroot_workspace_client_fixture(workspace.path());

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .unwrap()
            .expect("a claimed workspace host reaching sibling source needs a stage");

        // The stage roots at the workspace; esbuild's cwd is the mirrored
        // project dir nested under it.
        let expected_working_dir = stage.root.join("sub-packages/host");
        assert_eq!(stage.bundle_working_dir, expected_working_dir);
        assert_ne!(
            stage.bundle_working_dir, stage.root,
            "a workspace-widened stage must mirror the project below the stage root"
        );
        assert!(stage.entries[0]
            .source_path
            .starts_with(&stage.bundle_working_dir));
        assert_eq!(
            stage.entries[0].source_path,
            stage.bundle_working_dir.join("pages/widget.client.ts")
        );

        // A workspace sibling anywhere in the closure now selects copy mode
        // on its own, with no node_modules and no tsconfig `paths` present.
        assert!(
            !stage.preserve_symlinks,
            "a workspace-sibling closure must select copy mode even without node_modules or \
             tsconfig paths"
        );

        // The sibling normal-import module is materialised as a real file at
        // its workspace-relative location, not a symlink back to the live
        // tree.
        let staged_plain = stage.root.join("lib/shared/plain.ts");
        let metadata = std::fs::symlink_metadata(&staged_plain).unwrap();
        assert!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "a sibling normal module must be staged as a real file, not a symlink, once the \
             closure forces copy mode"
        );

        // The sibling `?raw` importer is rewritten in place with a generated
        // raw module beside it — proof the raw expansion crosses the widened
        // boundary instead of erroring "outside the mirrorable project tree".
        let staged_helper = stage.root.join("lib/shared/helper.ts");
        let rewritten = std::fs::read_to_string(&staged_helper).unwrap();
        assert!(!rewritten.contains("?raw"), "{rewritten}");
        assert!(std::fs::read_dir(stage.root.join("lib/shared"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .any(|path| path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))));

        assert!(
            stage
                .raw_targets
                .iter()
                .any(|target| target.ends_with("lib/shared/panel.frag")),
            "the sibling raw target must be tracked for dev invalidation: {:?}",
            stage.raw_targets
        );

        // Issue #1710: the sibling PLAIN module (`plain.ts` — neither a
        // terminal `?raw` target nor a worker dependency) and the sibling
        // `?raw` importer (`helper.ts`) must both land in the new
        // invalidation set, or editing either serves stale dev output until
        // a restart.
        assert!(
            stage
                .client_script_siblings
                .iter()
                .any(|target| target.ends_with("lib/shared/plain.ts")),
            "the sibling plain module must be tracked for dev invalidation: {:?}",
            stage.client_script_siblings
        );
        assert!(
            stage
                .client_script_siblings
                .iter()
                .any(|target| target.ends_with("lib/shared/helper.ts")),
            "the sibling `?raw` importer must be tracked for dev invalidation: {:?}",
            stage.client_script_siblings
        );
    }

    #[test]
    fn build_dev_client_scripts_to_disk_returns_sibling_plain_module_in_outcome() {
        // Issue #1710, build-layer assertion: staging-struct coverage alone
        // doesn't prove the outcome plumbing threads the sibling closure all
        // the way out to the dev caller (`zfb dev`'s watcher wiring). The
        // entry lives under `pages/`, a discovery root, so it is picked up by
        // `discover_client_scripts` without needing a registered entry.
        //
        // Issue #2163: this test used to force copy mode by adding a
        // workspace `node_modules` dir plus a project tsconfig with `paths`
        // (symlink mode stages the sibling as a real symlink back to the
        // live tree, which the embedded, non-system esbuild binary used by
        // this non-ignored test resolves to its real path, tripping the
        // stage-escape audit (#1705) as unstaged). The sibling-closure
        // disjunct now selects copy mode on its own — the workaround is
        // removed, and this is precisely the in-repo e2e workaround the
        // epic set out to make unnecessary.
        let workspace = tempdir().unwrap();
        let (project_root, _entries) = write_reroot_workspace_client_fixture(workspace.path());
        let assets_root = project_root.join("dev-assets");
        let registered = zfb_build::ClientEntryList::new();
        let outcome = build_dev_client_scripts_to_disk(
            &project_root,
            &assets_root,
            crate::config::Framework::Preact,
            None,
            &std::collections::HashSet::new(),
            &registered,
        )
        .expect("dev bundle of the workspace-reroot sibling fixture must succeed");
        assert!(outcome.changed);
        assert!(
            outcome
                .client_script_siblings
                .iter()
                .any(|target| target.ends_with("lib/shared/plain.ts")),
            "the dev outcome must carry the sibling plain module: {:?}",
            outcome.client_script_siblings
        );
    }

    #[test]
    fn client_script_stage_no_op_equivalence_without_workspace_marker() {
        // Without a workspace marker `first_party_root == project_root`, so the
        // stage must NOT re-root: esbuild's cwd equals the stage root and the
        // entry stages at its project-relative path with no mirrored-project
        // prefix — byte-identical to the pre-#1674 single-package behavior.
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_reroot_ws_file(
            root,
            "pages/widget.client.ts",
            "import text from '../src/message.txt?raw';\nconsole.log(text);\n",
        );
        write_reroot_ws_file(root, "src/message.txt", "no-op equivalence\n");
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: root.join("pages/widget.client.ts"),
        }];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("a project-local raw graph still needs a stage");
        assert_eq!(
            stage.bundle_working_dir, stage.root,
            "a non-workspace project must not re-root: cwd stays the stage root"
        );
        assert_eq!(
            stage.entries[0].source_path,
            stage.root.join("pages/widget.client.ts"),
            "the entry must stage at its project-relative path, not under a workspace prefix"
        );
    }

    #[test]
    fn client_script_stage_present_but_unclaimed_workspace_marker_does_not_reroot() {
        // A `pnpm-workspace.yaml` that does NOT claim the project keeps the
        // boundary at the project root (mirrors first_party_root_for's
        // non-membership rule), so the stage must not re-root.
        let workspace = tempdir().unwrap();
        write_reroot_ws_file(
            workspace.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/*'\n",
        );
        let project_root = workspace.path().join("sub-packages/host");
        write_reroot_ws_file(
            &project_root,
            "pages/widget.client.ts",
            "import text from '../src/message.txt?raw';\nconsole.log(text);\n",
        );
        write_reroot_ws_file(&project_root, "src/message.txt", "unclaimed\n");
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: project_root.join("pages/widget.client.ts"),
        }];

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .unwrap()
            .expect("a project-local raw graph still needs a stage");
        assert_eq!(
            stage.bundle_working_dir, stage.root,
            "an unclaimed project must keep its own boundary and not re-root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn client_script_stage_widened_root_contains_symlink_escape() {
        // A symlinked dir INSIDE the project that escapes to a location outside
        // BOTH the project and the workspace must not leak into the stage. The
        // walk's containment bound stays project-scoped under the widened root.
        let outer = tempdir().unwrap();
        let workspace = outer.path().join("workspace");
        let (project_root, entries) = write_reroot_workspace_client_fixture(&workspace);
        let secret_dir = outer.path().join("secret");
        write_reroot_ws_file(
            &secret_dir,
            "leaked.ts",
            "export const leaked = 'SECRET';\n",
        );
        std::os::unix::fs::symlink(&secret_dir, project_root.join("escape")).unwrap();

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .unwrap()
            .expect("the sibling raw graph still needs a stage");
        assert!(
            !stage.bundle_working_dir.join("escape/leaked.ts").exists(),
            "a symlink escaping the project must not be followed into the mirrored project dir"
        );
        assert!(
            !stage.root.join("secret/leaked.ts").exists()
                && !stage.root.join("escape/leaked.ts").exists(),
            "the escaped file must not appear anywhere in the stage"
        );
    }

    #[test]
    fn client_script_stage_widened_root_uses_copy_mode_with_paths_and_node_modules() {
        // Issue #2163: this fixture (`write_reroot_workspace_client_fixture`)
        // ships a genuine workspace sibling, so the sibling-closure disjunct
        // alone now selects copy mode for it — this test no longer isolates
        // "paths + node_modules triggers copy mode" the way its original
        // name claimed (renamed from `..._uses_copy_mode_for_sibling`).
        // Kept as a regression pin for the SECOND disjunct
        // (`has_node_modules && shadow_config_scope_uses_paths(..)`) so that
        // arm stays exercised too, redundantly with the sibling-driven first
        // disjunct on this same fixture; the isolated sibling-only proof
        // (no node_modules, no tsconfig paths) lives in
        // `client_script_stage_reroots_at_workspace_and_materialises_sibling_closure_as_copy`.
        let workspace = tempdir().unwrap();
        let (project_root, entries) = write_reroot_workspace_client_fixture(workspace.path());
        std::fs::create_dir_all(workspace.path().join("node_modules")).unwrap();
        write_reroot_ws_file(
            workspace.path(),
            "sub-packages/host/tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@shared/*":["../../lib/shared/*"]}}}"#,
        );

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .unwrap()
            .expect("the sibling raw graph still needs a stage");
        assert!(
            !stage.preserve_symlinks,
            "workspace node_modules plus tsconfig paths must select copy mode"
        );
        let staged_plain = stage.root.join("lib/shared/plain.ts");
        let metadata = std::fs::symlink_metadata(&staged_plain).unwrap();
        assert!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "copy mode must materialise the sibling module as a real file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn client_script_stage_symlinks_both_workspace_and_project_node_modules() {
        // With an install at BOTH the workspace root and the project dir, the
        // stage links each at its matching location so esbuild's nearest-package
        // walk keeps project-nested precedence over the workspace-hoisted tree.
        let workspace = tempdir().unwrap();
        let (project_root, entries) = write_reroot_workspace_client_fixture(workspace.path());
        write_reroot_ws_file(
            workspace.path(),
            "node_modules/.workspace-marker",
            "workspace hoisted",
        );
        write_reroot_ws_file(
            &project_root,
            "node_modules/.project-marker",
            "project nested",
        );

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .unwrap()
            .expect("the sibling raw graph still needs a stage");

        let workspace_link = stage.root.join("node_modules");
        let project_link = stage.bundle_working_dir.join("node_modules");
        assert!(
            std::fs::symlink_metadata(&workspace_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the workspace-hoisted install must be linked at the stage root"
        );
        assert!(
            std::fs::symlink_metadata(&project_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the project-nested install must be linked at the mirrored project dir"
        );
        assert_eq!(
            std::fs::read_link(&workspace_link).unwrap(),
            workspace.path().join("node_modules")
        );
        assert_eq!(
            std::fs::read_link(&project_link).unwrap(),
            project_root.join("node_modules")
        );
    }

    // ---- Issue #2301: production-path regression combining all three ----
    // #2300 gatekeeper-fix ingredients — a CLAIMED pnpm workspace, the
    // nested project's OWN `node_modules`, and a tsconfig `extends` chain
    // resolving INTO it — through both production shadow-staging entry
    // points. No prior fixture combined all three at once.

    /// Shared workspace topology for issue #2301: a claimed workspace
    /// (`apps/*`) whose nested project (`apps/site`) ships its own
    /// `node_modules` containing a `@tsconfig/strict-base` package, with the
    /// project's own `tsconfig.json` extending it by bare specifier. Returns
    /// the nested project root; callers add their own pipeline-specific
    /// trigger (an island glob, or a client-script `?raw` import) on top.
    ///
    /// This is the un-migrated twin of #2300's own unit-level fixture
    /// (`materialise_shadow_typescript_configs_excludes_nested_workspace_node_modules_config`,
    /// which drives `collect_islands_shadow_configs` /
    /// `materialise_shadow_typescript_configs` directly) — here the SAME
    /// topology is driven through the full production entry points
    /// (`materialise_islands_shadow_with_worker_context` /
    /// `stage_client_script_preprocessing_with_worker_context`), which also
    /// exercise the node_modules wholesale-symlink step that runs right
    /// after config materialisation and is where the pre-fix bug actually
    /// surfaced as an IO error (see the two tests below).
    fn write_nested_node_modules_tsconfig_workspace(workspace: &Path) -> PathBuf {
        write_reroot_ws_file(
            workspace,
            "pnpm-workspace.yaml",
            "packages:\n  - 'apps/*'\n",
        );
        write_reroot_ws_file(
            workspace,
            "node_modules/.workspace-marker",
            "workspace hoisted\n",
        );
        let project_root = workspace.join("apps/site");
        write_reroot_ws_file(
            &project_root,
            "node_modules/@tsconfig/strict-base/tsconfig.json",
            r#"{"compilerOptions":{"strict":true}}"#,
        );
        write_reroot_ws_file(
            &project_root,
            "tsconfig.json",
            r#"{"extends":"@tsconfig/strict-base/tsconfig.json"}"#,
        );
        project_root
    }

    #[cfg(unix)]
    #[test]
    fn stage_client_script_preprocessing_with_worker_context_excludes_nested_workspace_node_modules_tsconfig(
    ) {
        // Pre-fix (before c6cb2ba7 / #2300's migration to
        // `zfb_types::has_node_modules_segment`), the nested package config's
        // root-relative path from the WIDENED workspace root is
        // `apps/site/node_modules/@tsconfig/strict-base/tsconfig.json` —
        // `node_modules` is not the FIRST component, so the old `.next()`-only
        // check in `internal_shadow_config_path` never excluded it. It was
        // wrongly collected and `materialise_shadow_typescript_configs`
        // materialised a REAL `node_modules` dir under the mirrored project
        // dir BEFORE the project-node_modules wholesale-symlink step below it
        // runs — `shadow_symlink`'s `remove_file` cannot remove a directory,
        // so the subsequent `std::os::unix::fs::symlink` call failed with
        // EEXIST ("File exists (os error 17)"). Observed pre-fix by
        // temporarily reverting the `has_node_modules_segment` hunk in this
        // worktree and running this test: it failed with exactly that EEXIST,
        // propagated through the `symlink client preprocess stage project
        // node_modules ... -> ...` context.
        let workspace = tempdir().unwrap();
        let project_root = write_nested_node_modules_tsconfig_workspace(workspace.path());
        write_reroot_ws_file(
            &project_root,
            "pages/widget.client.ts",
            "import text from '../src/message.txt?raw';\nconsole.log(text);\n",
        );
        write_reroot_ws_file(
            &project_root,
            "src/message.txt",
            "nested node_modules tsconfig fixture\n",
        );
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: project_root.join("pages/widget.client.ts"),
        }];

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .expect(
                "stage materialisation must not IO-error — a wrongly-collected nested package \
                 tsconfig would materialise a real node_modules dir before the project \
                 node_modules symlink step runs, which then fails with EEXIST",
            )
            .expect("a project-local raw graph still needs a stage");

        assert!(
            std::fs::symlink_metadata(stage.root.join("node_modules"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the workspace-hoisted node_modules must be linked wholesale at the stage root"
        );
        let project_node_modules_link = stage.bundle_working_dir.join("node_modules");
        assert!(
            std::fs::symlink_metadata(&project_node_modules_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the project's own nested node_modules must be linked wholesale, not shadowed by \
             a real dir the config-collection pass created first"
        );

        // The project's own leaf tsconfig IS materialised as a real file —
        // the gatekeeper excludes the NESTED PACKAGE config from shadow
        // mirroring, not the project's own leaf.
        let staged_leaf = stage.bundle_working_dir.join("tsconfig.json");
        let leaf_meta = std::fs::symlink_metadata(&staged_leaf).unwrap();
        assert!(
            leaf_meta.file_type().is_file() && !leaf_meta.file_type().is_symlink(),
            "the project's own leaf tsconfig must be materialised as a real file"
        );
        // ...and its rewritten `extends` still names the canonical REAL
        // package config (bare-specifier extends are always pinned absolute,
        // regardless of whether the resolved config was excluded from shadow
        // mirroring) — proving the gatekeeper excludes shadow MIRRORING of
        // the package config, not esbuild-visible extends resolution.
        let leaf_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&staged_leaf).unwrap()).unwrap();
        let real_package_config = project_root
            .join("node_modules/@tsconfig/strict-base/tsconfig.json")
            .canonicalize()
            .unwrap();
        assert_eq!(
            leaf_json["extends"].as_str(),
            Some(real_package_config.to_string_lossy().as_ref()),
            "the leaf's rewritten extends must point at the canonical real package config, \
             not a staged spelling under the stage's node_modules: {leaf_json:?}"
        );
    }

    /// Inverted #1667 guard-pin test (issue #1677): a worker whose SOURCE
    /// lives in a sibling workspace package used to fail with the named
    /// #1667 limitation (the #1500 flat-naming contract was project-scoped).
    /// #1677 threads the `worker--ws-` scoped naming contract (issue #1673)
    /// through this pipeline instead, so the sibling worker entry now stages
    /// and names successfully.
    #[test]
    fn client_script_stage_sibling_worker_entry_names_scoped_companion() {
        let workspace = tempdir().unwrap();
        write_reroot_ws_file(
            workspace.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        );
        write_reroot_ws_file(
            workspace.path(),
            "lib/widgets/worker.ts",
            "self.postMessage('sibling');\n",
        );
        let project_root = workspace.path().join("sub-packages/host");
        let entry = write_reroot_ws_file(
            &project_root,
            "pages/widget.client.ts",
            "new Worker(new URL('../../../lib/widgets/worker.ts', import.meta.url), { type: 'module' });\nconsole.log('x');\n",
        );
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let stage = stage_client_script_preprocessing(&project_root, &entries)
            .unwrap()
            .expect("a workspace-sibling worker entry now needs a stage (issue #1677)");

        let workers = stage
            .workers_by_entry
            .get("widget")
            .expect("the widget entry must have a discovered module worker");
        assert_eq!(workers.len(), 1);
        let worker = &workers[0];

        let first_party_root = zfb_types::first_party_root_for(&project_root);
        let expected_filename = zfb_types::module_worker_filename_scoped(
            &project_root,
            &first_party_root,
            &first_party_root.join("lib/widgets/worker.ts"),
        )
        .expect("a workspace-sibling worker source has a scoped companion filename");
        assert_eq!(worker.filename, expected_filename);
        assert!(
            worker.filename.starts_with("worker--ws-"),
            "a workspace-sibling worker source must mint the scoped `-ws-` companion name: {}",
            worker.filename
        );

        // The worker's physical source must be staged at its
        // workspace-relative slot (not the mirrored PROJECT dir), matching
        // the sibling-closure staging that runs earlier in the stage.
        let expected_source_path = stage.root.join("lib/widgets/worker.ts");
        assert_eq!(worker.source_path, expected_source_path);
        assert_eq!(
            std::fs::read_to_string(&worker.source_path).unwrap(),
            "self.postMessage('sibling');\n"
        );
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
            workspace_package_edges_from_islands: Vec::new(),
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
    fn materialise_islands_shadow_uses_copy_mode_with_nested_tsconfig_paths() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("node_modules")).unwrap();
        std::fs::create_dir_all(project_root.join("components")).unwrap();
        std::fs::write(
            project_root.join("components/tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"..","paths":{"@/*":["src/*"]}}}"#,
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
            "project node_modules plus nested tsconfig paths uses copy-mode and omits --preserve-symlinks"
        );
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "plain source is a real copied file in copy-mode"
        );
    }

    #[test]
    fn materialise_islands_shadow_uses_copy_mode_for_workspace_sibling() {
        // Issue #2163: the positive control for the sibling-closure copy-mode
        // disjunct on the islands site — a genuine workspace sibling in the
        // mirrored closure, with NO node_modules and NO tsconfig `paths`
        // anywhere in this fixture, must still select copy mode on its own.
        // The existing negative,
        // `materialise_islands_shadow_mirrors_glob_target_transitive_project_imports`,
        // only proves a project-LOCAL helper still mirrors — its fixture has
        // no workspace/siblings at all, so it cannot exercise this disjunct.
        let workspace = tempdir().unwrap();
        write_shadow_fixture(
            workspace.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        );
        let sibling_src = write_shadow_fixture(
            workspace.path(),
            "lib/shared/helper.ts",
            "export const helper = 'ISLANDS_SIBLING';\n",
        );
        let project_root = workspace.path().join("sub-packages/host");
        let island_src = write_shadow_fixture(
            &project_root,
            "components/gallery.tsx",
            "\"use client\";\n\
             import { helper } from \"../../../lib/shared/helper\";\n\
             export function Gallery() { console.log(helper); return null; }\n",
        );

        let islands = vec![zfb_islands::Island::new("Gallery", island_src.clone())];
        let scan_meta = zfb_islands::ScanMeta {
            uses_client_router: false,
            near_miss_candidates: 0,
            glob_reachable_from_islands: Vec::new(),
            island_reachable_modules: vec![island_src.clone(), sibling_src.clone()],
            raw_import_edges_from_islands: Vec::new(),
            module_worker_edges_from_islands: Vec::new(),
            workspace_package_edges_from_islands: Vec::new(),
        };

        let outcome = materialise_islands_shadow(&project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("a workspace-sibling closure must not keep stopgap: {o:?}")
            }
        };
        assert!(
            !shadow.preserve_symlinks,
            "a workspace sibling in the mirrored closure must select copy mode even without \
             node_modules or tsconfig paths"
        );

        // `bundle_working_dir` is the mirrored PROJECT dir
        // (`<shadow_root>/sub-packages/host`); pop the two project-relative
        // components to reach the shadow root, mirroring how the sibling
        // closure test does it on the client-script preprocessing stage.
        let shadow_root = shadow
            .bundle_working_dir
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let staged_sibling = shadow_root.join("lib/shared/helper.ts");
        let metadata = std::fs::symlink_metadata(&staged_sibling).unwrap();
        assert!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "the workspace sibling must be mirrored as a real file, not a symlink, once the \
             closure forces copy mode"
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
            workspace_package_edges_from_islands: Vec::new(),
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

    /// Issue #2301: the islands-site companion to
    /// `stage_client_script_preprocessing_with_worker_context_excludes_nested_workspace_node_modules_tsconfig`
    /// above, driving the SAME shared topology
    /// (`write_nested_node_modules_tsconfig_workspace`) through
    /// `materialise_islands_shadow_with_worker_context` instead — the two
    /// production entry points that independently call
    /// `collect_islands_shadow_configs` / `materialise_shadow_typescript_configs`
    /// against the widened workspace root (issue #2163 established the two
    /// sites as maintained parallel twins).
    #[cfg(unix)]
    #[test]
    fn materialise_islands_shadow_with_worker_context_excludes_nested_workspace_node_modules_tsconfig(
    ) {
        let workspace = tempdir().unwrap();
        let project_root = write_nested_node_modules_tsconfig_workspace(workspace.path());
        let (island_src, glob_src, _target) = write_basic_glob_shadow_project(&project_root);

        let (islands, scan_meta) = basic_shadow_inputs(
            &project_root,
            vec![glob_src.clone()],
            vec![island_src.clone(), glob_src],
        );
        let outcome = materialise_islands_shadow(&project_root, &islands, &scan_meta).expect(
            "shadow materialisation must not IO-error — a wrongly-collected nested package \
             tsconfig would materialise a real node_modules dir before the project \
             node_modules symlink step runs, which then fails with EEXIST",
        );
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => panic!("supported glob must be ready: {o:?}"),
        };

        let shadow_root = shadow._tempdir.path();
        assert!(
            std::fs::symlink_metadata(shadow_root.join("node_modules"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the workspace-hoisted node_modules must be linked wholesale at the shadow root"
        );
        let project_node_modules_link = shadow.bundle_working_dir.join("node_modules");
        assert!(
            std::fs::symlink_metadata(&project_node_modules_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the project's own nested node_modules must be linked wholesale, not shadowed by \
             a real dir the config-collection pass created first"
        );

        let staged_leaf = shadow.bundle_working_dir.join("tsconfig.json");
        let leaf_meta = std::fs::symlink_metadata(&staged_leaf).unwrap();
        assert!(
            leaf_meta.file_type().is_file() && !leaf_meta.file_type().is_symlink(),
            "the project's own leaf tsconfig must be materialised as a real file"
        );
        let leaf_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&staged_leaf).unwrap()).unwrap();
        let real_package_config = project_root
            .join("node_modules/@tsconfig/strict-base/tsconfig.json")
            .canonicalize()
            .unwrap();
        assert_eq!(
            leaf_json["extends"].as_str(),
            Some(real_package_config.to_string_lossy().as_ref()),
            "the leaf's rewritten extends must point at the canonical real package config, \
             not a staged spelling under the shadow's node_modules: {leaf_json:?}"
        );

        assert!(
            shadow.remap.contains_key(&island_src),
            "island source_path must be remapped into the shadow"
        );
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
            workspace_package_edges_from_islands: Vec::new(),
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
        // Serialise against the `..._even_on_tailwind_failure` test above,
        // which `set_var`s `ZFB_TAILWIND_BIN` process-wide. Both run in this
        // binary under the `--include-ignored` command in this test's own
        // `#[ignore]` reason (issue #1799 review finding).
        let _env_lock = TAILWIND_BIN_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

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
            timing_enabled: false,
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

    // -------------------------------------------------------------------------
    // copy_redirects_file unit tests (issue #1543 / epic #1541 Preview Parity)
    // -------------------------------------------------------------------------

    /// Missing `public/_redirects` is silently ignored (no error, no
    /// file created) — not every project uses the feature.
    #[test]
    fn copy_redirects_file_missing_source_is_noop() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        copy_redirects_file(project_root, &outdir, std::path::Path::new("public"))
            .expect("missing _redirects must not error");
        assert!(!outdir.join("_redirects").exists());
    }

    /// Default project shape: `public/_redirects` lands at the output
    /// root, `<outdir>/_redirects`.
    #[test]
    fn copy_redirects_file_default_lands_at_outdir_root() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        let contents = b"/old /new 301\n";
        std::fs::write(project_root.join("public/_redirects"), contents).unwrap();
        copy_redirects_file(project_root, &outdir, std::path::Path::new("public"))
            .expect("copy must succeed");
        let dest = outdir.join("_redirects");
        assert!(dest.is_file(), "_redirects must land at outdir root");
        assert_eq!(std::fs::read(&dest).unwrap(), contents);
    }

    /// Custom `outDir`-aware: this function receives `outdir` as-is
    /// (the fully-resolved output directory), so a custom outDir is
    /// honoured automatically — `_redirects` lands at its root too.
    #[test]
    fn copy_redirects_file_custom_outdir_lands_at_its_root() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("build-output");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(project_root.join("public/_redirects"), b"/a /b 301\n").unwrap();
        copy_redirects_file(project_root, &outdir, std::path::Path::new("public"))
            .expect("copy must succeed");
        assert!(outdir.join("_redirects").is_file());
    }

    /// Custom `publicDir`-aware: the source is `<public_dir>/_redirects`,
    /// matching where `copy_public_dir` looks for the rest of `public/`.
    #[test]
    fn copy_redirects_file_custom_public_dir_is_honoured() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("static-files")).unwrap();
        std::fs::write(project_root.join("static-files/_redirects"), b"/a /b 301\n").unwrap();
        copy_redirects_file(project_root, &outdir, std::path::Path::new("static-files"))
            .expect("copy must succeed");
        assert!(outdir.join("_redirects").is_file());
    }

    /// Custom `base`: unlike `copy_public_dir`, `_redirects` must land
    /// at the bare output root, NEVER under a base-path segment — the
    /// caller (`run_build`) intentionally never passes a base-prefixed
    /// path to this function, and this test pins that contract at the
    /// function-signature level (no `base` parameter exists to misuse).
    #[test]
    fn copy_redirects_file_ignores_base_by_construction() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(project_root.join("public/_redirects"), b"/a /b 301\n").unwrap();
        // Simulate a project with `base: "/pj/test/"` configured: the
        // rest of public/ would land under outdir/pj/test/ via
        // copy_public_dir, but copy_redirects_file has no base
        // parameter to relocate under — it always targets outdir root.
        copy_public_dir(
            project_root,
            &outdir,
            std::path::Path::new("public"),
            Some("/pj/test/"),
        )
        .expect("public dir copy must succeed");
        copy_redirects_file(project_root, &outdir, std::path::Path::new("public"))
            .expect("redirects copy must succeed");
        assert!(
            outdir.join("_redirects").is_file(),
            "_redirects must land at the bare outdir root even with base configured"
        );
        assert!(
            !outdir.join("pj/test/_redirects").exists(),
            "_redirects must NOT be relocated under the base segment"
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
        let (map, _findings) = build_prerender_map(&routes, dir.path(), |_| {});

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

    /// Issue #1802 (epic #1799, gap (a)): the CSS source-plan seam must
    /// publish its computed sibling mirror roots to the caller-supplied
    /// observer BEFORE the Tailwind subprocess ever runs — and therefore
    /// even when that subprocess later fails. Without this, a failed boot
    /// CSS build in dev would leave the dev-watch reconciliation with no
    /// root to register, and there would be no filesystem event through
    /// which recovery could ever trigger.
    ///
    /// Forces the Tailwind engine to fail deterministically WITHOUT
    /// spawning a real subprocess: `TailwindSubprocessEngine::produce_utility_css`
    /// (crates/zfb-css/src/engine.rs) checks `binary_path.exists()` before
    /// ever exec'ing, so pointing `ZFB_TAILWIND_BIN` at a path that is
    /// guaranteed not to exist is a fast, hermetic, deterministic failure —
    /// no real tailwind binary is spawned.
    ///
    /// `set_var` is PROCESS-wide, and `ZFB_TAILWIND_BIN` is genuinely read
    /// elsewhere in this same binary — by production code (the
    /// `with_embedded_binary` skip above) and by the env-gated
    /// `default_runner_emit_prod_assets_returns_non_empty_css_for_real_project`
    /// below, which crates/CLAUDE.md documents running in this very process via
    /// `cargo test -p zfb --lib commands::build:: -- --include-ignored`.
    /// `EnvGuard` bounds the mutation in TIME but not across THREADS, so
    /// both tests take [`TAILWIND_BIN_ENV_LOCK`] to serialise against each
    /// other; without it this test can point that one at a bogus path
    /// mid-run and flake it (issue #1799 review finding).
    #[test]
    fn build_default_css_payload_with_source_plan_publishes_mirror_roots_even_on_tailwind_failure()
    {
        let _env_lock = TAILWIND_BIN_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        struct EnvGuard {
            prev: Option<std::ffi::OsString>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(v) => std::env::set_var("ZFB_TAILWIND_BIN", v),
                    None => std::env::remove_var("ZFB_TAILWIND_BIN"),
                }
            }
        }
        let prev = std::env::var_os("ZFB_TAILWIND_BIN");
        std::env::set_var(
            "ZFB_TAILWIND_BIN",
            "/nonexistent/zfb-test-tailwind-missing-binary",
        );
        let _guard = EnvGuard { prev };

        let (_tmp, project) = sibling_css_workspace_fixture();

        // `Config::default()` leaves Tailwind ENABLED (the default) — the
        // `tailwind.enabled = false` path never computes mirror roots at
        // all (there is no `@source` scan to feed), so this must exercise
        // the Tailwind-enabled branch to reach the seam under test.
        let cfg = Config::default();
        let observed: std::cell::RefCell<Option<Vec<PathBuf>>> = std::cell::RefCell::new(None);

        let result = build_default_css_payload_with_source_plan(
            &project,
            &project.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
            &|roots| {
                *observed.borrow_mut() = Some(roots.to_vec());
            },
        );

        assert!(
            result.is_err(),
            "expected the Tailwind engine to fail deterministically (missing binary); \
             got {result:?}"
        );
        let observed = observed.borrow();
        assert!(
            observed.is_some(),
            "the mirror-root observer must fire even though the Tailwind subprocess \
             later fails — otherwise a failed boot CSS build never registers sibling \
             watches"
        );
        assert!(
            !observed.as_ref().unwrap().is_empty(),
            "this fixture claims a workspace sibling via a tsconfig alias, so the \
             published set must be non-empty: {observed:?}"
        );
    }

    /// Review finding (issue #1802): `tailwind.enabled = false` opts out of
    /// the Tailwind `@source` scan, NOT out of CSS Modules discovery — see
    /// `css_payload_emits_claimed_sibling_module_css_and_matches_class_map`,
    /// which proves `build_authored_only_css_payload` still ships a claimed
    /// sibling's `.module.css` bytes on this exact path. An earlier version
    /// of this seam published an EMPTY mirror-root set whenever Tailwind was
    /// disabled, which would have left that same sibling directory
    /// unwatched in dev — a claimed sibling's CSS Module edit would go
    /// stale until restart even though Tailwind was never involved. The
    /// observer must fire with the SAME non-empty set regardless of
    /// `tailwind.enabled`.
    #[test]
    fn build_default_css_payload_with_source_plan_publishes_mirror_roots_with_tailwind_disabled() {
        let (_tmp, project) = sibling_css_workspace_fixture();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let observed: std::cell::RefCell<Option<Vec<PathBuf>>> = std::cell::RefCell::new(None);

        let payload = build_default_css_payload_with_source_plan(
            &project,
            &project.join("dist"),
            &cfg,
            &[],
            &[],
            &[],
            &|roots| {
                *observed.borrow_mut() = Some(roots.to_vec());
            },
        )
        .expect("authored-only path must not error (hermetic, no tailwind binary required)");
        assert!(payload.is_some());

        let observed = observed.borrow();
        let roots = observed
            .as_ref()
            .expect("the observer must fire on the tailwind-disabled path too");
        assert!(
            !roots.is_empty(),
            "mirror roots must still be published with tailwind.enabled = false, since \
             CSS Modules discovery still scans them: {roots:?}"
        );
    }
}
