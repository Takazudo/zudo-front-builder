//! `zfb dev` — boot the dev pipeline + dev HTTP server.
//!
//! Wires four crates together per the doc-comment in
//! [`zfb_server`]'s lib.rs:
//!
//! 1. A [`tokio::sync::broadcast`] channel of [`zfb_server::ReloadEvent`]s
//!    that the SSE live-reload route consumes.
//! 2. A [`zfb_server::PageCache`] of rendered HTML keyed by URL path.
//! 3. A [`zfb_build::BuildOrchestrator`] driving the watcher + dep-graph
//!    + asset pipeline; its `on_outcome` callback translates every
//!      non-noop tick into reload events via
//!      [`zfb_server::outcome_to_events`] and broadcasts them.
//! 4. A long-lived [`zfb_build::renderer::RendererState`] (T7) that
//!    owns the embedded V8 host. The asset pipeline's
//!    [`PageRenderer`] callback drives [`zfb_build::renderer::render_one`]
//!    against this state per affected route, so a single edit triggers
//!    one in-process render — not a fresh host boot.
//!
//! Then it binds the address from `args.host:args.port`, prints the
//! ready banner via [`crate::output::ready`], and runs the axum server
//! until Ctrl+C.
//!
//! ## Lifecycle of the renderer state
//!
//! `start(...)` is called once at boot. The returned
//! [`zfb_build::renderer::RendererState`] is wrapped in a [`Drop`]
//! guard so panics, Ctrl-C, or any other early-exit path tears the
//! embedded V8 host down cleanly. Without that guard a panicking
//! dev loop would leak the host resources.
//!
//! ## Configuration
//!
//! Project configuration is loaded via [`crate::config::load_from_dir`]
//! at startup. Today this resolves to a `zfb.config.json` if present, or
//! sensible defaults otherwise; encountering a `zfb.config.ts` produces a
//! clear "not yet supported" error.
//!
//! **Precedence rule:** CLI args (`--host`, `--port`) override the
//! corresponding config values when supplied.
//!
//! ## v1 → wave-3 transition
//!
//! Earlier waves passed a noop [`PageRenderer`] into the orchestrator
//! that returned an empty `RenderedPage` list — the dev cache stayed
//! empty and every request fell through to the dev 404 body.
//!
//! Wave 3 (T7) replaces that noop with a renderer-backed callback. The
//! callback maps each page id to its [`zfb_build::renderer::RouteUniverseEntry`]
//! and drives [`zfb_build::renderer::render_one`] against the long-
//! lived [`zfb_build::renderer::RendererState`]. The result file is
//! read back into a [`zfb_build::RenderedPage`] so the existing
//! atomic-write + reload-broadcast plumbing keeps working unchanged.
//!
//! ### Same gaps as `zfb build`
//!
//! Dynamic / catchall routes (`paths()` runtime expansion) and the
//! Worker-entry wrapping are still pending. See [`crate::commands::build`]
//! for the detailed gap analysis. Today the dev renderer reuses the
//! same plumbing and inherits the same limitations.

// V8-off (issue #371, sub-task 4.1a): on the `!feature = "embed_v8"`
// path the `pub async fn run` body and the V8-bearing helpers below
// are compiled out. The imports, helper functions, and types they
// reference then look unused — silence the lints in that
// configuration so V8-off builds stay warning-clean. The V8-on path
// continues to surface real unused-item warnings.
#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_build::bundler::{bundle, BundleMode, BundlerInput, BundlerOutput};
use zfb_build::renderer::{
    render_one, shutdown, start, Backend, RendererStartInput, RendererState, RouteUniverseEntry,
};
use zfb_build::{
    BuildContext, BuildOrchestrator, BuildOutcome, CssRunner, DevAssetPipeline, DiscoveryOutcome,
    IslandsBundleInfo, IslandsRunner, OrchestratorConfig, PageRenderer, RelDistPath, RenderedPage,
    RendererReloader,
};
use zfb_graph::persist::{load_from_disk, save_to_disk, ManifestDigest};
use zfb_graph::{DependencyGraph, PageDeps, PageId};
use zfb_server::{
    outcome_to_events, serve_with_listener, PageCache, ReloadEvent, ServeOpts, SsrDispatcher,
    SsrRouteRecord, SsrRouteSet, SsrRoutesHandle,
};

use crate::cli::DevArgs;
use crate::commands::resolve::{resolve_addr, resolve_host, resolve_port, resolve_under_root};
use crate::config;
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, cfg_framework_to_render, check_runtime_installed,
    embedded_node_modules, eval_deferred_paths_via_worker, expand_dynamic_routes, WorkerDispatch,
};
#[cfg(feature = "embed_v8")]
use zfb_render::paths::PathsCache;

/// Default source directories the watcher follows.
const DEFAULT_WATCH_ROOTS: &[&str] = &[
    "pages",
    "content",
    "components",
    "layouts",
    "styles",
    "data",
    "public",
    "zfb.config.json",
    "zfb.config.ts",
];

/// Strip `.` components so `./src/mdx` and `src/mdx` compare equal in
/// the dedupe / coverage checks below.
fn normalize_relative(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// The dev watcher's source roots: [`DEFAULT_WATCH_ROOTS`] plus each
/// configured collection's `path`.
///
/// Collections may live anywhere in the project (`CollectionDef::path`
/// is project-root-relative, e.g. `src/mdx/notes`) — only watching the
/// fixed default roots means edits under such a collection never produce
/// a watcher event and the dev server serves stale HTML until restart.
///
/// Rules:
/// - paths are normalized (leading `./` stripped) before comparison;
/// - a collection path already covered by an earlier root is skipped
///   (`content/blog` is inside the default `content` root; nested
///   collections dedupe against each other after the shallow-first
///   sort), avoiding double event delivery from overlapping recursive
///   watches;
/// - missing directories are tolerated downstream — the watcher warns
///   and skips them at registration.
fn derive_watch_roots(cfg: &config::Config) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
    let mut collection_paths: Vec<PathBuf> = cfg
        .collections
        .iter()
        .map(|c| normalize_relative(&c.path))
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    // Shallow-first so a parent collection root lands before its
    // children and the coverage check below collapses the family to
    // the parent.
    collection_paths.sort_by_key(|p| p.components().count());
    for p in collection_paths {
        let covered = roots.iter().any(|r| p.starts_with(r));
        if !covered {
            roots.push(p);
        }
    }
    roots
}

/// Entry point for `zfb dev`.
///
/// Available only when the `embed_v8` cargo feature is on (issue #371,
/// sub-task 4.1a). The V8-off counterpart lives at the bottom of this
/// file and surfaces a clear runtime error explaining that this binary
/// was built without V8 support.
#[cfg(feature = "embed_v8")]
pub async fn run(args: &DevArgs) -> Result<()> {
    // 1. Resolve the project root and load configuration.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;

    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    let dist_root = resolve_under_root(&project_root, &cfg.out_dir);
    let public_root = resolve_under_root(&project_root, &cfg.public_dir);

    if !dist_root.exists() {
        std::fs::create_dir_all(&dist_root)
            .with_context(|| format!("failed to create dist dir {}", dist_root.display()))?;
    }

    // Issue #534 — dev's per-route HTML writes must NOT land in the
    // project's `outDir` (`dist/`). When dev was given the same `dist_dir`
    // as the build pipeline, starting `pnpm dev` after a clean `pnpm build`
    // overwrote every prod HTML file with a dev-mode rendering that lacked
    // the production `<link rel="stylesheet">` / islands `<script
    // type="module">` head injections (`prod_head_assets`). The on-disk
    // copy then silently broke a subsequent `pnpm preview`, which serves
    // `dist/` verbatim.
    //
    // The dev server primarily serves HTML from its in-memory `PageCache`
    // (URL-keyed — see `zfb_server::routes::PageCache`), so the on-disk
    // copy is mostly a stale by-product of the read-back-after-render step
    // in `DevRenderSession::render_one_with`. Redirecting it to a
    // dev-only directory under `.zfb-build/` keeps the read-back working
    // while taking dev out of the production output's write set entirely.
    //
    // The dev server's disk-fallback in `read_from_dist` intentionally
    // still points at `dist_root` (not at `dev_html_root`): on a cold
    // cache it serves whatever the most recent `pnpm build` left there,
    // which is what users expect from "build, then dev for a quick check"
    // and is now safe because dev no longer mutates that file.
    let dev_html_root = dev_html_root_for(&project_root);
    // Guard against a pathological `outDir` (`.zfb-build/dev-pages`,
    // its parent, or anything that overlaps): when the dev HTML root
    // collides with `dist_root` we are back to the #534 condition and
    // dev's per-route writes corrupt `pnpm preview`'s output. Refuse
    // to start with a clear error so the user can pick a non-colliding
    // `outDir`.
    if dev_html_root == dist_root
        || dev_html_root.starts_with(&dist_root)
        || dist_root.starts_with(&dev_html_root)
    {
        anyhow::bail!(
            "zfb dev: configured `outDir` ({}) overlaps with the dev HTML \
             scratch root ({}). Pick an `outDir` outside `.zfb-build/` \
             (the default `dist/` is fine) — otherwise dev's per-route \
             HTML writes would corrupt the production build output.",
            dist_root.display(),
            dev_html_root.display(),
        );
    }
    if !dev_html_root.exists() {
        std::fs::create_dir_all(&dev_html_root).with_context(|| {
            format!(
                "failed to create dev html dir {}",
                dev_html_root.display()
            )
        })?;
    }

    let host = resolve_host(args.host.as_deref(), cfg.host.as_deref(), DEFAULT_DEV_HOST);
    let port = resolve_port(args.port, cfg.port, DEFAULT_DEV_PORT);
    let addr = resolve_addr(host.as_str(), port)?;

    let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);
    let pages = PageCache::new();

    // Plugin lifecycle. Spawn the host once at boot so
    // `preBuild` runs before the bundler/renderer start, and so dev-
    // middleware registrations can be installed into the dev server.
    // The host is dropped when this `run` returns (Ctrl+C path), which
    // kills the subprocess.
    let plugin_host = crate::commands::plugins::maybe_spawn_host(&cfg).await?;

    // #255 — run the new `setup` hook once, before `preBuild`. The
    // registries are owned by `zfb-build`; we extract the
    // `injected_routes` view, translate it into the dev-server's
    // local mirror, and plumb it through `ServeOpts.injected_routes`.
    // The alias map / virtual-module registry feed Wave 2 (#260,
    // #261) which is not yet wired — we keep `_setup_registries` in
    // scope here so the registries stay live for the duration of the
    // dev session (the embedded references count on it).
    let setup_registries = if let Some(h) = plugin_host.as_ref() {
        let cfg_json = serde_json::to_value(&cfg)
            .context("plugin lifecycle: serialise config for setup ctx")?;
        h.run_setup(&project_root, zfb_build::SetupCommand::Dev, &cfg_json)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("setup lifecycle hook")?
    } else {
        zfb_build::SetupRegistries::empty()
    };

    if let Some(h) = plugin_host.as_ref() {
        let ctx = zfb_build::BuildHookContext {
            project_root: project_root.clone(),
            out_dir: dist_root.clone(),
            config: serde_json::to_value(&cfg)
                .context("plugin lifecycle: serialise config for preBuild ctx")?,
            // dev mode: routes always absent on preBuild (no manifest yet).
            routes: None,
        };
        h.run_pre_build(&ctx)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("preBuild lifecycle hook")?;
    }
    let plugin_set = if let Some(h) = plugin_host.as_ref() {
        crate::commands::plugins::build_dev_middleware_set(h, &project_root, &cfg).await?
    } else {
        None
    };

    // Translate the build-side InjectedRouteList into the dev
    // server's local mirror so zfb-server doesn't depend on
    // zfb-build. Wave 2 (#260, #261) consume `setup_registries.aliases`
    // and `setup_registries.virtual_modules` — kept named (not `_`)
    // so the variable stays in scope as the wiring lands.
    let injected_route_set = if setup_registries.injected_routes.is_empty() {
        None
    } else {
        let records: Vec<zfb_server::InjectedRouteRecord> = setup_registries
            .injected_routes
            .iter()
            .map(|r| zfb_server::InjectedRouteRecord {
                pattern: r.pattern.clone(),
                entrypoint: r.entrypoint.clone(),
                plugin: r.plugin.clone(),
            })
            .collect();
        Some(zfb_server::InjectedRouteSet::new(records))
    };
    // #261 — build mode wires `aliases` + `virtual_modules` into the esbuild
    // subprocess config (see `crates/zfb/src/commands/build.rs`). Dev mode
    // (#377 wiring below) reuses the same setup_registries-derived alias /
    // virtual-module lists to drive its own islands bundle so dev and build
    // agree on plugin-registered resolution.

    // #260 — pre-fetch all virtual-module sources once so the embedded V8 host
    // can resolve plugin-registered virtual modules at runtime. Each
    // `invoke_virtual_loader` runs exactly once per dev-session boot; the
    // resolved sources are owned by the `PluginRegistryHooks` clone the
    // factory closure captures.
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
    let v8_plugin_hooks = crate::v8_host_adapter::translate_setup_registries_to_hooks(
        &setup_registries,
        &virtual_sources,
    );
    // #268 — derive alias / virtual-module lists for the main bundler from
    // the same setup_registries the islands path already uses. These are
    // cheap clones of data already on the heap; producing them here (before
    // the rename below) keeps `boot_dev_renderer` API symmetrical with the
    // build-command path.
    let dev_plugin_alias_entries: Vec<(String, String)> = setup_registries
        .aliases
        .iter()
        .map(|(from, entry)| (from.clone(), entry.target.to_string_lossy().into_owned()))
        .collect();
    let dev_plugin_virtual_modules: Vec<(String, String)> = virtual_sources
        .iter()
        .map(|(spec, src)| (spec.clone(), src.clone()))
        .collect();
    // Keep `setup_registries` in scope so the underlying registries stay live
    // (the hook entries borrow nothing from it now, but the variable's role as
    // the lifecycle owner is preserved).
    let _setup_registries = setup_registries;

    // 2. Stand up the long-lived renderer state if the project looks
    //    runnable. We surface failures as a warning + fall back to the
    //    noop renderer so the dev server still boots — the user can
    //    still poke at the dev URL while they fix the underlying
    //    bundler / runtime issue.
    let dev_session = match boot_dev_renderer(
        &project_root,
        &cfg,
        v8_plugin_hooks,
        dev_plugin_alias_entries.clone(),
        dev_plugin_virtual_modules.clone(),
    ) {
        Ok(s) => Some(s),
        Err(err) => {
            output::warn(format!(
                "renderer disabled — falling back to empty page cache: {err:#}",
            ));
            None
        }
    };

    // 3. Build orchestrator setup.
    //
    // Cold-start optimisation: try to reuse a previously persisted
    // graph from `.zfb/graph.bin`. If the manifest digest still
    // matches the current project layout, deserialise and reuse —
    // otherwise build fresh and save the new graph back so the
    // *next* cold start is fast.
    // Includes configured collection paths (e.g. `src/mdx/notes`) so
    // edits there produce watcher events; the manifest digest below
    // covers them automatically since it walks the same roots.
    let watch_roots: Vec<PathBuf> = derive_watch_roots(&cfg);
    let graph_cache_path = project_root.join(".zfb").join("graph.bin");
    let manifest_digest = compute_manifest_digest(&project_root, &watch_roots);
    let initial_graph = load_persisted_graph(&graph_cache_path, manifest_digest.as_ref());
    // Note: we deliberately do NOT write a fresh empty graph here on
    // a cache miss. If we did, a `zfb dev` killed before the
    // orchestrator's first watcher tick would persist an empty graph
    // tagged with the current digest — and the next cold start would
    // happily reuse that empty cache as authoritative. Save only on
    // shutdown (below), once the graph has actually been populated.
    //
    // Formerly `initial_graph.unwrap_or_default()`. Now explicit: on a
    // cache miss we construct a known-empty graph. Default was removed
    // from DependencyGraph to prevent silent empty-graph construction
    // elsewhere.
    let graph = Arc::new(Mutex::new(
        initial_graph.unwrap_or_default(),
    ));

    // Seed the graph with all page source paths from the router scan so
    // `plan_for_changes` can resolve `PageSelection::All` to a concrete
    // page list even before the first file-change tick. Without this
    // seeding the graph is empty, `resolve_all` produces an empty page
    // set, and every watcher tick is a no-op (zfb#N / cold-start bug).
    if let Some(ref session) = dev_session {
        if let Ok(mut g) = graph.lock() {
            for page_id in session.page_ids() {
                g.upsert(PageDeps::new(page_id, vec![]));
            }
        }
    }

    let graph_for_save = Arc::clone(&graph);
    let pipeline = DevAssetPipeline::new();
    let extra_watch_paths = resolve_extra_watch_paths(&cfg.extra_watch_paths);
    // Configured collection roots classify as Content ahead of the
    // standard root-segment walk — without this, a collection under
    // `src/` (e.g. `src/mdx/notes`) classifies as Module and wastefully
    // re-bundles islands on every entry edit.
    let content_roots: Vec<PathBuf> = cfg
        .collections
        .iter()
        .map(|c| normalize_relative(&c.path))
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    let orch_config = OrchestratorConfig::new(&project_root, watch_roots.clone())
        .with_extra_watch_paths(extra_watch_paths)
        .with_policy(
            zfb_build::GranularityPolicy::default().with_content_roots(content_roots),
        );
    let orchestrator = BuildOrchestrator::new(orch_config, graph, pipeline);

    let render_pages: PageRenderer = match dev_session.as_ref() {
        // Issue #534 — pass `dev_html_root` (under `.zfb-build/`), not
        // `dist_root`, so per-route dev renders do not overwrite the
        // production HTML files that `pnpm preview` serves.
        Some(session) => make_render_callback(session.clone(), dev_html_root.clone()),
        None => Arc::new(|_pages: &[PageId]| Ok(Vec::new())),
    };

    // Issue #377 — dev-mode initial-load islands bundling.
    //
    // Build the same IslandsPluginConfig the production runner uses so dev
    // and build resolve aliases / virtual modules identically inside the
    // islands esbuild invocation. Then:
    //
    // 1. Run an initial bundle eagerly here, before the dev server starts
    //    accepting requests, so the FIRST GET on a page with an island
    //    already sees a `<script type="module" src="/assets/islands.js">`
    //    in the served HTML (the bug the issue reports — without this
    //    initial pass the orchestrator would only emit the bundle on
    //    the first file-change tick, leaving cold-boot page loads silent).
    // 2. Seed `islands_bundle_url` with the result (or leave it `None` when
    //    the project has no `"use client"` components — projects without
    //    islands must not ship a `<script>` tag pointing at a non-existent
    //    bundle).
    // 3. Wire the same wrapper as the orchestrator's `run_islands` callback
    //    so a watcher tick re-bundles, rewrites the shared URL handle, and
    //    surfaces an `IslandsBundleInfo` that flows through the existing
    //    `outcome_to_events` -> SSE `Islands` event path (the dev livereload
    //    client then re-imports the new bundle in place).
    // Reuse the alias / virtual-module lists already derived from
    // `setup_registries` higher up (so the islands esbuild and the main
    // dev bundler resolve plugin-registered modules identically).
    let islands_plugin_config = crate::commands::build::IslandsPluginConfig {
        alias_entries: dev_plugin_alias_entries.clone(),
        virtual_modules: dev_plugin_virtual_modules.clone(),
    };
    // The dev server only serves assets it mounted itself. For a path-
    // shaped `base` (e.g. `/foo/`) the mount prefix is `Some("/foo")`
    // and the bundle URL must include it. For an absolute-URL `base`
    // (CDN deploy target — `https://cdn.example.com/`)
    // `dev_mount_prefix` returns `None` because the dev server mounts
    // at root; injecting the absolute URL would point the browser at
    // an origin the dev server never serves, breaking first-load
    // hydration for the CDN deploy scenario. Source the prefix from
    // `dev_mount_prefix` (not `asset_url_base_prefix`) so absolute-URL
    // bases collapse cleanly to "no prefix" here.
    let dev_islands_url_prefix: String =
        zfb_types::dev_mount_prefix(cfg.base.as_deref()).unwrap_or_default();
    let islands_bundle_url_handle: zfb_server::IslandsBundleUrl =
        Arc::new(std::sync::RwLock::new(None));

    // Tracks chunk filenames written by the most recent islands bundle so the
    // next rebundle tick can delete stale ones (issue #809).
    let live_chunk_filenames: Arc<Mutex<HashSet<String>>> =
        Arc::new(Mutex::new(HashSet::new()));

    // Eager initial bundle. Failures are non-fatal — we warn and let the
    // dev server boot anyway. The hot-rebuild path will retry on the next
    // file change so a transient esbuild hiccup at boot doesn't strand the
    // user. Pre-existing islands assets on disk from a previous build are
    // also overwritten by this call (the bundler always writes to the
    // stable path), keeping dev's view of `/assets/islands.js` consistent
    // with the source tree.
    // Small helper closure to translate a stable_url into the prefixed
    // form the dev server actually serves it at (`/assets/islands.js`
    // for no-base, `/foo/assets/islands.js` for `base: "/foo/"`, plain
    // `/assets/islands.js` for absolute-URL bases). Captures by clone
    // so the run_islands callback below can own its own copy.
    let prefix_for_init = dev_islands_url_prefix.clone();
    let prefixed_islands_url = move |stable: String| -> String {
        if prefix_for_init.is_empty() {
            stable
        } else {
            format!("{prefix_for_init}{stable}")
        }
    };
    match crate::commands::build::build_default_islands_payload(
        &project_root,
        &dist_root,
        cfg.framework,
        &islands_plugin_config,
    ) {
        Ok(Some(payload)) => {
            let url = prefixed_islands_url(payload.stable_url);
            if let Ok(mut guard) = islands_bundle_url_handle.write() {
                *guard = Some(url);
            }
            // Write chunk companions alongside islands.js and seed the
            // live-chunk tracker. Boot failures here are non-fatal —
            // the server still comes up; the next successful rebuild
            // will retry.
            let assets_dir = dist_root.join(zfb_types::DIST_ASSETS_DIR);
            match refresh_dev_island_chunks(&assets_dir, &payload.companions, &HashSet::new()) {
                Ok(names) => {
                    if let Ok(mut guard) = live_chunk_filenames.lock() {
                        *guard = names;
                    }
                }
                Err(e) => {
                    output::warn(format!(
                        "initial islands chunks write failed (chunks may 404 \
                         until the next rebuild): {e:#}"
                    ));
                }
            }
        }
        Ok(None) => {
            // No `"use client"` islands in the project. Leave the handle
            // at `None` so the server skips head injection entirely.
        }
        Err(err) => {
            output::warn(format!(
                "initial islands bundle failed (no <script type=\"module\"> \
                 will be injected until the next successful rebuild): {err:#}"
            ));
        }
    }

    let run_islands: Option<IslandsRunner> = {
        let project_root = project_root.clone();
        let dist_root_for_islands = dist_root.clone();
        let plugin_cfg = islands_plugin_config.clone();
        let framework = cfg.framework;
        let url_prefix = dev_islands_url_prefix.clone();
        let url_handle = Arc::clone(&islands_bundle_url_handle);
        let chunk_names = Arc::clone(&live_chunk_filenames);
        Some(Arc::new(move || -> Result<Option<IslandsBundleInfo>> {
            let payload = crate::commands::build::build_default_islands_payload(
                &project_root,
                &dist_root_for_islands,
                framework,
                &plugin_cfg,
            )?;
            // Rewrite the shared handle so the next initial GET (a fresh
            // browser tab, or a page that has not yet hydrated) sees the
            // current bundle URL. The dev server holds the same Arc, so
            // this is visible without re-routing through ServeOpts.
            //
            // Treat lock poisoning as a soft event: a writer panic should
            // not abort the watcher loop. Recover the inner and continue.
            let mut guard = url_handle.write().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "dev.run_islands.url_handle",
                    "rwlock poisoned, recovered"
                );
                p.into_inner()
            });
            let Some(payload) = payload else {
                // The project produced no islands bundle this tick. Clear
                // the shared URL so the next served HTML response does
                // NOT keep injecting a stale `<script type="module">`
                // tag — without this, removing the last `"use client"`
                // component would leave the previously-emitted bundle URL
                // visible on every page until the dev server restarts.
                *guard = None;
                // Also prune any chunks that were part of the last bundle
                // — with no islands bundle at all, none of the chunk files
                // should be served either.
                {
                    let mut prev = chunk_names.lock().unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "dev.run_islands.chunk_names (clear)",
                            "mutex poisoned, recovered"
                        );
                        p.into_inner()
                    });
                    let assets_dir = dist_root_for_islands.join(zfb_types::DIST_ASSETS_DIR);
                    if let Err(e) =
                        refresh_dev_island_chunks(&assets_dir, &[], &prev)
                    {
                        tracing::warn!(
                            error = %e,
                            "dev islands: failed to prune stale chunks after no-bundle tick (ignored)"
                        );
                    }
                    *prev = HashSet::new();
                }
                return Ok(None);
            };
            let bundle_url = if url_prefix.is_empty() {
                payload.stable_url
            } else {
                format!("{url_prefix}{}", payload.stable_url)
            };
            // Write / prune chunk files for this generation.
            {
                let mut prev = chunk_names.lock().unwrap_or_else(|p| {
                    tracing::warn!(
                        site = "dev.run_islands.chunk_names",
                        "mutex poisoned, recovered"
                    );
                    p.into_inner()
                });
                let assets_dir = dist_root_for_islands.join(zfb_types::DIST_ASSETS_DIR);
                match refresh_dev_island_chunks(&assets_dir, &payload.companions, &prev) {
                    Ok(names) => *prev = names,
                    Err(e) => {
                        return Err(e.context("dev islands: failed to refresh chunk files"));
                    }
                }
            }
            // The bundler does not currently surface a "bytes-changed" bit
            // back through `build_default_islands_payload` — the URL stays
            // stable (`/assets/islands.js`) on every rebuild, the bytes on
            // disk update in place. Report `changed = true` so the SSE
            // layer always emits a reload event after a successful
            // re-bundle; the browser then re-imports the URL with a
            // cache-busting `?v=…` query that picks up the new bytes.
            // `components` is empty because the build-side payload doesn't
            // carry per-island names; the livereload client's empty-
            // component path handles "unknown components" by reloading the
            // whole bundle.
            let info = IslandsBundleInfo {
                changed: true,
                bundle_url: bundle_url.clone(),
                components: Vec::new(),
            };
            *guard = Some(bundle_url);
            Ok(Some(info))
        }))
    };

    // Issue #494 / #498: wire the CSS runner end-to-end, mirroring the
    // islands runner above.
    //
    // Step 1: shared URL handle — the dev server reads from this on every
    // HTML response; the runner writes to it on every CSS rebuild tick.
    let css_bundle_url_handle: zfb_server::CssBundleUrl =
        Arc::new(std::sync::RwLock::new(None));

    // Step 2: eager initial CSS bundle at boot so the very first page
    // request already carries a `<link rel="stylesheet">` tag.
    // Failures are non-fatal — we warn and let the dev server boot with
    // unstyled HTML. The hot-rebuild path will retry on the next file
    // change.
    let dev_css_url_prefix: String =
        zfb_types::dev_mount_prefix(cfg.base.as_deref()).unwrap_or_default();
    match crate::commands::build::build_default_css_payload(&project_root, &dist_root, &cfg) {
        Ok(Some(payload)) => {
            // Write the bytes to dist so `GET /assets/styles.css` is
            // immediately serveable (unlike islands, the CSS pipeline does
            // not write to disk as a side-effect of building).
            let out_path = dist_root.join(&payload.relative_path);
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&out_path, &payload.bytes).is_ok() {
                let url = if dev_css_url_prefix.is_empty() {
                    payload.stable_url
                } else {
                    format!("{dev_css_url_prefix}{}", payload.stable_url)
                };
                if let Ok(mut guard) = css_bundle_url_handle.write() {
                    *guard = Some(url);
                }
            } else {
                output::warn(
                    "initial CSS bundle: failed to write bytes to dist (no <link> until rebuild)",
                );
            }
        }
        Ok(None) => {
            // Tailwind disabled or no sources. Leave the handle at None.
        }
        Err(err) => {
            output::warn(format!(
                "initial CSS bundle failed (no <link rel=\"stylesheet\"> \
                 will be injected until the next successful rebuild): {err:#}"
            ));
        }
    }

    // Step 3: CssRunner closure — re-invokes the payload builder, writes
    // fresh bytes to disk, and updates the shared URL handle.
    let run_css: Option<CssRunner> = {
        let project_root_for_css = project_root.clone();
        let dist_root_for_css = dist_root.clone();
        let cfg_for_css = cfg.clone();
        let url_prefix = dev_css_url_prefix.clone();
        let url_handle = Arc::clone(&css_bundle_url_handle);
        Some(Arc::new(move || -> Result<bool> {
            let payload = crate::commands::build::build_default_css_payload(
                &project_root_for_css,
                &dist_root_for_css,
                &cfg_for_css,
            )?;
            let mut guard = url_handle.write().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "dev.run_css.url_handle",
                    "rwlock poisoned, recovered"
                );
                p.into_inner()
            });
            let Some(payload) = payload else {
                // CSS disabled / no sources this tick — clear the URL so
                // subsequent HTML responses don't reference a stale file.
                *guard = None;
                return Ok(false);
            };
            // Write fresh bytes to dist so the dev server serves them
            // immediately. This is the "freshness proof" the acceptance
            // test checks (byte-for-byte match between payload.bytes and
            // GET /assets/styles.css).
            let out_path = dist_root_for_css.join(&payload.relative_path);
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&out_path, &payload.bytes) {
                tracing::warn!("css runner: failed to write bytes to dist: {err}");
                return Ok(false);
            }
            let bundle_url = if url_prefix.is_empty() {
                payload.stable_url
            } else {
                format!("{url_prefix}{}", payload.stable_url)
            };
            *guard = Some(bundle_url);
            // Return true unconditionally on a successful emit so the
            // orchestrator marks outcome.css_changed = true and the
            // livereload SSE event fires. The URL is stable so the bytes
            // update in place on disk.
            Ok(true)
        }))
    };

    // Issue #807 — build the live SSR routes handle here, before the
    // reload_renderer closure, so the closure can hold a clone of the
    // Arc and update it on each tick. The dispatcher is built once and
    // shared across all refreshes (the Arc<Mutex<Option<RendererState>>>
    // it wraps is the same one the refresh swaps the new host into).
    let ssr_route_set = build_ssr_route_set(dev_session.as_ref());

    // Per-tick bundle refresh for EDIT ticks. Without this the renderer
    // stays bound to the boot-time bundle: the content snapshot and page
    // modules are baked in at bundle time, so an in-place save
    // (`ChangeKind::Modified` — what VS Code and most editors emit)
    // re-rendered byte-identical stale HTML forever. Only rename-replace
    // saves worked, by accident, through the watch-ADD discovery path.
    // The pipeline skips this when the tick's plan is already
    // renderer-fresh (boot initial render, watch-ADD discovery
    // re-bundle), so each tick bundles at most once.
    let reload_renderer: Option<RendererReloader> = dev_session.as_ref().map(|session| {
        let session = session.clone();
        let html_root = dev_html_root.clone();
        // Clone the handle so the closure can write a fresh SsrRouteSet
        // into it after each bundle refresh (issue #807). `None` when
        // the renderer is disabled (no SSR in this project).
        let ssr_handle_for_reload = ssr_route_set.clone();
        Arc::new(move || {
            let (changed, vanished_rel) = session
                .refresh_bundle_and_routes()
                .context("edit-tick bundle refresh failed")?;
            if !changed.is_empty() {
                tracing::debug!(
                    count = changed.len(),
                    "edit-tick refresh changed route sets"
                );
            }
            // Issue #807 — update the live SSR route set so newly-added or
            // removed `prerender = false` routes are visible to the request
            // dispatcher on the next request, without a dev-server restart.
            if let Some(handle) = &ssr_handle_for_reload {
                refresh_live_ssr_routes(&session, handle);
            }
            // Convert relative vanished output paths to absolute dist paths
            // so DevAssetPipeline can delete them directly.
            let vanished_abs: Vec<std::path::PathBuf> = vanished_rel
                .into_iter()
                .map(|rel| html_root.join(rel))
                .collect();
            if !vanished_abs.is_empty() {
                tracing::debug!(
                    count = vanished_abs.len(),
                    "edit-tick refresh found globally-vanished routes"
                );
            }
            Ok(vanished_abs)
        }) as RendererReloader
    });

    let ctx = BuildContext {
        // Issue #534 — `DevAssetPipeline::apply` writes
        // `RenderedPage.html` to `<ctx.dist_root>/<output_path>` on every
        // watcher tick (see `crates/zfb-build/src/pipeline/dev.rs`). This
        // is the second HTML writer in the dev pipeline (the first being
        // the renderer itself, redirected above via `make_render_callback`).
        // Both must point at `dev_html_root` for dev to fully stop touching
        // the project's `outDir`; redirecting only the renderer leaves the
        // pipeline's downstream write to clobber `dist/` again. Note that
        // dev's `BuildContext` is only consumed by `DevAssetPipeline`, so
        // this swap is local to dev — production builds use a separate
        // `BuildContext` constructed in `commands::build`.
        dist_root: dev_html_root.clone(),
        render_pages,
        run_css,
        run_islands,
        reload_renderer,
    };

    // 3b. Eager initial render (zfb#642 / #644).
    //
    // `BuildOrchestrator::run` is purely watcher-driven — it renders a
    // page only after a file-change event. Nothing else populates the
    // dev page cache: the in-memory `PageCache` starts empty and the dev
    // server's only HTML source is the on-disk `read_from_dist` fallback
    // pointed at `dev_html_root` (issue #534). So without an eager render
    // here, a fresh `zfb dev` leaves `.zfb-build/dev-pages/` empty and
    // 404s EVERY route until the user happens to edit a file. (Before
    // #534 the fallback read `dist/`, which a prior `pnpm build` had
    // populated, masking the gap.)
    //
    // Run the initial full render NOW — synchronously, before the watcher
    // loop is spawned and before `output::ready` announces the server —
    // so `dev-pages/` is populated before the server can serve a single
    // request. Mirrors the eager CSS / islands boot bundles above. Going
    // through the orchestrator/pipeline (not the raw render callback) also
    // primes `DevAssetPipeline.last_bytes` so the first real edit dedups
    // correctly. A render error here is fatal: the user would otherwise
    // stare at a wall of 404s with no clue why.
    match orchestrator.initial_build(&ctx) {
        Ok(Some(outcome)) => {
            let expected_routes = dev_session.as_ref().map(|s| s.route_count()).unwrap_or(0);
            // Surface the previously-silent zero-page failure (zfb#642):
            // the renderer knows about routes, yet produced no HTML. Every
            // route would 404. Make it visible on stderr instead.
            if expected_routes > 0 && outcome.pages_rendered == 0 {
                output::error(format!(
                    "dev initial render produced 0 pages for {expected_routes} known route(s) — \
                     every route will 404. This usually means the renderer failed silently; \
                     check the bundler / runtime output above."
                ));
            }
        }
        Ok(None) => {
            // No pages in the graph at all (renderer disabled or a
            // project with zero SSG routes). The dev server still boots so
            // the user can poke at it / fix the project; SSR-only routes
            // still work via the request-time path.
        }
        Err(err) => {
            output::error(format!(
                "dev initial render failed — every route will 404 until the next \
                 successful rebuild: {err:#}"
            ));
        }
    }

    // 4. on_outcome — translate each tick into reload events.
    let tx_cb = tx.clone();
    let on_outcome = move |outcome: &BuildOutcome| {
        for ev in outcome_to_events(outcome) {
            let _ = tx_cb.send(ev);
        }
    };

    // 5. Spawn the orchestrator's watcher loop.
    //
    // Issue #659 — `discover_hook` makes a content file CREATED after
    // boot discoverable without a `zfb dev` restart: it rebundles the
    // content snapshot, reloads the embedded V8 host in place, re-expands
    // `paths()`, and rebuilds the dev session's source→route table. Built
    // from `dev_session` (the V8-backed renderer); `None` when the
    // renderer is disabled, which keeps the legacy add-needs-restart
    // behaviour.
    let discover_hook: Option<zfb_build::DiscoveryHook> = dev_session
        .as_ref()
        .map(|session| {
            make_discovery_hook(
                session.clone(),
                Arc::clone(&graph_for_save),
                dev_html_root.clone(),
                // Issue #807 — clone the live handle so the discovery hook can
                // rewrite it on a watch-ADD tick (the pipeline skips
                // reload_renderer when the discovery refresh marked the
                // renderer fresh).
                ssr_route_set.clone(),
            )
        });
    let orch_handle = tokio::spawn(async move {
        if let Err(err) = orchestrator.run(ctx, discover_hook, on_outcome).await {
            output::error(format!("build orchestrator stopped: {err:#}"));
        }
    });

    // 6. Build the serve options and announce readiness.
    //
    // Issue #229: thread `cfg.base` through so the dev server mounts
    // pages, assets, and live-reload under the same prefix the build
    // pipeline stamps onto asset URLs. Without this the dev HTML emits
    // `<link href="/<base>/assets/styles.css">` while the dev server
    // only knew about unprefixed `/assets/...` — every request 404s.
    // Issue #367 / #807 — `ssr_route_set` was built before the
    // reload_renderer closure above so the closure holds a clone of the
    // Arc and can update it on each tick (making added/removed
    // `prerender = false` routes visible without a restart).

    let opts = ServeOpts {
        project_root,
        dist_root,
        // Issue #534 — point the page-cache disk fallback at the dev
        // HTML dir, not the project's `outDir`. With `dist_root` here
        // (the historical wiring) the dev server's `read_from_dist`
        // would serve whatever `pnpm build` last wrote, hiding the
        // dev pipeline's edits because nothing populates
        // `PageCache` for HTML at runtime. Pairing the read with the
        // write side keeps every dev tick observable in the browser.
        html_root: dev_html_root.clone(),
        public_root,
        addr,
        pages,
        broadcast: tx,
        plugins: plugin_set,
        injected_routes: injected_route_set,
        ssr_routes: ssr_route_set,
        base: cfg.base.clone(),
        trailing_slash: cfg.trailing_slash,
        mode: zfb_server::ServerMode::Dev,
        // Issue #377: the dev server holds the same Arc as the
        // `run_islands` callback above; rebuild ticks rewrite the
        // inner Option, page responses read it on every served HTML
        // request. Cloning the Arc is cheap (refcount bump).
        islands_bundle_url: Some(Arc::clone(&islands_bundle_url_handle)),
        // Issue #494 / #498: same pattern as islands — the dev server
        // holds the same Arc as the `run_css` callback above; CSS
        // rebuild ticks rewrite the inner Option, page responses read it
        // on every served HTML request.
        css_bundle_url: Some(Arc::clone(&css_bundle_url_handle)),
    };

    // 7. Bind the TCP listener first so the port-in-use error surfaces
    //    before the ready banner is printed. If bind fails here we exit
    //    with an error and no banner — which is the correct ordering.
    let listener = match TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind dev server to {addr}"))
    {
        Ok(l) => l,
        Err(e) => {
            orch_handle.abort();
            return Err(e);
        }
    };

    output::ready_with_interfaces("http", &host, port);

    // Run the server until Ctrl+C. Pass Ctrl+C as the graceful-shutdown
    // signal so axum drains in-flight connections before exiting. The
    // renderer guard tears down on drop here — the explicit `shutdown`
    // call belt-and-braces keeps the surface symmetrical (start ↔ shutdown).
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let result = tokio::select! {
        res = serve_with_listener(opts, listener, ctrl_c) => {
            orch_handle.abort();
            res
        }
    };

    if let Some(session) = dev_session {
        session.shutdown_explicit();
    }

    // Tear down the plugin host before exit so the Node
    // subprocess doesn't outlive `zfb dev`. Best-effort: a kill via
    // `kill_on_drop` covers the panic / Ctrl+C path; the explicit
    // shutdown is the graceful one.
    if let Some(h) = plugin_host {
        let _ = h.shutdown().await;
    }

    // Persist the graph one more time before exit so the latest
    // populated state — not just the boot-time fresh one — is what
    // the next cold start sees. Best-effort; warn-and-ignore on
    // failure (don't block shutdown on a disk error).
    if let Some(d) = manifest_digest.as_ref() {
        if let Ok(g) = graph_for_save.lock() {
            if let Err(err) = save_to_disk(&g, d, &graph_cache_path) {
                output::warn(format!(
                    "graph persistence: shutdown write to {} failed (ignored): {err:#}",
                    graph_cache_path.display()
                ));
            }
        }
    }

    result
}

/// V8-off stub for `zfb dev` (issue #371, sub-task 4.1a).
///
/// `zfb dev` needs the embedded V8 host to render pages. When the
/// `embed_v8` cargo feature is off this binary was built without that
/// host, so we surface a clear error at the call site instead of
/// compiling a partial pipeline that would silently hand back empty
/// pages.
#[cfg(not(feature = "embed_v8"))]
pub async fn run(_args: &DevArgs) -> Result<()> {
    anyhow::bail!(
        "zfb was built without V8 support (`--no-default-features` / \
         `embed_v8 = off`); `zfb dev` requires the embedded V8 host to \
         render pages. Rebuild with default features (`cargo build`) \
         or with `--features embed_v8` to enable this command."
    )
}

/// Compute the manifest digest for the current project, or return
/// `None` if the digest itself could not be computed (e.g. permission
/// denied while walking sources). On `None` the caller should bypass
/// the persistence layer entirely — never falsely reuse a stale
/// graph.
fn compute_manifest_digest(project_root: &Path, watch_roots: &[PathBuf]) -> Option<ManifestDigest> {
    // Config files that, when changed, must invalidate the graph
    // even though they live next to (not under) the watched roots.
    // Both JSON and TS are listed; missing ones are silently
    // skipped by the digest builder.
    let cfg_files = [
        PathBuf::from("zfb.config.json"),
        PathBuf::from("zfb.config.ts"),
    ];
    match ManifestDigest::compute(project_root, watch_roots, &cfg_files) {
        Ok(d) => Some(d),
        Err(err) => {
            output::warn(format!(
                "graph persistence: manifest digest failed (cache disabled): {err:#}"
            ));
            None
        }
    }
}

/// Try to reuse a persisted graph. Returns `Some(graph)` only when
/// the on-disk file exists and its digest matches the live one. All
/// other outcomes (no digest, missing file, mismatch, IO error) map
/// to `None` so the caller falls back to a fresh graph.
fn load_persisted_graph(
    graph_cache_path: &Path,
    digest: Option<&ManifestDigest>,
) -> Option<DependencyGraph> {
    let d = digest?;
    match load_from_disk(graph_cache_path, d) {
        Ok(Some(g)) => Some(g),
        Ok(None) => None,
        Err(err) => {
            output::warn(format!(
                "graph persistence: load from {} failed (ignored): {err:#}",
                graph_cache_path.display()
            ));
            None
        }
    }
}

/// Canonicalise each entry in `cfg.extra_watch_paths`, dropping any
/// entry that does not exist at boot with a user-visible warning.
///
/// The config loader already enforces "absolute path"; this function
/// implements the rest of the documented contract:
///
/// - canonicalise (`Path::canonicalize`) — the watcher's events will
///   match the canonical form, so downstream rebuild logic compares
///   like for like.
/// - skip-with-warning when the path does not exist. We do NOT
///   re-watch later if it appears — the documented escape hatch is
///   "restart `zfb dev`".
///
/// Canonicalisation also implicitly resolves symlinks; that is the
/// behaviour we want for events-match-on-canonical-path. The dev
/// command boots once per session, so this runs exactly once per
/// configured entry.
fn resolve_extra_watch_paths(raw: &[PathBuf]) -> Vec<PathBuf> {
    let mut resolved = Vec::with_capacity(raw.len());
    for p in raw {
        match p.canonicalize() {
            Ok(c) => resolved.push(c),
            Err(err) => {
                output::warn(format!(
                    "extraWatchPaths: skipping {} (canonicalize failed: {}); \
                     the path will NOT be re-watched if it appears later — \
                     restart `zfb dev` after creating it",
                    p.display(),
                    err,
                ));
            }
        }
    }
    resolved
}

// ---------------------------------------------------------------------------
// Dev islands chunk helpers
// ---------------------------------------------------------------------------

/// Write new chunk files into `assets_dir`, delete chunks from the previous
/// generation that are no longer in the new set, and return the new set.
///
/// `assets_dir` is the on-disk `<dist_root>/assets/` directory that the dev
/// server already serves via ServeDir.  Because chunks land in that directory
/// under their self-hashed basenames (e.g. `islands-chunk-WOEGGERP.js`), the
/// entry's baked-in relative `import("./islands-chunk-WOEGGERP.js")` resolves
/// without any additional routing code.
///
/// Errors writing a chunk are returned immediately (callers treat them as
/// non-fatal at the boot path, fatal at the watcher tick path).  Errors
/// deleting stale chunks are logged and ignored — a stale file that fails to
/// delete is preferable to aborting the rebuild loop.
fn refresh_dev_island_chunks(
    assets_dir: &Path,
    companions: &[zfb_build::pipeline::CompanionFile],
    prev_filenames: &HashSet<String>,
) -> anyhow::Result<HashSet<String>> {
    let new_filenames: HashSet<String> =
        companions.iter().map(|c| c.filename.clone()).collect();

    // Write each new chunk file. The entry was already written by the bundler
    // as a side effect of `bundle()`; these are the code-split companions.
    for companion in companions {
        if companion.filename.is_empty()
            || companion.filename.contains('/')
            || companion.filename.contains('\\')
            || companion.filename.contains("..")
        {
            anyhow::bail!(
                "dev islands: chunk filename {:?} must be a flat basename \
                 (no path separator or `..`)",
                companion.filename
            );
        }
        let dest = assets_dir.join(&companion.filename);
        std::fs::write(&dest, &companion.bytes).with_context(|| {
            format!(
                "dev islands: failed to write chunk file {}",
                dest.display()
            )
        })?;
    }

    // Prune stale chunk files from the previous generation.
    for stale in prev_filenames.difference(&new_filenames) {
        let path = assets_dir.join(stale);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "dev islands: failed to delete stale chunk (ignored)"
                );
            }
        }
    }

    Ok(new_filenames)
}

// ---------------------------------------------------------------------------
// Renderer plumbing
// ---------------------------------------------------------------------------

/// Long-lived dev-session state that owns the renderer subprocess and
/// the route table. Cloned by the [`PageRenderer`] callback so each
/// orchestrator tick can map page ids → URLs.
#[derive(Clone)]
struct DevRenderSession {
    inner: Arc<DevRenderInner>,
}

/// The dev session's source→route + SSR route tables.
///
/// Issue #659 — wrapped in a single [`RwLock`] inside [`DevRenderInner`]
/// so a content file CREATED after boot can rebuild them in place (the
/// running render callback / SSR adapter read the SAME tables via the
/// shared `Arc<DevRenderInner>`). The EDIT path never mutates these — it
/// re-renders against the frozen boot tables exactly as before.
struct DevRouteTables {
    /// Mapped from the page module's project-relative source path
    /// (which is what the dependency graph keys on) to the renderer
    /// entries. Seeded at boot from the router scan; rebuilt on a
    /// watch-ADD (#659).
    ///
    /// Issue #367: only pages with `prerender != false` are kept
    /// here. Pages that opted out of SSG go into `ssr_routes`
    /// instead and reach the V8 host at request time.
    ///
    /// Issue #502/#507: the value is a `Vec` because one dynamic SSG
    /// source (`pages/blog/[slug].tsx`) expands via `paths()` into N
    /// concrete URLs that all share the same source `PageId`. A static
    /// route resolves to a single-element vec; a dynamic SSG route
    /// resolves to one entry per `paths()` URL. `render_one` emits one
    /// `RenderedPage` per entry on each tick so every fanned-out URL
    /// reaches `dist/` (and thus the dev server's disk fallback).
    routes_by_source: HashMap<PathBuf, Vec<RouteUniverseEntry>>,
    /// URL patterns for `prerender = false` pages (issue #367 /
    /// Gap 1). Empty when every page in the project SSGs. The dev
    /// server reads this list (via [`DevRenderSession::ssr_patterns`])
    /// and builds an [`zfb_server::SsrRouteSet`] from it.
    ssr_routes: Vec<RouteUniverseEntry>,
}

/// Boot-time inputs stashed so a watch-ADD (#659) can re-bundle the SSR
/// worker with a fresh content snapshot and reload the V8 host in place.
///
/// Only compiled in on the V8 path — the discovery hook that consumes
/// these is `embed_v8`-gated like `boot_dev_renderer`.
#[cfg(feature = "embed_v8")]
struct DevRebuildInputs {
    cfg: config::Config,
    v8_plugin_hooks: zfb_render::PluginRegistryHooks,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
}

struct DevRenderInner {
    /// Source→route + SSR route tables (issue #659 — interior-mutable so
    /// a watch-ADD rebuilds them in place; see [`DevRouteTables`]).
    routes: std::sync::RwLock<DevRouteTables>,
    /// Mutex-wrapped renderer state. The orchestrator's callback runs
    /// on the watcher's thread; render_one is sync and short, so a
    /// global lock is fine here.
    ///
    /// Wrapped in an outer Arc (in addition to the surrounding
    /// `Arc<DevRenderInner>`) so the SSR adapter (#367) can hold a
    /// separate handle into the same V8 host without taking a
    /// dependency on `DevRenderInner` itself. The adapter clones
    /// this Arc once at construction and goes through it via
    /// `spawn_blocking` per request.
    renderer: Arc<Mutex<Option<RendererState>>>,
    /// Project root. Passed to `render_one` so it can locate the source
    /// file for static-HTML routes (#409).
    project_root: PathBuf,
    /// Boot-time bundle inputs for the watch-ADD re-bundle + host reload
    /// (issue #659). `None` would mean "no discovery" but boot always
    /// populates it on the V8 path.
    #[cfg(feature = "embed_v8")]
    rebuild_inputs: DevRebuildInputs,
}

impl DevRenderSession {
    /// Drive a single source page id against the renderer. Returns one
    /// [`RenderedPage`] per [`RouteUniverseEntry`] mapped to this source
    /// path, each populated with the bytes the renderer just wrote, so
    /// the dev pipeline's atomic-write + cache layer can fold the result
    /// through the existing reload broadcast.
    ///
    /// A static route maps to a single entry; a dynamic SSG route
    /// (`pages/blog/[slug].tsx`) maps to N entries — one per concrete URL
    /// its `paths()` resolved to (issue #502/#507). Returns an empty Vec
    /// when the source path is unknown to the renderer (dynamic route
    /// deferred to SSR, or a page never seen by the router scan).
    fn render_one(&self, page: &PageId, dist_dir: &Path) -> Result<Vec<RenderedPage>> {
        // Read the (possibly watch-ADD-rebuilt, #659) route table. Clone
        // the entries out so the lock is released before the V8 render —
        // the reload path takes the write lock, and `render_one` may run
        // on the same tick that just rebuilt the table.
        let entries = match self
            .inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .routes_by_source
            .get(page.path())
        {
            Some(es) => es.clone(),
            None => return Ok(Vec::new()),
        };
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        let state = lock
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("renderer not started"))?;
        let project_root = self.inner.project_root.clone();
        // Delegate the loop to `render_one_with`, injecting the real V8
        // render call as the per-entry closure. This keeps the fan-out logic
        // testable without a live renderer (see the seam test below).
        Self::render_one_with(page, &entries, |entry| {
            render_one(state, entry, dist_dir, &project_root).map_err(anyhow::Error::from)
        })
    }

    /// Fan-out loop: for each `RouteUniverseEntry` in `entries`, call
    /// `render_entry` to produce the path of the written HTML file, read
    /// it back, and wrap it in a `RenderedPage` with a synthetic `PageId`
    /// derived from the entry's `output_path`.
    ///
    /// Extracted from `render_one` so the per-entry render step can be
    /// replaced with a stub in tests, avoiding the need for a live V8
    /// `RendererState`. The public `render_one` delegates here with the
    /// real `zfb_build::renderer::render_one` call as the closure.
    fn render_one_with<F>(
        page: &PageId,
        entries: &[RouteUniverseEntry],
        mut render_entry: F,
    ) -> Result<Vec<RenderedPage>>
    where
        F: FnMut(&RouteUniverseEntry) -> Result<PathBuf>,
    {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let written = render_entry(entry)?;
            let html = std::fs::read_to_string(&written)
                .with_context(|| format!("failed to read rendered page {}", written.display()))?;
            // RouteUniverseEntry::output_path is a PathBuf validated by the
            // router/render_pipeline (relative, no escapes). Wrap it in
            // RelDistPath for the pipeline's type contract. If the path is
            // somehow invalid, surface an error rather than silently skipping.
            let output_path = RelDistPath::new(entry.output_path.clone())
                .with_context(|| format!("renderer returned invalid output_path for {:?}", page))?;
            // Guardrail 1 (#507): `DevAssetPipeline.last_output_path` keys on
            // the `RenderedPage.page` id and prunes the previous artifact
            // when a page's output path changes. N URLs that share one source
            // `PageId` would collide on that key — the pipeline would prune
            // all but the last fanned-out URL each tick, so only one dynamic
            // page would survive on disk. Give each resolved URL a distinct,
            // deterministic synthetic `PageId` derived from its output path so
            // every URL gets its own pipeline bookkeeping slot (stable across
            // ticks → byte-dedup still works). The synthetic id is internal to
            // the pipeline's per-tick bookkeeping; the dependency graph keys
            // on real source paths only (see `page_ids`).
            //
            // Coupling landmine (read before adding watch-time router re-scan):
            // every route — static included — now keys `RenderedPage.page` on
            // `output_path`, not the source path. The pipeline's stale-output
            // prune (DevAssetPipeline.last_output_path) was designed around a
            // *stable source id* whose output_path can flip across ticks (e.g.
            // sitemap.xml → sitemap.rss); output-path keying makes such a flip
            // produce two distinct keys, so the old artifact would orphan in
            // dist/. Still unreachable after #659 (which added live table
            // rebuilding in `discover_created`): that function's diff gate
            // (lines ~1289-1301) only re-renders sources whose entry COUNT
            // changed (`prev.len() != entries.len()`), so an output_path flip
            // on a stable-count source is never re-rendered/pruned and the
            // orphan path is never triggered. If entry-count-stable output_path
            // flips must be handled, restore source-path keying for static
            // routes (or key dynamic entries on (source_path, output_path)).
            out.push(RenderedPage {
                page: PageId::new(entry.output_path.clone()),
                output_path,
                html,
                content_type: None,
            });
        }
        Ok(out)
    }

    /// Clone the shared handle to the embedded V8 host so the SSR
    /// adapter can dispatch requests through the same renderer state
    /// that drives build-time SSG (#367). Cheap — the underlying
    /// renderer is wrapped in an `Arc<Mutex<...>>` already.
    fn renderer_handle(&self) -> Arc<Mutex<Option<RendererState>>> {
        Arc::clone(&self.inner.renderer)
    }

    /// Return the URL patterns of every page that exports
    /// `prerender = false` (issue #367 / Gap 1). Used by the dev
    /// command to build the [`zfb_server::SsrRouteSet`] handed to
    /// the dev router.
    ///
    /// `zfb_router::Route::template()` emits Hono-style colon syntax
    /// (`/blog/:slug`, `/docs/:slug{.+}`), but `SsrRouteSet`'s matcher
    /// is `pages/`-style (`/blog/[slug]`, `/docs/[...slug]`). Translate
    /// here so dev SSR matches dynamic-route URLs at all. The two public
    /// grammars (this one via SSR + `with_ssr_handler`'s axum-style via
    /// `embed_handlers`) diverge by historical accident — see the TODO
    /// in `crates/zfb-server/src/embed_handlers.rs` for the unify-later
    /// follow-up.
    fn ssr_patterns(&self) -> Vec<String> {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .ssr_routes
            .iter()
            .map(|e| colon_template_to_bracket(&e.route_key))
            .collect()
    }

    /// Return all known page IDs (source paths) from the router scan.
    /// Used to seed the dependency graph so incremental rebuilds have a
    /// non-empty page set to resolve against.
    fn page_ids(&self) -> Vec<PageId> {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .routes_by_source
            .keys()
            .map(|p| PageId::new(p.clone()))
            .collect()
    }

    /// Number of SSG source routes the renderer knows about. Used by the
    /// eager initial-build step to detect the "0 pages rendered for N
    /// routes" silent-failure case (zfb#642 / #644): if this is non-zero
    /// but the initial render produced nothing, every route would 404.
    fn route_count(&self) -> usize {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .routes_by_source
            .len()
    }

    /// Tear down the underlying [`RendererState`] cleanly. Safe to call
    /// multiple times — subsequent calls are a no-op.
    fn shutdown_explicit(&self) {
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        if let Some(state) = lock.take() {
            let _ = shutdown(state);
        }
    }

    /// Discover content files CREATED after dev-server boot (issue #659).
    ///
    /// Thin gate over [`Self::refresh_bundle_and_routes`] — see there for
    /// what the refresh does. Kept as a named entry point so the
    /// discovery hook's intent stays explicit at the call site.
    ///
    /// Returns the changed source [`PageId`]s and the relative output paths
    /// that vanished from the global live route set. The caller is
    /// responsible for joining the relative paths with the appropriate dist
    /// root before propagating them (issue #804 P2).
    #[cfg(feature = "embed_v8")]
    fn discover_created(
        &self,
        created: &[PathBuf],
    ) -> Result<(Vec<PageId>, Vec<std::path::PathBuf>)> {
        if created.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        self.refresh_bundle_and_routes()
    }

    /// Re-bundle the SSR worker, swap in a freshly-started embedded V8
    /// host, and rebuild the route tables — the dev server's "make the
    /// running renderer see the source tree as it is NOW" primitive.
    ///
    /// The collection snapshot and every page module are baked into the
    /// SSR bundle, and `routes_by_source` is frozen, at whatever point
    /// this was last run (boot, or a previous refresh). Two paths call
    /// it:
    ///
    /// - the watch-ADD discovery hook (issue #659) — a content file like
    ///   `content/blog/foo.mdx` CREATED after boot is invisible to the
    ///   running host (404 until restart) without a refresh;
    /// - the per-tick `BuildContext::reload_renderer` — an in-place EDIT
    ///   (`ChangeKind::Modified`) re-renders pages, but against the
    ///   stale bundle the render output is byte-identical to the boot
    ///   render, so saves from in-place-writing editors (VS Code et al.)
    ///   never showed up. Rename-replace saves only worked by accident,
    ///   via the Created path above.
    ///
    /// Steps:
    /// 1. re-scan the router (cheap; picks up brand-new `pages/` files),
    /// 2. re-bundle with a fresh content snapshot from disk,
    /// 3. START a new embedded V8 host against the new bundle, swap it
    ///    into the SAME `Arc<Mutex<…>>` the render callback and SSR
    ///    adapter hold, and shut the old host down AFTER the swap.
    ///    Start-before-swap is deliberate: the old take→reload→put
    ///    order consumed the previous state up front, so a host-start
    ///    failure left the slot `None` and every later render broke
    ///    until restart. Acceptable when only rare watch-ADDs hit the
    ///    path; fatal now that every edit tick does. (Bundle errors —
    ///    the common failure, e.g. saving a syntax error — abort in
    ///    step 2 before the renderer is touched either way.)
    /// 4. re-expand `paths()` through the new host and swap the rebuilt
    ///    route tables in under the route `RwLock`. Rebuilding on EDIT
    ///    ticks too (not just Created) means frontmatter edits that
    ///    change a dynamic route's `paths()` output surface without a
    ///    restart.
    ///
    /// Returns `(changed_sources, vanished_output_paths)` where:
    /// - `changed_sources`: source [`PageId`]s whose route set changed
    ///   (empty for a plain content edit).
    /// - `vanished_output_paths`: relative output paths (under dist) that
    ///   existed in the old live route set but are absent from the new one,
    ///   globally across all sources. Used by the caller to prune stale
    ///   HTML files and invalidate PageCache entries (issue #804).
    ///
    /// `embed_v8`-gated like `boot_dev_renderer` — the host start +
    /// `paths()` runtime eval need the embedded V8 host.
    #[cfg(feature = "embed_v8")]
    fn refresh_bundle_and_routes(&self) -> Result<(Vec<PageId>, Vec<std::path::PathBuf>)> {
        let project_root = &self.inner.project_root;
        let inputs = &self.inner.rebuild_inputs;

        // Re-scan the router. `Router::scan` is unchanged by adding a
        // CONTENT file (the dynamic `[slug].tsx` source is the same), but
        // re-running it is cheap and keeps boot and rebuild symmetrical —
        // and it correctly picks up a brand-new `.tsx`/`.md` page placed
        // directly under `pages/` too.
        let pages_dir = project_root.join("pages");
        let router = zfb_router::Router::scan(&pages_dir).map_err(anyhow::Error::from)?;
        let plan = build_route_universe(router.routes());

        // 1. Re-bundle with a fresh content snapshot (reads every
        //    configured collection from disk, so created AND edited
        //    entries are in the snapshot).
        let bundler_out = assemble_and_bundle_dev(
            project_root,
            &inputs.cfg,
            inputs.plugin_alias_entries.clone(),
            inputs.plugin_virtual_modules.clone(),
        )
        .context("dev refresh: re-bundle failed")?;

        // 2. Start a NEW embedded V8 host against the rebuilt bundle,
        //    swap it into the existing mutex (the render callback + SSR
        //    adapter share this exact Arc), and shut the old host down
        //    only after the swap — a host-start failure must leave the
        //    previous renderer serving (see the method docs).
        {
            let started = start(RendererStartInput {
                bundle_path: bundler_out.bundle_path.clone(),
                sourcemap_path: bundler_out.sourcemap_path.clone(),
                backend: Backend::EmbeddedV8 {
                    host_factory: crate::v8_host_adapter::make_v8_host_factory_with_hooks(
                        inputs.v8_plugin_hooks.clone(),
                    ),
                },
                request_timeout: None,
            })
            .map_err(anyhow::Error::from)
            .context("dev refresh: renderer start failed (previous renderer kept serving)")?;
            let previous = {
                let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
                    tracing::warn!(
                        site = "refresh_bundle_and_routes",
                        "renderer mutex poisoned, recovered"
                    );
                    p.into_inner()
                });
                lock.replace(started)
            };
            if let Some(prev) = previous {
                if let Err(err) = shutdown(prev) {
                    tracing::warn!(
                        site = "refresh_bundle_and_routes",
                        error = %err,
                        "old renderer shutdown failed; continuing with new host"
                    );
                }
            }
        }

        // 3. Rebuild the route tables through the reloaded host (re-expands
        //    `paths()`, so the dynamic source now resolves the new URL).
        let (new_routes_by_source, new_ssr_routes) =
            build_dev_route_tables(&router, &plan, project_root, &self.inner.renderer)
                .context("dev refresh: route-table rebuild failed")?;

        // 4. Diff against the frozen table to find:
        //    (a) which source pages gained/changed entries, and
        //    (b) which output paths vanished globally (were live before
        //        but are absent from every source in the new table).
        //    The global diff is critical: if route A loses /x while route B
        //    simultaneously gains /x, /x must NOT be considered vanished.
        let (changed, vanished_output_paths) = {
            let old = self.inner.routes.read().unwrap_or_else(|p| p.into_inner());

            let changed: Vec<PageId> = new_routes_by_source
                .iter()
                .filter(|(src, entries)| {
                    old.routes_by_source
                        .get(*src)
                        .map(|prev| prev.len() != entries.len())
                        .unwrap_or(true)
                })
                .map(|(src, _)| PageId::new(src.clone()))
                .collect();

            // Collect the globally-live output_path sets for old and new.
            // Use HashSet for O(1) membership checks.
            let old_live: std::collections::HashSet<std::path::PathBuf> = old
                .routes_by_source
                .values()
                .flat_map(|entries| entries.iter().map(|e| e.output_path.clone()))
                .collect();
            let new_live: std::collections::HashSet<std::path::PathBuf> = new_routes_by_source
                .values()
                .flat_map(|entries| entries.iter().map(|e| e.output_path.clone()))
                .collect();

            let vanished: Vec<std::path::PathBuf> = old_live
                .difference(&new_live)
                .cloned()
                .collect();

            (changed, vanished)
        };
        {
            let mut tables = self.inner.routes.write().unwrap_or_else(|p| p.into_inner());
            tables.routes_by_source = new_routes_by_source;
            tables.ssr_routes = new_ssr_routes;
        }

        Ok((changed, vanished_output_paths))
    }
}

impl Drop for DevRenderInner {
    fn drop(&mut self) {
        // Defence-in-depth: even if `shutdown_explicit` was missed
        // (panicking dev loop, early ?-return), the inner Arc's drop
        // tears the subprocess down. Errors from shutdown are
        // swallowed here — the process is already going away.
        if let Ok(mut g) = self.renderer.lock() {
            if let Some(state) = g.take() {
                let _ = shutdown(state);
            }
        }
    }
}

/// Assemble the dev-mode bundler input and run the bundler, returning the
/// fresh [`BundlerOutput`] (issue #659 — extracted from `boot_dev_renderer`
/// so the watch-ADD re-bundle reuses the EXACT same configuration the boot
/// bundle used; any drift here would make a newly-added page render
/// differently in dev than it did at boot). The embedded node_modules /
/// esbuild tempdir handles live only for the synchronous `bundle()` call
/// (which writes `bundle_path` to disk), so scoping them to this function
/// is correct.
///
/// `recompute snapshot` is implicit: `build_content_snapshot_json` re-reads
/// the content collections from disk on every call, so a re-bundle here
/// picks up a content file created after boot.
#[cfg(feature = "embed_v8")]
fn assemble_and_bundle_dev(
    project_root: &Path,
    cfg: &config::Config,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
) -> Result<BundlerOutput> {
    // Embed the content snapshot so a page's `getStaticProps()` (and any
    // runtime `paths()`) sees the same collection data the production
    // build does. The published `zfb/content` `getCollection(...)` reads
    // `globalThis.__zfb.contentSnapshot`, which `createPageRouter`
    // installs from this JSON at worker boot. Without it, dev resolves
    // against the bundler's placeholder empty snapshot and every
    // `getCollection(...)` returns `[]` — the symptom in #493 where the
    // scaffolded homepage rendered an empty post list under `zfb dev`
    // even though `zfb build` produced the full list. Built via the same
    // `build_content_snapshot_json` helper `zfb build` uses, so dev and
    // build stay byte-identical (returns `None` when no collections are
    // declared, matching the previous behaviour for collection-less
    // projects).
    let content_snapshot_json =
        crate::commands::build::build_content_snapshot_json(project_root, cfg);

    // The intermediate `.zfb-build/` directory lives at
    // `<project_root>/.zfb-build/`, NOT under `<dist_root>/`. This
    // mirrors the production path in `commands/build.rs` (see zfb#231).
    // Dev mode's leak risk is lower (the dev server doesn't ship dist/
    // anywhere), but keeping the path consistent with build means the
    // intermediate is always at one well-known location regardless of
    // mode, and any tooling pointing at it stays mode-agnostic.
    let mut bundler_input = BundlerInput::for_project(
        project_root.to_path_buf(),
        cfg_framework_to_render(cfg.framework),
        BundleMode::Development,
        project_root.join(".zfb-build"),
        content_snapshot_json,
    );
    // Discover the Next-style root `mdx-components.tsx` convention (#616),
    // mirroring `commands/build.rs`. Re-runs every bundle (shadow is a fresh
    // tempdir), so `zfb dev` / preview pick up edits to the file with no
    // special-casing. `None` when absent ⇒ output identical to no-file.
    bundler_input.mdx_components_file =
        crate::commands::build::discover_mdx_components_file(project_root);
    // Mirror the production build CLI: surface project-side
    // `node_modules/` and `tsconfig.json#compilerOptions.paths` to the
    // bundler so esbuild can resolve user-installed packages and TS
    // path aliases. Helpers live in `commands/build.rs`; they are
    // intentionally re-used so dev and prod stay in lockstep.
    //
    // When the project has no node_modules (cargo-install scenario), fall
    // back to the binary-embedded @takazudo packages. The `_embedded_nm_handle`
    // keeps the tempdir alive for the duration of the bundle step.
    let _embedded_nm_handle: Option<tempfile::TempDir>;
    if let Some(nm) = crate::commands::build::detect_project_node_modules(project_root) {
        bundler_input.node_modules_dir = Some(nm);
        _embedded_nm_handle = None;
    } else {
        match embedded_node_modules() {
            Ok((handle, nm_path)) => {
                bundler_input.node_modules_dir = Some(nm_path);
                // Vendored / cargo-install mode: mirror
                // `commands/build.rs` — see
                // `BundlerInput::node_modules_preserve_symlinks` for
                // the full rationale (issues #443 / #450).
                bundler_input.node_modules_preserve_symlinks = true;
                _embedded_nm_handle = Some(handle);
            }
            Err(e) => {
                crate::output::warn(format!(
                    "could not extract embedded @takazudo packages ({e}); \
                     falling back to node_modules walk"
                ));
                _embedded_nm_handle = None;
            }
        }
    }
    bundler_input.tsconfig_paths = crate::commands::build::read_tsconfig_paths(project_root);
    // Per-collection content materialisation so dev-mode SSR also
    // installs `globalThis.__zfb.content` and renders MDX bodies as
    // JSX (#506). Mirrors the production-build wiring in build.rs.
    bundler_input.content_collections = cfg
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
    // CSS Modules — mirror the production-build wiring so dev preview
    // resolves `import styles from "./x.module.css"` to the same scoped
    // class names `zfb build` produces. The scoped names are
    // deterministic (both sides use `CssModulesConfig::default()`), so
    // the dev stylesheet and dev-rendered HTML agree. A failure here is
    // non-fatal: log it and continue with empty maps (`.module.css`
    // imports then degrade to `{}` rather than aborting the dev boot).
    bundler_input.css_module_class_maps =
        match crate::commands::build::compute_css_module_class_maps(project_root, cfg) {
            Ok(maps) => maps,
            Err(e) => {
                crate::output::warn(format!(
                    "CSS Modules class-map computation failed ({e}); \
                     `.module.css` imports will resolve to empty maps in dev"
                ));
                std::collections::HashMap::new()
            }
        };

    // Thread the opt-in `stripMdExt` flag through so the dev-mode
    // bundler (which feeds the embedded V8 host) honours the same setting as
    // `zfb build`. The dev loader at `crates/zfb-render/src/loader.rs`
    // also reads this flag for in-process MDX rendering, so dev preview
    // matches built dist (zfb#127 / #129).
    bundler_input.strip_md_ext = cfg.strip_md_ext;
    // Mirror the `commands/build.rs` wiring for `resolveMarkdownLinks`
    // so dev preview rewrites `.mdx` link targets to their final route
    // URLs the same way the production build does (sub #234).
    if let Some(routes) = crate::commands::build::resolve_links_routes_from_config(project_root, cfg) {
        let on_broken_links = match cfg
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
    // syntect theme. Mirrors the `commands/build.rs` wiring.
    bundler_input.code_highlight_theme = cfg.code_highlight.as_ref().and_then(|c| c.theme.clone());
    // Thread `markdown.gfm` and `markdown.cjkFriendly` through to the
    // bundler so dev rendering and the build agree on the parser
    // construct set. Mirrors the `commands/build.rs` wiring.
    // Thread the optional `codeHighlight.themesDir` so dev rendering
    // loads custom .tmTheme files just like the production build.
    // Mirrors the `commands/build.rs` wiring.
    bundler_input.code_highlight_themes_dir = cfg
        .code_highlight
        .as_ref()
        .and_then(|c| c.themes_dir.as_ref())
        .map(|td| project_root.join(td));
    // Thread `markdown.gfm` through to the bundler so dev rendering
    // and the build agree on the parser construct set. Mirrors the
    // `commands/build.rs` wiring.
    bundler_input.gfm_constructs =
        crate::config::resolve_gfm_constructs(cfg.markdown.as_ref());
    // Thread the optional `site` canonical-origin URL so `zfb dev` emits
    // `globalThis.__zfb.site` the same way `zfb build` does. Mirrors the
    // `commands/build.rs` wiring (sub #254).
    bundler_input.site = cfg.site.clone();
    // Thread `prefetch.disabled` so `zfb dev` emits
    // `globalThis.__zfb.prefetchDisabled = true` the same way `zfb build`
    // does. Mirrors the `commands/build.rs` wiring (sub #277).
    bundler_input.prefetch_disabled = cfg
        .prefetch
        .as_ref()
        .and_then(|p| p.disabled)
        .unwrap_or(false);
    bundler_input.toc = cfg.markdown.as_ref().and_then(|m| m.toc.clone());
    // Thread `markdown.externalLinks` through to the bundler so dev
    // rendering matches the production build. Mirrors `commands/build.rs`.
    // `site` (top-level cfg.site, #254) lets `ExternalLinksPlugin`
    // classify same-origin absolute URLs as internal.
    bundler_input.external_links = cfg
        .markdown
        .as_ref()
        .and_then(|m| m.external_links.clone())
        .map(|el| (el.into_content_config(), cfg.site.clone()));
    bundler_input.cjk_friendly =
        crate::config::resolve_cjk_friendly(cfg.markdown.as_ref());
    bundler_input.hard_breaks =
        crate::config::resolve_hard_breaks(cfg.markdown.as_ref());
    // #664 / #672 — thread `bundle.exclude` so `zfb dev` keeps the same
    // project-relative globs out of the esbuild graph as the production build.
    // Empty → skip nothing. Mirrors `commands/build.rs`.
    bundler_input.bundle_exclude =
        crate::config::resolve_bundle_exclude(cfg.bundle.as_ref());
    // #676 -- mirror commands/build.rs: thread `bundle.mainFields` /
    // `bundle.external` so dev's page/SSR bundle resolves (or externalizes)
    // CJS-main-only deps identically to the production build.
    bundler_input.main_fields =
        crate::config::resolve_bundle_main_fields(cfg.bundle.as_ref());
    bundler_input
        .external
        .extend(crate::config::resolve_bundle_external(cfg.bundle.as_ref()));
    // #586 — thread `markdown.features` so dev rendering honours the opt-in
    // feature plugins, matching the production build. Mirrors
    // `commands/build.rs`. `None` keeps the legacy always-on chain.
    bundler_input.markdown_features =
        cfg.markdown.as_ref().and_then(|m| m.features.clone());
    // #268 — thread plugin-registered aliases and virtual modules into the
    // dev-mode bundler's esbuild invocation. Mirrors `commands/build.rs`
    // wiring so `zfb dev` and `zfb build` produce identical alias resolution
    // for pages / layouts / shared SSR-only modules.
    bundler_input.plugin_alias_entries = plugin_alias_entries;
    bundler_input.plugin_virtual_modules = plugin_virtual_modules;
    // Sub #212 follow-up — same embedded-esbuild wiring as
    // `commands/build.rs`. Without this, dev mode would also blow up on
    // consumer projects without `crates/zfb/binaries/esbuild/`.
    let _embedded_esbuild_handle: Option<tempfile::TempDir>;
    if bundler_input.esbuild_binary.is_none() && std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
        match crate::render_pipeline::embedded_binary("esbuild") {
            Ok((handle, path)) => {
                bundler_input.esbuild_binary = Some(path);
                _embedded_esbuild_handle = Some(handle);
            }
            Err(e) => {
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
    let bundler_out: BundlerOutput = bundle(bundler_input).context("bundler step failed")?;
    Ok(bundler_out)
}

/// `(routes_by_source, ssr_routes)` — the pair [`build_dev_route_tables`]
/// produces and [`DevRouteTables`] stores.
#[cfg(feature = "embed_v8")]
type BuiltRouteTables = (HashMap<PathBuf, Vec<RouteUniverseEntry>>, Vec<RouteUniverseEntry>);

/// Build the dev session's source→route + SSR route tables from the router
/// scan + the live V8 host (issue #659 — extracted from `boot_dev_renderer`
/// so boot and the watch-ADD rebuild produce byte-identical tables). The
/// `renderer` mutex must already hold a started/reloaded [`RendererState`]
/// because the dynamic-route `paths()` runtime phase borrows the live
/// embedded V8 host out of it.
#[cfg(feature = "embed_v8")]
fn build_dev_route_tables(
    router: &zfb_router::Router,
    plan: &crate::render_pipeline::RouteUniversePlan,
    project_root: &Path,
    renderer: &Arc<Mutex<Option<RendererState>>>,
) -> Result<BuiltRouteTables> {
    let prerender_map = build_prerender_map(router.routes(), project_root, |msg| {
        crate::output::warn(msg)
    });

    // Build the source-path → entries map once. Router source paths are
    // project-relative; PageId keys on the same value (the orchestrator
    // tracks pages by their source path). Each value is a Vec so a dynamic
    // SSG source can hold its N `paths()`-expanded entries (#502/#507);
    // static routes contribute a single-element vec.
    let mut routes_by_source: HashMap<PathBuf, Vec<RouteUniverseEntry>> = HashMap::new();
    let mut ssr_routes: Vec<RouteUniverseEntry> = Vec::new();
    for route in router.routes() {
        if let Some(entry) = plan
            .static_routes
            .iter()
            .find(|e| e.route_key == route.template())
        {
            // Default to SSG when the prerender map has no entry —
            // delegated to `is_ssr_route` for the single source of truth
            // (matches `crates/zfb/src/commands/build.rs`).
            if crate::render_pipeline::is_ssr_route(&prerender_map, &route.template()) {
                ssr_routes.push(entry.clone());
            } else {
                routes_by_source
                    .entry(route.source_path.clone())
                    .or_default()
                    .push(entry.clone());
            }
        }
    }
    // Deep-review regression (PR #376): also pick up `prerender = false`
    // routes that live in `plan.deferred_dynamic` — dynamic routes whose
    // `paths()` couldn't be statically expanded. Without this, dev SSR
    // would 404 a `[slug]` route with `export const prerender = false`
    // (the SsrRouteSet has no record for it). Mirrors the build-side
    // chain in `crates/zfb/src/commands/build.rs` for `ssr_route_refs` /
    // `ssr_route_keys_for_runtime_bundle`.
    //
    // The deferred entry has no concrete URL — the template IS the most
    // specific identifier we have. We synthesise a RouteUniverseEntry
    // whose URL fields mirror the template; only the route pattern is
    // load-bearing for SsrRouteSet (output_path / url_path are never
    // touched by the SSR dispatch path).
    for deferred in &plan.deferred_dynamic {
        if !crate::render_pipeline::is_ssr_route(&prerender_map, &deferred.template) {
            continue;
        }
        ssr_routes.push(RouteUniverseEntry {
            url_path: deferred.template.clone(),
            output_path: PathBuf::new(),
            route_key: deferred.template.clone(),
            static_html: false,
            source_path: None,
        });
    }

    // Issue #502/#507 — expand `prerender = true` (SSG) dynamic routes so
    // `zfb dev` serves them. `zfb build` does this two-phase expansion;
    // `boot_dev_renderer` never did, so on a fresh scaffold (no prior
    // `dist/` to fall back to) every `[slug]`/`[...tag]` SSG URL 404'd. This
    // mirrors the build path: phase 1 statically extracts literal `paths()`
    // arrays; phase 2 evaluates the remainder at runtime through the same
    // embedded V8 host the SSG renderer already owns (basic-blog's `paths()`
    // resolves slugs from `getCollection()`, which only the runtime path
    // can answer).
    //
    // SSR (`prerender = false`) dynamic routes are deliberately excluded
    // here — they were already routed into `ssr_routes` above and must reach
    // the V8 host at request time, NOT be SSG'd to disk (a disk artifact
    // would shadow the SSR handler). We collect only the SSG-eligible
    // deferred routes for expansion.
    //
    // Scope (#507): this expands `paths()` once, at dev-server boot. Live
    // re-expansion when collection content is added/removed during a running
    // dev session (watch-time `paths()` invalidation) is OUT of scope and is
    // a documented follow-up — a content edit today requires a `zfb dev`
    // restart to pick up new/removed dynamic URLs.
    let ssg_deferred: Vec<crate::render_pipeline::PendingDynamicRoute> = plan
        .deferred_dynamic
        .iter()
        .filter(|d| !crate::render_pipeline::is_ssr_route(&prerender_map, &d.template))
        .cloned()
        .collect();
    if !ssg_deferred.is_empty() {
        // Map each route template back to its source path so the resolved
        // (concrete-URL) entries — whose `source_path` is `None` and whose
        // `route_key` is the template — can be filed under the right source
        // `PageId` in `routes_by_source`. The dependency graph and the
        // watcher key on source paths, so the fan-out must land under the
        // dynamic source's path, not under each concrete URL.
        let template_to_source: HashMap<String, PathBuf> = ssg_deferred
            .iter()
            .map(|d| (d.template.clone(), d.source_path.clone()))
            .collect();

        let mut paths_cache = PathsCache::new();

        // Phase 1 — literal `paths()` arrays (no runtime needed).
        // A missing `paths()` export on an SSG route is a hard error here
        // too — consistent with `zfb build` (issue #520).
        let static_expansion =
            expand_dynamic_routes(&ssg_deferred, project_root, &mut paths_cache)?;

        // Phase 2 — evaluate the routes phase 1 couldn't resolve statically
        // through the running embedded V8 host. We borrow the live host out
        // of the dev session's renderer mutex (the same `Arc<Mutex<Option<
        // RendererState>>>` the SSG render callback and the SSR adapter
        // share) and dispatch via `WorkerDispatch::EmbeddedV8`, exactly like
        // `commands/build.rs::eval_deferred_paths`.
        let runtime_expansion = {
            let mut lock = renderer.lock().unwrap_or_else(|p| {
                tracing::warn!(site = "boot_dev_renderer.paths", "mutex poisoned, recovered");
                p.into_inner()
            });
            let state = lock
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("renderer not started for paths() evaluation"))?;
            let host = state.embedded_v8_host_mut().ok_or_else(|| {
                anyhow::anyhow!(
                    "embedded V8 host unavailable after start; \
                     Backend::EmbeddedV8 state had no host"
                )
            })?;
            let mut dispatch = WorkerDispatch::EmbeddedV8 { host };
            eval_deferred_paths_via_worker(
                &static_expansion.deferred,
                &mut dispatch,
                &mut paths_cache,
                None,
            )
        };

        // Surface routes that still couldn't be expanded (after both phases)
        // as warnings so the user knows why a `[slug]` route didn't appear —
        // mirrors the build-side diagnostics. These are silently dropped from
        // `dist/` otherwise.
        for d in &runtime_expansion.deferred {
            crate::output::warn(format!(
                "dynamic SSG route {} not expanded in dev: {}",
                d.template, d.reason
            ));
        }

        // File every resolved concrete-URL entry under its dynamic source's
        // `PageId`. A single `[slug].tsx` source thus accumulates N entries,
        // which `render_one` fans out into N `RenderedPage`s per tick.
        for entry in static_expansion
            .resolved
            .into_iter()
            .chain(runtime_expansion.resolved)
        {
            if let Some(source) = template_to_source.get(&entry.route_key) {
                routes_by_source
                    .entry(source.clone())
                    .or_default()
                    .push(entry);
            } else {
                // Should not happen: every resolved entry's route_key came
                // from one of the ssg_deferred routes. Warn rather than drop
                // silently if the invariant is ever violated.
                crate::output::warn(format!(
                    "resolved dynamic URL {} has no matching source route ({}); skipping",
                    entry.url_path, entry.route_key
                ));
            }
        }
    }

    Ok((routes_by_source, ssr_routes))
}

/// Bring up the renderer and the route map for the dev session.
///
/// On any error, returns it unchanged so the caller decides whether to
/// fall back to a noop renderer or hard-fail. Today the dev command
/// chooses to fall back so the user still gets a reachable HTTP server
/// while they fix the underlying issue.
// `boot_dev_renderer` constructs the long-lived dev RendererState
// backed by the in-process V8 host. Compiled in only when the
// `embed_v8` feature is on (issue #371, sub-task 4.1a).
#[cfg(feature = "embed_v8")]
fn boot_dev_renderer(
    project_root: &Path,
    cfg: &config::Config,
    v8_plugin_hooks: zfb_render::PluginRegistryHooks,
    // Plugin-registered import aliases from `setup_registries.aliases`.
    // Threaded into `BundlerInput::plugin_alias_entries` so the dev-mode
    // esbuild invocation can resolve plugin aliases from pages / layouts /
    // shared modules (#268).
    plugin_alias_entries: Vec<(String, String)>,
    // Plugin-registered virtual-module `(specifier, source)` pairs.
    // Threaded into `BundlerInput::plugin_virtual_modules` (#268).
    plugin_virtual_modules: Vec<(String, String)>,
) -> Result<DevRenderSession> {
    check_runtime_installed(project_root)?;

    let pages_dir = project_root.join("pages");
    if !pages_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "no pages/ directory under {}",
            project_root.display()
        ));
    }
    let router = zfb_router::Router::scan(&pages_dir).map_err(anyhow::Error::from)?;

    let plan = build_route_universe(router.routes());
    // Guardrail 2 (#507): an all-dynamic SSG project (only `paths()`-based
    // routes, no static `/`) has an empty `static_routes` but a non-empty
    // `deferred_dynamic`. Bailing on `static_routes.is_empty()` alone would
    // skip renderer boot before the dynamic-route expansion below ever runs,
    // so such a project would never serve any page. Only skip the boot when
    // the project has neither static nor dynamic routes at all.
    if plan.static_routes.is_empty() && plan.deferred_dynamic.is_empty() {
        return Err(anyhow::anyhow!(
            "no routes to render — dev mode skips renderer boot"
        ));
    }

    // Assemble + run the dev bundle (#659: extracted into
    // `assemble_and_bundle_dev` so the watch-ADD rebuild reuses the exact
    // same bundler configuration). Recomputes the content snapshot from
    // disk, so a re-bundle on a created file sees the new content.
    // Stash the bundle inputs BEFORE they are moved into the bundle /
    // host-start calls below, so a watch-ADD (#659) can re-bundle with the
    // identical configuration and reload the host in place. The clones are
    // cheap relative to a bundle (a few small Vecs + the config).
    let rebuild_inputs = DevRebuildInputs {
        cfg: cfg.clone(),
        v8_plugin_hooks: v8_plugin_hooks.clone(),
        plugin_alias_entries: plugin_alias_entries.clone(),
        plugin_virtual_modules: plugin_virtual_modules.clone(),
    };

    let bundler_out: BundlerOutput = assemble_and_bundle_dev(
        project_root,
        cfg,
        plugin_alias_entries,
        plugin_virtual_modules,
    )?;

    let state = start(RendererStartInput {
        bundle_path: bundler_out.bundle_path.clone(),
        sourcemap_path: bundler_out.sourcemap_path.clone(),
        backend: Backend::EmbeddedV8 {
            host_factory: crate::v8_host_adapter::make_v8_host_factory_with_hooks(
                v8_plugin_hooks,
            ),
        },
        request_timeout: None,
    })
    .map_err(anyhow::Error::from)
    .context("renderer start failed")?;

    // Wrap the renderer state in the shared `Arc<Mutex<Option<...>>>` up
    // front so the SSG `paths()` runtime-evaluation phase below can borrow
    // the live embedded V8 host out of the same handle the SSG render
    // callback and the SSR adapter use later (#502/#507). One host, shared
    // across boot-time paths() eval, build-time SSG render, and request-time
    // SSR.
    let renderer: Arc<Mutex<Option<RendererState>>> = Arc::new(Mutex::new(Some(state)));

    // Issue #367 — extract `export const prerender = …` per page so
    // we can keep SSG-eligible pages in `routes_by_source` (the SSG
    // render callback's lookup table) while routing `prerender =
    // false` pages into the request-time SSR set instead. Without this
    // split the SSG callback would stamp a stale snapshot to disk on
    // every watcher tick and the dist fallback would shadow the SSR
    // handler.
    // Build the route tables from the router scan + the live host (#659:
    // extracted into `build_dev_route_tables` so the watch-ADD rebuild
    // reproduces the boot tables exactly).
    let (routes_by_source, ssr_routes) =
        build_dev_route_tables(&router, &plan, project_root, &renderer)?;

    Ok(DevRenderSession {
        inner: Arc::new(DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
            }),
            renderer,
            project_root: project_root.to_path_buf(),
            rebuild_inputs,
        }),
    })
}

/// Per-route HTML output directory for the dev pipeline (issue #534).
///
/// Dev's renderer writes one file per route on each tick (initial scan
/// and every watcher rebuild). Until #534, these writes landed in the
/// project's `outDir` (`dist/`), silently overwriting the production
/// HTML produced by a prior `pnpm build` — stripping the prod-only
/// `<link rel="stylesheet">` / islands `<script type="module">` head
/// injections and breaking subsequent `pnpm preview`.
///
/// Dev now writes to `<project_root>/.zfb-build/dev-pages/`. This sits
/// under the existing `.zfb-build/` intermediate directory (already
/// `.gitignore`d by the project templates) and is read back by
/// `DevRenderSession::render_one_with` to populate the in-memory
/// `PageCache`. End users of the dev server are unaffected — page
/// lookups are URL-keyed and never touch this path.
fn dev_html_root_for(project_root: &Path) -> PathBuf {
    project_root.join(".zfb-build").join("dev-pages")
}

/// Build the [`PageRenderer`] callback that the orchestrator hands to
/// [`DevAssetPipeline`].
fn make_render_callback(session: DevRenderSession, dist_dir: PathBuf) -> PageRenderer {
    Arc::new(move |pages: &[PageId]| {
        let mut out = Vec::with_capacity(pages.len());
        for page in pages {
            match session.render_one(page, &dist_dir) {
                Ok(rendered) => {
                    // A static route yields one page; a dynamic SSG route
                    // yields one page per `paths()`-resolved URL (#502/#507).
                    // An empty Vec means the source path is unknown to the
                    // renderer (dynamic route deferred to SSR, or a page never
                    // in the router scan) — intentionally a no-op so the
                    // watcher tick still succeeds and other pages keep
                    // rebuilding.
                    out.extend(rendered);
                }
                Err(err) => {
                    // One page's render failure must not kill the
                    // watcher. Log and continue.
                    output::error(format!(
                        "renderer error for {}: {err:#}",
                        page.path().display()
                    ));
                }
            }
        }
        Ok(out)
    })
}

/// Build the live watch-ADD discovery hook handed to
/// [`zfb_build::BuildOrchestrator::run`] (issue #659).
///
/// The orchestrator invokes it with the `Created` subset of a tick's
/// changed paths. We restrict the expensive re-bundle to files created
/// under `content/`, `pages/`, or any configured collection root (the
/// roots that feed the SSR bundle and the route table — collections may
/// live anywhere, e.g. `src/mdx/notes`); a file created elsewhere —
/// `styles/`, `public/` — can never add a content-collection route, so
/// we skip discovery for it and let the normal tick handle it.
///
/// On a relevant create we delegate to
/// [`DevRenderSession::discover_created`] (re-bundle → swap in a fresh
/// host → rebuild route tables) and then upsert the new file into the
/// dependency graph as a content dep of each rediscovered source page, so
/// a LATER edit of that same file hot-reloads its consumer exactly like a
/// pre-existing post does.
///
/// The returned [`DiscoveryOutcome`] reports `renderer_reloaded: true`
/// whenever the refresh ran, so the pipeline's per-tick
/// `reload_renderer` is skipped and the tick bundles exactly once.
/// Any routes that vanished during the rebuild are propagated in
/// `vanished_output_paths` so the pipeline can prune their HTML files
/// (issue #804 P2).
///
/// `embed_v8`-gated: discovery needs the embedded V8 host.
#[cfg(feature = "embed_v8")]
fn make_discovery_hook(
    session: DevRenderSession,
    graph: Arc<Mutex<DependencyGraph>>,
    html_root: PathBuf,
    // Issue #807 — the live SSR routes handle. The discovery refresh marks
    // the renderer fresh, so the pipeline skips `reload_renderer`; we must
    // rewrite the handle HERE or a newly-created `prerender = false` route
    // 404s until a later edit. `None` when the project has no SSR.
    ssr_routes: Option<SsrRoutesHandle>,
) -> zfb_build::DiscoveryHook {
    let mut relevant_roots: Vec<PathBuf> = vec![
        session.inner.project_root.join("content"),
        session.inner.project_root.join("pages"),
    ];
    for collection in &session.inner.rebuild_inputs.cfg.collections {
        let root = session.inner.project_root.join(&collection.path);
        if !relevant_roots.contains(&root) {
            relevant_roots.push(root);
        }
    }
    Arc::new(move |created: &[PathBuf]| {
        // Only created files under content/, pages/, or a collection
        // root can introduce a new content-collection route; skip the
        // re-bundle otherwise.
        let relevant: Vec<PathBuf> = created
            .iter()
            .filter(|p| relevant_roots.iter().any(|root| p.starts_with(root)))
            .cloned()
            .collect();
        if relevant.is_empty() {
            return Ok(DiscoveryOutcome::default());
        }

        let (changed, vanished_rel) = session.discover_created(&relevant)?;

        // Issue #807 — the discovery refresh swapped in a fresh V8 host and
        // rebuilt the route tables, but it reports `renderer_reloaded: true`,
        // so the pipeline will SKIP `reload_renderer` for this tick. That
        // closure is the only OTHER place the live SSR route set is rewritten,
        // so without this call a newly-created `prerender = false` page never
        // reaches the request dispatcher and 404s until a later edit. Rewrite
        // the handle here via the same `make_ssr_route_set` path.
        if let Some(handle) = &ssr_routes {
            refresh_live_ssr_routes(&session, handle);
        }

        // Upsert each newly-created content file as a content dep of the
        // rediscovered source pages, so subsequent EDITs of the new file
        // map to their consumer page in the graph (matching how a
        // pre-existing post behaves). `upsert` merges deps, so this does
        // not clobber the page's other edges.
        if !changed.is_empty() {
            if let Ok(mut g) = graph.lock() {
                for page in &changed {
                    let deps: Vec<(PathBuf, zfb_graph::DepKind)> = relevant
                        .iter()
                        .map(|c| (c.clone(), zfb_graph::DepKind::Content))
                        .collect();
                    g.upsert(PageDeps::new(page.clone(), deps));
                }
            }
        }

        // Map relative vanished output paths to absolute dist paths so the
        // orchestrator can forward them to the pipeline's prune loop.
        let vanished_abs: Vec<PathBuf> = vanished_rel
            .into_iter()
            .map(|rel| html_root.join(rel))
            .collect();

        Ok(DiscoveryOutcome {
            pages: changed,
            renderer_reloaded: true,
            vanished_output_paths: vanished_abs,
        })
    })
}

/// Build the initial live [`SsrRoutesHandle`] for the dev server from the
/// dev session (issue #367 / Gap 1, live-update issue #807).
///
/// Returns `None` when the dev session is absent (renderer disabled —
/// the SSR layer would have no V8 host to dispatch through). When every
/// page is SSG (no `prerender = false` routes at boot), the handle still
/// wraps `None` so the request dispatcher does nothing but a later refresh
/// can populate it if the user adds a `prerender = false` route mid-session.
///
/// The handle is an `Arc<RwLock<Option<SsrRouteSet>>>`. The per-tick
/// renderer reload callback holds a clone of this `Arc` and writes a fresh
/// `SsrRouteSet` into it after each bundle refresh — adding or removing
/// `prerender = false` routes mid-session is reflected on the next request
/// without a dev-server restart (issue #807).
///
/// The dispatcher is constructed once from the session's renderer handle
/// and reused across refreshes (the `Arc<Mutex<Option<RendererState>>>`
/// it wraps is the same one the refresh swaps the new host into, so
/// request-time SSR automatically sees the new bundle).
///
/// Compiled in only when the `embed_v8` feature is on (issue #371,
/// sub-task 4.1a) — the SSR adapter requires the V8 host.
#[cfg(feature = "embed_v8")]
fn build_ssr_route_set(session: Option<&DevRenderSession>) -> Option<SsrRoutesHandle> {
    let session = session?;
    let renderer_handle = session.renderer_handle();
    let dispatcher: Arc<dyn SsrDispatcher> =
        Arc::new(crate::ssr_adapter::EmbeddedV8SsrAdapter::new(
            renderer_handle,
        ));
    let initial_set = make_ssr_route_set(session, Arc::clone(&dispatcher));
    Some(Arc::new(std::sync::RwLock::new(initial_set)))
}

/// Build an [`SsrRouteSet`] from the dev session's current SSR patterns.
///
/// Returns `None` when the session has zero `prerender = false` routes,
/// which the request dispatcher treats identically to "no SSR configured."
/// Called at boot (inside [`build_ssr_route_set`]) and after each tick's
/// renderer reload to refresh the live handle (issue #807).
#[cfg(feature = "embed_v8")]
fn make_ssr_route_set(
    session: &DevRenderSession,
    dispatcher: Arc<dyn SsrDispatcher>,
) -> Option<SsrRouteSet> {
    let patterns = session.ssr_patterns();
    if patterns.is_empty() {
        return None;
    }
    let records = patterns
        .into_iter()
        .map(|pattern| SsrRouteRecord { pattern })
        .collect();
    Some(SsrRouteSet::new(records, dispatcher))
}

/// Rewrite the live [`SsrRoutesHandle`] from the dev session's CURRENT SSR
/// patterns (issue #807). Builds a fresh dispatcher over the session's
/// renderer handle and a fresh [`SsrRouteSet`] via [`make_ssr_route_set`],
/// then swaps it into the `RwLock`.
///
/// Shared by BOTH refresh seams so they stay in lock-step:
/// - the per-tick `reload_renderer` (in-place EDIT ticks), and
/// - the watch-ADD discovery hook (`make_discovery_hook`).
///
/// Without the discovery-hook call site, a newly-created `prerender = false`
/// page 404s until a later edit: the discovery refresh marks the renderer
/// fresh, so the pipeline skips `reload_renderer` and the live handle is
/// never updated for that tick.
#[cfg(feature = "embed_v8")]
fn refresh_live_ssr_routes(session: &DevRenderSession, handle: &SsrRoutesHandle) {
    let fresh_dispatcher = {
        let renderer_handle = session.renderer_handle();
        Arc::new(crate::ssr_adapter::EmbeddedV8SsrAdapter::new(
            renderer_handle,
        )) as Arc<dyn SsrDispatcher>
    };
    let new_set = make_ssr_route_set(session, fresh_dispatcher);
    if let Ok(mut lock) = handle.write() {
        *lock = new_set;
    }
}

/// Translate Hono-style colon-syntax templates emitted by
/// `zfb_router::Route::template()` into the `pages/`-style bracket
/// grammar consumed by `zfb_server::SsrRouteSet`'s matcher (which calls
/// into `crate::injected_routes::pattern_matches`).
///
/// Grammar:
/// - `:name`        → `[name]`        (single-segment dynamic param)
/// - `:name{.+}`    → `[...name]`     (catchall — Hono regex quantifier)
/// - literal segments are preserved unchanged
///
/// The root `/` is preserved as `/`.
fn colon_template_to_bracket(template: &str) -> String {
    let segments: Vec<&str> = template.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::with_capacity(template.len() + 4);
    for seg in &segments {
        out.push('/');
        if let Some(rest) = seg.strip_prefix(':') {
            // Catchall (Hono `:name{.+}`) wins over single-segment.
            if let Some(name) = rest.strip_suffix("{.+}") {
                out.push_str("[...");
                out.push_str(name);
                out.push(']');
            } else {
                out.push('[');
                out.push_str(rest);
                out.push(']');
            }
        } else {
            out.push_str(seg);
        }
    }
    out
}

const DEFAULT_DEV_HOST: &str = "localhost";
const DEFAULT_DEV_PORT: u16 = 3000;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // `resolve_host` / `resolve_addr` live in `crate::commands::resolve` (shared
    // with `preview`); their precedence and binding tests live there too.

    /// Build a stub [`DevRenderInner`] for the route-plumbing seam tests
    /// (no live V8 host). The discovery (#659) `rebuild_inputs` are filled
    /// with defaults — these tests never call `discover_created`.
    #[cfg(feature = "embed_v8")]
    fn stub_dev_inner(
        routes_by_source: HashMap<PathBuf, Vec<RouteUniverseEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
            }),
            renderer: Arc::new(Mutex::new(None)),
            project_root: PathBuf::new(),
            rebuild_inputs: DevRebuildInputs {
                cfg: config::Config::default(),
                v8_plugin_hooks: zfb_render::PluginRegistryHooks::default(),
                plugin_alias_entries: Vec::new(),
                plugin_virtual_modules: Vec::new(),
            },
        }
    }

    /// V8-off counterpart of [`stub_dev_inner`] — no `rebuild_inputs`
    /// field exists when `embed_v8` is disabled.
    #[cfg(not(feature = "embed_v8"))]
    fn stub_dev_inner(
        routes_by_source: HashMap<PathBuf, Vec<RouteUniverseEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
            }),
            renderer: Arc::new(Mutex::new(None)),
            project_root: PathBuf::new(),
        }
    }

    #[test]
    fn default_watch_roots_includes_zfb_config_json() {
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.json"));
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.ts"));
    }

    fn cfg_with_collections(paths: &[&str]) -> config::Config {
        config::Config {
            collections: paths
                .iter()
                .map(|p| config::CollectionDef {
                    name: p.replace('/', "-"),
                    path: PathBuf::from(p),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                })
                .collect(),
            ..config::Config::default()
        }
    }

    /// Regression: a collection configured outside the default watch
    /// roots (e.g. `src/mdx/notes`) was never watched, so edits there
    /// produced no rebuild and the dev server served stale HTML until
    /// restart. Discovered during usage in a consumer project with
    /// custom collection paths.
    #[test]
    fn derive_watch_roots_appends_custom_collection_paths() {
        let cfg = cfg_with_collections(&["src/mdx/notes", "src/mdx/guides"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("src/mdx/notes")));
        assert!(roots.contains(&PathBuf::from("src/mdx/guides")));
        // Defaults are preserved in front.
        assert!(roots.contains(&PathBuf::from("pages")));
        assert!(roots.contains(&PathBuf::from("content")));
    }

    /// A collection under the default `content/` root must NOT be
    /// re-registered — overlapping recursive watches deliver duplicate
    /// events for the same write.
    #[test]
    fn derive_watch_roots_skips_paths_covered_by_default_roots() {
        let cfg = cfg_with_collections(&["content/blog"]);
        let roots = derive_watch_roots(&cfg);
        assert!(!roots.contains(&PathBuf::from("content/blog")));
        assert_eq!(
            roots.len(),
            DEFAULT_WATCH_ROOTS.len(),
            "covered collection path must not add a watch root"
        );
    }

    /// Nested collections collapse to the shallowest ancestor regardless
    /// of declaration order, and duplicates dedupe.
    #[test]
    fn derive_watch_roots_collapses_nested_and_duplicate_collections() {
        let cfg = cfg_with_collections(&["src/mdx/notes", "src/mdx", "src/mdx/notes"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("src/mdx")));
        assert!(!roots.contains(&PathBuf::from("src/mdx/notes")));
        assert_eq!(roots.len(), DEFAULT_WATCH_ROOTS.len() + 1);
    }

    /// Leading `./` is normalized away so `./src/mdx` and `src/mdx`
    /// compare equal in the dedupe/coverage checks.
    #[test]
    fn derive_watch_roots_normalizes_leading_curdir() {
        let cfg = cfg_with_collections(&["./src/mdx", "src/mdx"]);
        let roots = derive_watch_roots(&cfg);
        assert!(roots.contains(&PathBuf::from("src/mdx")));
        assert_eq!(roots.len(), DEFAULT_WATCH_ROOTS.len() + 1);
    }

    /// Issue #534 regression — dev's per-route HTML output dir must live
    /// under `.zfb-build/`, NOT under the project's `outDir` (`dist/`).
    /// If this contract is broken, `pnpm dev` after a clean `pnpm build`
    /// silently overwrites prod HTML in `dist/`, stripping the prod-only
    /// `<link>` / islands `<script type="module">` head injections and
    /// breaking the subsequent `pnpm preview`.
    ///
    /// Falsifiability: changing the helper to return `dist/dev-pages`
    /// or `dist` itself fails the second / third assertion.
    #[test]
    fn dev_html_root_lives_under_dot_zfb_build_not_outdir() {
        let project_root = PathBuf::from("/tmp/proj");
        let dev_html_root = dev_html_root_for(&project_root);

        // The exact, documented contract:
        //   `<project_root>/.zfb-build/dev-pages`.
        assert_eq!(
            dev_html_root,
            PathBuf::from("/tmp/proj/.zfb-build/dev-pages"),
        );

        // Negative checks against the regressed locations. The default
        // `outDir` is `dist/`; assert the dev path does not collide
        // anywhere under it.
        let dist_root = project_root.join("dist");
        assert_ne!(dev_html_root, dist_root, "must not equal outDir");
        assert!(
            !dev_html_root.starts_with(&dist_root),
            "dev html dir must not live anywhere under outDir ({}); got {}",
            dist_root.display(),
            dev_html_root.display(),
        );
    }

    /// The render callback must:
    /// 1. Be tolerant of genuinely-unknown page ids (a source path that
    ///    maps to no `RouteUniverseEntry` at all) — return an empty list,
    ///    never error.
    /// 2. NOT silently drop a source path that DOES map to entries.
    ///
    /// Contract change (#502/#507): previously every dynamic route was
    /// dropped here because `routes_by_source` only held static routes.
    /// Now a `[slug]` SSG source resolves (via boot-time `paths()`
    /// expansion) to N entries under its source path, so the callback
    /// must fan those out — only a source path absent from the map is
    /// silently dropped. We use a `None` renderer so the absent-key path
    /// returns `Ok(empty)` without ever reaching the lock.
    #[test]
    fn render_callback_drops_unknown_pages_silently() {
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(HashMap::new(), Vec::new())),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        // A source path not present in routes_by_source is still dropped.
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages).unwrap();
        assert!(out.is_empty());
    }

    /// A dynamic SSG source whose `paths()` expanded into N concrete URLs
    /// fans out through `render_one_with`'s loop: one `RenderedPage` per
    /// entry, each with a synthetic `PageId` derived from the entry's
    /// `output_path` (guardrail 1, #507).
    ///
    /// Uses the `render_one_with` seam directly with a stub closure that
    /// writes known HTML to a tempdir, so the test drives the real loop
    /// without a live V8 `RendererState`. N=3 entries guard against
    /// hard-coded two-item expectations.
    ///
    /// Falsifiability:
    /// - If the loop exits early (e.g. a `break` after one entry), the
    ///   `out.len() == 3` assertion fails.
    /// - If the synthetic `PageId` is derived from the source path instead
    ///   of `entry.output_path`, all three ids are identical and the
    ///   `unique.len() == 3` assertion fails.
    #[test]
    fn dynamic_ssg_source_fans_out_to_multiple_entries_with_distinct_ids() {
        use std::collections::HashSet;
        use tempfile::tempdir;

        let tmp = tempdir().expect("tempdir");

        // Three slugs — one dynamic SSG source → three concrete URLs.
        let slugs = ["hello", "world", "rust"];
        let entries: Vec<RouteUniverseEntry> = slugs
            .iter()
            .map(|slug| RouteUniverseEntry {
                url_path: format!("/blog/{slug}"),
                output_path: PathBuf::from(format!("blog/{slug}/index.html")),
                route_key: "/blog/:slug".into(),
                static_html: false,
                source_path: None,
            })
            .collect();

        let source_page = PageId::new(PathBuf::from("pages/blog/[slug].tsx"));

        // Stub render-fn: create the output file in the tempdir and return
        // its absolute path — exercises the read_to_string round-trip.
        let tmp_path = tmp.path().to_path_buf();
        let out = DevRenderSession::render_one_with(
            &source_page,
            &entries,
            |entry| {
                let dest = tmp_path.join(&entry.output_path);
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                std::fs::write(&dest, format!("<html>{}</html>", entry.url_path)).unwrap();
                Ok(dest)
            },
        )
        .expect("render_one_with must succeed");

        // One RenderedPage per entry.
        assert_eq!(out.len(), 3, "fan-out must emit one page per entry");

        // Every synthetic PageId is derived from the entry's output_path.
        for (rendered, entry) in out.iter().zip(entries.iter()) {
            assert_eq!(
                rendered.page,
                PageId::new(entry.output_path.clone()),
                "PageId must be derived from entry.output_path, not source path"
            );
            // Verify the loop actually read what the stub wrote.
            assert!(
                rendered.html.contains(&entry.url_path),
                "html must reflect what the stub renderer wrote for {}",
                entry.url_path
            );
        }

        // All synthetic ids are distinct (guardrail 1 property).
        let unique: HashSet<_> = out.iter().map(|r| &r.page).collect();
        assert_eq!(
            unique.len(),
            3,
            "each resolved URL must get a distinct pipeline page id"
        );
    }

    /// On a known page id, but a `None` renderer (boot half-failed),
    /// the callback logs an error and returns an empty list — the
    /// watcher must keep going.
    #[test]
    fn render_callback_keeps_watcher_alive_on_render_error() {
        let mut routes: HashMap<PathBuf, Vec<RouteUniverseEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            vec![RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
                static_html: false,
                source_path: None,
            }],
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages).unwrap();
        assert!(out.is_empty(), "errors must yield empty list, not panic");
    }

    /// Deep-review regression (PR #376): `Route::template()` emits
    /// Hono-style colon syntax (`/blog/:slug`, `/docs/:slug{.+}`), but
    /// `SsrRouteSet`'s matcher consumes `pages/`-style brackets
    /// (`/blog/[slug]`, `/docs/[...slug]`). Without translation,
    /// dynamic-route SSR matches never fire in dev.
    #[test]
    fn colon_template_to_bracket_translates_dynamic_and_catchall() {
        assert_eq!(colon_template_to_bracket("/"), "/");
        assert_eq!(colon_template_to_bracket("/about"), "/about");
        assert_eq!(colon_template_to_bracket("/blog/:slug"), "/blog/[slug]");
        assert_eq!(
            colon_template_to_bracket("/docs/:rest{.+}"),
            "/docs/[...rest]",
        );
        assert_eq!(
            colon_template_to_bracket("/a/:b/c/:d"),
            "/a/[b]/c/[d]",
        );
    }

    /// `ssr_patterns()` must hand the SsrRouteSet bracket-grammar
    /// patterns so dynamic SSR routes match in dev.
    #[test]
    fn ssr_patterns_emit_bracket_grammar_for_dynamic_routes() {
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(
                HashMap::new(),
                vec![
                    RouteUniverseEntry {
                        url_path: "/blog/:slug".into(),
                        output_path: PathBuf::new(),
                        route_key: "/blog/:slug".into(),
                        static_html: false,
                        source_path: None,
                    },
                    RouteUniverseEntry {
                        url_path: "/api/x".into(),
                        output_path: PathBuf::new(),
                        route_key: "/api/x".into(),
                        static_html: false,
                        source_path: None,
                    },
                ],
            )),
        };
        let patterns = session.ssr_patterns();
        assert_eq!(patterns, vec!["/blog/[slug]".to_string(), "/api/x".to_string()]);
    }

    /// Review-fix (codex finding 2): a discovery-hook refresh must rewrite
    /// the live `SsrRoutesHandle` so a newly-created `prerender = false`
    /// route is dispatchable WITHOUT a later edit.
    ///
    /// The discovery hook calls `refresh_live_ssr_routes` (it can't rely on
    /// the pipeline's `reload_renderer`, which is skipped when the discovery
    /// refresh marks the renderer fresh). This test drives that exact seam:
    /// a project that booted with ZERO SSR routes (handle wraps `None`),
    /// then a mid-session discovery adds a `prerender = false` route to the
    /// session's tables. After `refresh_live_ssr_routes` the handle must hold
    /// a populated set whose matcher resolves the new route.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn discovery_refresh_updates_live_ssr_routes_without_edit() {
        // Boot state: no SSR routes → the live handle wraps `None`, exactly
        // what `build_ssr_route_set` produces for an all-SSG project.
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(HashMap::new(), Vec::new())),
        };
        let dispatcher: Arc<dyn SsrDispatcher> = Arc::new(
            crate::ssr_adapter::EmbeddedV8SsrAdapter::new(session.renderer_handle()),
        );
        let handle: SsrRoutesHandle = Arc::new(std::sync::RwLock::new(make_ssr_route_set(
            &session,
            Arc::clone(&dispatcher),
        )));
        assert!(
            handle.read().unwrap().is_none(),
            "boot handle must be empty for an all-SSG project"
        );

        // Mid-session discovery: a new `prerender = false` page is found and
        // added to the session's route tables (what `discover_created` does).
        {
            let mut tables = session.inner.routes.write().unwrap();
            tables.ssr_routes.push(RouteUniverseEntry {
                url_path: "/blog/:slug".into(),
                output_path: PathBuf::new(),
                route_key: "/blog/:slug".into(),
                static_html: false,
                source_path: None,
            });
        }

        // The discovery hook's new call (review-fix) — NOT an edit tick.
        refresh_live_ssr_routes(&session, &handle);

        let guard = handle.read().unwrap();
        let set = guard
            .as_ref()
            .expect("handle must now hold a populated SsrRouteSet");
        assert!(
            set.find_match("/blog/anything").is_some(),
            "the newly-created prerender=false route must dispatch without an extra edit"
        );
    }

    // ---------------------------------------------------------------------------
    // refresh_dev_island_chunks unit tests (issue #809)
    // ---------------------------------------------------------------------------

    fn make_companion(filename: &str, bytes: &[u8]) -> zfb_build::pipeline::CompanionFile {
        zfb_build::pipeline::CompanionFile {
            filename: filename.to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn writes_chunk_files_to_assets_dir() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let companions = vec![
            make_companion("islands-chunk-AAAAAAAA.js", b"chunk-a"),
            make_companion("islands-chunk-BBBBBBBB.js", b"chunk-b"),
        ];
        let result = refresh_dev_island_chunks(&assets, &companions, &HashSet::new()).unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains("islands-chunk-AAAAAAAA.js"));
        assert!(result.contains("islands-chunk-BBBBBBBB.js"));
        assert_eq!(
            std::fs::read(assets.join("islands-chunk-AAAAAAAA.js")).unwrap(),
            b"chunk-a"
        );
        assert_eq!(
            std::fs::read(assets.join("islands-chunk-BBBBBBBB.js")).unwrap(),
            b"chunk-b"
        );
    }

    #[test]
    fn prunes_stale_chunks_on_rebundle() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        // Generation 1: two chunks.
        let gen1 = vec![
            make_companion("islands-chunk-GEN1AAAA.js", b"gen1-a"),
            make_companion("islands-chunk-GEN1BBBB.js", b"gen1-b"),
        ];
        let prev = refresh_dev_island_chunks(&assets, &gen1, &HashSet::new()).unwrap();
        assert!(assets.join("islands-chunk-GEN1AAAA.js").exists());
        assert!(assets.join("islands-chunk-GEN1BBBB.js").exists());

        // Generation 2: different chunks (simulates a dynamically-imported
        // module change so esbuild emits a new content hash).
        let gen2 = vec![
            make_companion("islands-chunk-GEN2CCCC.js", b"gen2-c"),
        ];
        let next = refresh_dev_island_chunks(&assets, &gen2, &prev).unwrap();

        // New chunk exists.
        assert!(assets.join("islands-chunk-GEN2CCCC.js").exists());
        // Stale gen-1 chunks were pruned.
        assert!(
            !assets.join("islands-chunk-GEN1AAAA.js").exists(),
            "stale chunk A must be pruned"
        );
        assert!(
            !assets.join("islands-chunk-GEN1BBBB.js").exists(),
            "stale chunk B must be pruned"
        );
        assert_eq!(next.len(), 1);
        assert!(next.contains("islands-chunk-GEN2CCCC.js"));
    }

    #[test]
    fn prunes_all_chunks_when_new_set_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        // Seed one chunk.
        let gen1 = vec![make_companion("islands-chunk-STALEAAA.js", b"stale")];
        let prev = refresh_dev_island_chunks(&assets, &gen1, &HashSet::new()).unwrap();
        assert!(assets.join("islands-chunk-STALEAAA.js").exists());

        // Next bundle has no chunks (project switched to zero dynamic imports).
        let next = refresh_dev_island_chunks(&assets, &[], &prev).unwrap();
        assert!(next.is_empty());
        assert!(
            !assets.join("islands-chunk-STALEAAA.js").exists(),
            "stale chunk must be pruned when new set is empty"
        );
    }

    #[test]
    fn zero_dynamic_imports_is_no_op() {
        // A project without dynamic imports: both prev and new companion sets
        // are empty — no files written or deleted, empty set returned.
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let result = refresh_dev_island_chunks(&assets, &[], &HashSet::new()).unwrap();
        assert!(result.is_empty());
        // No files created in the assets dir.
        assert_eq!(std::fs::read_dir(&assets).unwrap().count(), 0);
    }

    #[test]
    fn kept_chunks_survive_rebundle() {
        // A chunk whose hash did not change (same dynamic import, same content)
        // should be retained rather than deleted and re-written.
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let shared_chunk = "islands-chunk-STABLE00.js";
        let gen1 = vec![
            make_companion(shared_chunk, b"stable-content"),
            make_companion("islands-chunk-OLDCHUNK.js", b"old"),
        ];
        let prev = refresh_dev_island_chunks(&assets, &gen1, &HashSet::new()).unwrap();

        // Next bundle still has the stable chunk but dropped the old one.
        let gen2 = vec![make_companion(shared_chunk, b"stable-content")];
        let next = refresh_dev_island_chunks(&assets, &gen2, &prev).unwrap();

        assert!(assets.join(shared_chunk).exists(), "kept chunk must still exist");
        assert!(
            !assets.join("islands-chunk-OLDCHUNK.js").exists(),
            "old chunk must be pruned"
        );
        assert_eq!(next.len(), 1);
        assert!(next.contains(shared_chunk));
    }

    #[test]
    fn rejects_unsafe_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().to_path_buf();

        let bad_names = [
            "../escape.js",
            "subdir/chunk.js",
            "subdir\\chunk.js",
            "",
        ];
        for name in bad_names {
            let companion = make_companion(name, b"bytes");
            let result = refresh_dev_island_chunks(&assets, &[companion], &HashSet::new());
            assert!(
                result.is_err(),
                "should reject unsafe filename {:?}",
                name
            );
        }
    }
}
