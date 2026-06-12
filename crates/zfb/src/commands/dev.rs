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
#[cfg(feature = "embed_v8")]
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_build::bundler::{bundle_with_session, BundleMode, BundlerOutput, ShadowSession};
use zfb_build::renderer::{
    render_one, shutdown, start, Backend, RendererStartInput, RendererState, RouteUniverseEntry,
};
use zfb_build::{
    BuildContext, BuildOrchestrator, BuildOutcome, ClientScriptsRunner, CssRunner,
    DevAssetPipeline, DiscoveryOutcome, IslandsBundleInfo, IslandsRunner, OrchestratorConfig,
    PageRenderer, RefreshOutcome, RelDistPath, RenderedPage, RendererReloader,
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
    build_prerender_map, build_route_universe, check_runtime_installed,
    eval_deferred_paths_via_worker, expand_dynamic_routes, WorkerDispatch,
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
            format!("failed to create dev html dir {}", dev_html_root.display())
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

    // #255 / #260 / #261 / #268 — shared plugin setup phase:
    // setup → virtual-module prefetch → alias/virtual-module derivation.
    //
    // `SetupCommand::Dev` is the per-command difference (build uses
    // `SetupCommand::Build`).
    let plugin_setup = crate::commands::plugins::run_plugin_setup(
        &plugin_host,
        &project_root,
        &cfg,
        zfb_build::SetupCommand::Dev,
    )
    .await?;

    // preBuild runs before devMiddleware registration and the bundler.
    // Dev-mode difference: out_dir is `dist_root` (the dev scratch dir),
    // not the final `outdir` that build.rs uses.
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

    // Dev-only: register devMiddleware hooks.
    let plugin_set = if let Some(h) = plugin_host.as_ref() {
        crate::commands::plugins::build_dev_middleware_set(h, &project_root, &cfg).await?
    } else {
        None
    };

    // Dev-only: build the InjectedRouteSet from the setup registries.
    // zfb-server depends on zfb-build (see Cargo.toml), so no translation is
    // needed — InjectedRoute is the same type on both sides.
    let injected_route_set = if plugin_setup.setup_registries.injected_routes.is_empty() {
        None
    } else {
        let records: Vec<zfb_build::InjectedRoute> = plugin_setup
            .setup_registries
            .injected_routes
            .iter()
            .cloned()
            .collect();
        Some(zfb_server::InjectedRouteSet::new(records))
    };

    let v8_plugin_hooks = plugin_setup.v8_plugin_hooks;
    let dev_plugin_alias_entries = plugin_setup.plugin_alias_entries;
    let dev_plugin_virtual_modules = plugin_setup.plugin_virtual_modules;
    // Keep setup_registries in scope for the lifetime of the dev session —
    // the hook entries hold references into it.
    let _setup_registries = plugin_setup.setup_registries;

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
    let graph = Arc::new(Mutex::new(initial_graph.unwrap_or_default()));

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
    // Issue #1025 — wire the stale probe so each tick's BuildOutcome
    // reports the routes the lazy render callback marked stale
    // (`pages_stale`). Always wired when a render session exists; the
    // probe drains an empty buffer on every tick while the lazy switch
    // is off, so behaviour is unchanged today.
    let pipeline = match dev_session.as_ref() {
        Some(session) => {
            let probe_session = session.clone();
            DevAssetPipeline::with_stale_probe(Arc::new(move || {
                probe_session.inner.take_tick_stale()
            }))
        }
        None => DevAssetPipeline::new(),
    };
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
        .with_policy(zfb_build::GranularityPolicy::default().with_content_roots(content_roots));
    let orchestrator = BuildOrchestrator::new(orch_config, graph, pipeline);

    let render_pages: PageRenderer = match dev_session.as_ref() {
        // Issue #534 — pass `dev_html_root` (under `.zfb-build/`), not
        // `dist_root`, so per-route dev renders do not overwrite the
        // production HTML files that `pnpm preview` serves.
        Some(session) => make_render_callback(session.clone(), dev_html_root.clone()),
        None => Arc::new(|_pages: &[PageId], _narrowing| Ok(Vec::new())),
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
    let live_chunk_filenames: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    // Eager initial bundle. Failures are non-fatal — we warn and let the
    // dev server boot anyway. The hot-rebuild path will retry on the next
    // file change so a transient esbuild hiccup at boot doesn't strand the
    // user.
    // Unlike the production path (which only emits a hashed file), the dev
    // server must write the stable `dist/assets/islands.js` explicitly so
    // `ServeDir` can serve `GET /assets/islands.js` (mirrors the CSS path
    // below — the bundler carries bytes in memory only; disk writes belong
    // to the caller).
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
        Ok((Some(payload), _marker_names)) => {
            // Write the stable `islands.js` bytes to disk so the dev
            // server's `ServeDir` can handle `GET /assets/islands.js`.
            // The bundler no longer writes to disk itself — the caller
            // owns the disk write (same pattern as the CSS path below).
            // Only publish the bundle URL when the write succeeded —
            // setting it on a failed write would inject a `<script>` tag
            // pointing at a file that 404s (mirrors the pre-change
            // behaviour where a bundler write failure produced no URL).
            let assets_dir = dist_root.join(zfb_types::DIST_ASSETS_DIR);
            let islands_out_path = assets_dir.join(zfb_types::STABLE_ISLANDS_FILENAME);
            if let Some(parent) = islands_out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&islands_out_path, &payload.bytes) {
                Ok(()) => {
                    let url = prefixed_islands_url(payload.stable_url);
                    if let Ok(mut guard) = islands_bundle_url_handle.write() {
                        *guard = Some(url);
                    }
                }
                Err(e) => {
                    output::warn(format!(
                        "initial islands bundle write failed (no <script \
                         type=\"module\"> will be injected until the next \
                         successful rebuild): {e:#}"
                    ));
                }
            }
            // Write chunk companions alongside islands.js and seed the
            // live-chunk tracker. Boot failures here are non-fatal —
            // the server still comes up; the next successful rebuild
            // will retry.
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
        Ok((None, _marker_names)) => {
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
            // Marker names are only needed by the production build pass; dev
            // mode already surfaces unknown-marker warnings in the browser
            // console via the runtime.ts warn path.
            let (payload, _marker_names) = crate::commands::build::build_default_islands_payload(
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
                    if let Err(e) = refresh_dev_island_chunks(&assets_dir, &[], &prev) {
                        tracing::warn!(
                            error = %e,
                            "dev islands: failed to prune stale chunks after no-bundle tick (ignored)"
                        );
                    }
                    *prev = HashSet::new();
                }
                return Ok(None);
            };
            // Write the stable `islands.js` bytes to disk so ServeDir can
            // serve `GET /assets/islands.js`. The bundler carries bytes in
            // memory only — the dev caller owns the disk write (same
            // pattern as the CSS path).
            {
                let assets_dir = dist_root_for_islands.join(zfb_types::DIST_ASSETS_DIR);
                let islands_out_path = assets_dir.join(zfb_types::STABLE_ISLANDS_FILENAME);
                if let Some(parent) = islands_out_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&islands_out_path, &payload.bytes) {
                    return Err(anyhow::anyhow!(
                        "dev islands: failed to write islands.js to disk: {e:#}"
                    ));
                }
            }
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
    let css_bundle_url_handle: zfb_server::CssBundleUrl = Arc::new(std::sync::RwLock::new(None));

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
            // No CSS to ship (no authored globals, no CSS Modules, and —
            // when Tailwind is enabled — no scannable sources). Leave the
            // handle at None. Tailwind being disabled no longer implies
            // None on its own: authored CSS still ships (issue #824).
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

    // Client-scripts: eager initial bundle at boot + watcher-driven
    // rebuild. Mirrors the islands and CSS patterns above.
    //
    // Tracks which entry names were written by the most recent bundle so
    // the next rebuild can prune stale files (removed or renamed entries).
    let live_client_script_names: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));

    // Eager boot bundle — non-fatal, mirrors islands / CSS.
    match crate::commands::build::build_dev_client_scripts_to_disk(
        &project_root,
        &dist_root,
        cfg.framework,
        &std::collections::HashSet::new(),
    ) {
        Ok((_, names)) => {
            if let Ok(mut guard) = live_client_script_names.lock() {
                *guard = names;
            }
        }
        Err(err) => {
            output::warn(format!(
                "initial client-scripts bundle failed (no client scripts will be served \
                 until the next successful rebuild): {err:#}"
            ));
        }
    }

    // Watcher-driven rebuild closure.
    let run_client_scripts: Option<ClientScriptsRunner> = {
        let project_root_for_cs = project_root.clone();
        let dist_root_for_cs = dist_root.clone();
        let framework = cfg.framework;
        let entry_names = Arc::clone(&live_client_script_names);
        Some(Arc::new(move || -> Result<bool> {
            let prev = entry_names
                .lock()
                .unwrap_or_else(|p| {
                    tracing::warn!(
                        site = "dev.run_client_scripts.entry_names",
                        "mutex poisoned, recovered"
                    );
                    p.into_inner()
                })
                .clone();
            let (changed, new_names) = crate::commands::build::build_dev_client_scripts_to_disk(
                &project_root_for_cs,
                &dist_root_for_cs,
                framework,
                &prev,
            )?;
            let mut guard = entry_names.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "dev.run_client_scripts.entry_names (write)",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            });
            *guard = new_names;
            Ok(changed)
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
            let (changed, vanished_rel) = match session
                .refresh_bundle_and_routes()
                .context("edit-tick bundle refresh failed")?
            {
                // Phase-B skip (issue #940): the live host, route tables,
                // and SSR route set were all left untouched — report
                // `Skipped` so DevAssetPipeline bypasses the render
                // fan-out for the tick (issue #956).
                BundleRefresh::Skipped => return Ok(RefreshOutcome::Skipped),
                BundleRefresh::Refreshed { changed, vanished } => (changed, vanished),
            };
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
            // Issue #958 — propagate the refresh's changed-source set
            // instead of dropping it: a non-empty set means the route
            // structure moved this tick, so the dev pipeline must
            // disable content narrowing (fallback G5).
            Ok(RefreshOutcome::Refreshed {
                vanished: vanished_abs,
                changed_sources: changed,
            })
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
        run_client_scripts,
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
    let discover_hook: Option<zfb_build::DiscoveryHook> = dev_session.as_ref().map(|session| {
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
        // Issue #931: Host-header allowlist for non-localhost binds.
        // `allowedHosts` config entries plus the explicitly bound host;
        // the server disables enforcement entirely for loopback binds.
        allowed_hosts: cfg.allowed_hosts.clone(),
        bound_host: Some(host.clone()),
        // Issue #1020: render-on-request hook wired up in a later
        // sub-issue (the zfb-side lazy DevRenderSession adapter).
        // Placeholder `None` until that sub-issue lands.
        render_on_request_hook: None,
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

    // Announce the ACTUAL bound port, not the requested one: with
    // `--port 0` the OS picks an ephemeral port, and printing the literal
    // `0` makes the banner unparseable for callers that need to discover
    // the port (e.g. the dev E2E harness, #1018). For a fixed port the
    // two values are identical, so existing UX is unchanged.
    let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    output::ready_with_interfaces("http", &host, bound_port);

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
    let new_filenames: HashSet<String> = companions.iter().map(|c| c.filename.clone()).collect();

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
            format!("dev islands: failed to write chunk file {}", dest.display())
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

/// Compile-time default for the lazy dev-render switch (issue #1025).
///
/// `false` = today's fully-eager dev behaviour (zero change). The
/// wave-5 activation sub-issue flips this single line to `true`; until
/// then `ZFB_LAZY_DEV_RENDER=1` opts a session in for testing.
const LAZY_DEV_RENDER_DEFAULT: bool = false;

/// Resolve the lazy dev-render switch once at boot (issue #1025).
///
/// `ZFB_LAZY_DEV_RENDER=1|true` forces ON, `0|false` forces OFF; unset
/// or unrecognized values fall back to [`LAZY_DEV_RENDER_DEFAULT`]. Read
/// a single time in [`boot_dev_renderer`] and stored on
/// [`DevRenderInner::lazy_render`] — the tick path never re-reads the
/// environment.
fn lazy_dev_render_enabled() -> bool {
    match std::env::var("ZFB_LAZY_DEV_RENDER") {
        Ok(raw) => {
            let t = raw.trim();
            if t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true") {
                true
            } else if t.eq_ignore_ascii_case("0") || t.eq_ignore_ascii_case("false") {
                false
            } else {
                LAZY_DEV_RENDER_DEFAULT
            }
        }
        Err(_) => LAZY_DEV_RENDER_DEFAULT,
    }
}

/// Per-route staleness state for lazy dev rendering (issue #1025).
///
/// Keyed by **relative output path** (under the dev HTML root) — the
/// same key the pipeline's write bookkeeping and the synthetic
/// output-path `PageId`s use, and the value `lookup_by_url` resolves a
/// request to. One mutex guards all three fields so a mark / claim /
/// swap observes a consistent snapshot.
#[derive(Debug, Default)]
struct StaleRoutes {
    /// Monotonic tick generation. Incremented at the P4 route-table
    /// swap in [`DevRenderSession::refresh_bundle_and_routes`] — i.e.
    /// exactly when the route universe moves. Phase-B-skipped ticks
    /// (#956) never reach P4 and leave it untouched.
    generation: u64,
    /// Stale output path → the generation that (most recently) staled
    /// it. An entry means "the on-disk/cached HTML for this route may
    /// not reflect the current source tree; re-render before serving".
    entries: HashMap<PathBuf, u64>,
    /// Output paths marked stale by the CURRENT tick's render callback,
    /// drained once per tick into [`zfb_build::BuildOutcome::pages_stale`]
    /// via the pipeline's stale probe.
    tick_stale: Vec<PathBuf>,
}

/// ABA-safe token for one request-time stale-route render (issue #1025).
///
/// Returned by [`DevRenderInner::claim`]; passed back to
/// [`DevRenderInner::clear_if_current`] after the claimed route was
/// re-rendered. Carries the generation recorded for the entry at claim
/// time, so a tick that re-stales the route mid-render (at a higher
/// generation) keeps it stale — the clear only applies when no newer
/// staling happened.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaleClaim {
    /// The claimed route's relative output path.
    output_path: PathBuf,
    /// The stale entry's recorded generation at claim time.
    generation: u64,
}

impl DevRenderInner {
    /// Mark `output_paths` stale at the current generation and queue
    /// them for this tick's [`zfb_build::BuildOutcome::pages_stale`]
    /// signal. Re-marking an already-stale route bumps its recorded
    /// generation, which is what defeats the claim/clear ABA race.
    fn mark_stale<I: IntoIterator<Item = PathBuf>>(&self, output_paths: I) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        let generation = stale.generation;
        for path in output_paths {
            stale.entries.insert(path.clone(), generation);
            stale.tick_stale.push(path);
        }
    }

    /// Drop the stale entries for routes that were just rendered
    /// eagerly — fresh output supersedes any earlier staling. (Within a
    /// tick the eager set and the stale remainder are disjoint, so this
    /// can never erase a mark from the same tick.)
    fn clear_stale(&self, output_paths: &[PathBuf]) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        for path in output_paths {
            stale.entries.remove(path);
        }
    }

    /// P4 hook (issue #1025): the route tables just swapped — advance
    /// the tick generation and evict entries whose output routes
    /// vanished from the live route set (#804: a vanished route must
    /// not linger as a stale entry, or a later claim would try to
    /// render a route that no longer resolves). Also drops the vanished
    /// paths from the current tick buffer so they are never announced
    /// as stale.
    fn note_table_swap(&self, vanished: &[PathBuf]) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        stale.generation += 1;
        if !vanished.is_empty() {
            let vanished: HashSet<&PathBuf> = vanished.iter().collect();
            stale.entries.retain(|path, _| !vanished.contains(path));
            stale.tick_stale.retain(|path| !vanished.contains(path));
        }
    }

    /// Drain the routes marked stale by the current tick (consumed by
    /// the dev pipeline's stale probe — see
    /// [`zfb_build::DevAssetPipeline::with_stale_probe`]). Sorted for a
    /// deterministic [`zfb_build::BuildOutcome::pages_stale`].
    fn take_tick_stale(&self) -> Vec<PathBuf> {
        let mut drained = {
            let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
            std::mem::take(&mut stale.tick_stale)
        };
        drained.sort();
        drained.dedup();
        drained
    }

    /// Claim a stale route for a request-time render (issue #1025).
    ///
    /// `Some(claim)` when `output_path` is currently stale, `None` when
    /// it is fresh (serve the cached/on-disk HTML as-is). The returned
    /// claim records the entry's generation for the ABA check in
    /// [`Self::clear_if_current`].
    ///
    /// Calling discipline (wave-4 adapter): capture the claim while
    /// holding the renderer mutex — the same lock the P2 host swap
    /// takes — so the claim/render pair is serialized against host
    /// swaps and the rendered bytes always come from a host at least as
    /// new as the claimed generation.
    // Dead until the lazy-adapter sub-issue consumes it (issue #1025).
    #[allow(dead_code)]
    fn claim(&self, output_path: &Path) -> Option<StaleClaim> {
        let stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        stale.entries.get(output_path).map(|generation| StaleClaim {
            output_path: output_path.to_path_buf(),
            generation: *generation,
        })
    }

    /// Clear a claimed stale entry after a successful request-time
    /// render — but ONLY if no later tick re-staled the route: the
    /// entry is removed iff `claim.generation >=` the recorded
    /// generation. A route re-staled at a higher generation mid-render
    /// stays stale (ABA safety), so the next request re-renders it
    /// against the newer world.
    // Dead until the lazy-adapter sub-issue consumes it (issue #1025).
    #[allow(dead_code)]
    fn clear_if_current(&self, claim: &StaleClaim) {
        let mut stale = self.stale.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(recorded) = stale.entries.get(&claim.output_path) {
            if claim.generation >= *recorded {
                stale.entries.remove(&claim.output_path);
            }
        }
    }
}

/// One expanded route plus its `paths()` provenance (issue #958).
///
/// The provenance is what lets a content edit narrow a dynamic source's
/// render fan-out to the routes whose params match the edited entry's
/// slug candidates — see [`compute_tick_narrowing`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct DevRouteEntry {
    entry: RouteUniverseEntry,
    /// Per-URL params retained from `DynamicExpansion::resolved_with_params`
    /// (issue #958). `None` for static routes and for any expansion whose
    /// parallel `resolved` / `resolved_with_params` vectors disagreed in
    /// length (fallback S1 — the whole source then always renders in
    /// full).
    params: Option<crate::render_pipeline::ResolvedRouteParams>,
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
    ///
    /// Issue #958: each entry carries its optional `paths()` params
    /// provenance (see [`DevRouteEntry`]).
    routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
    /// URL patterns for `prerender = false` pages (issue #367 /
    /// Gap 1). Empty when every page in the project SSGs. The dev
    /// server reads this list (via [`DevRenderSession::ssr_patterns`])
    /// and builds an [`zfb_server::SsrRouteSet`] from it.
    ssr_routes: Vec<RouteUniverseEntry>,
    /// Reverse lookup: normalized request URL → SSG route entry (issue
    /// #1019). Keyed by every candidate produced by
    /// [`build_url_index`] so a single `RouteUniverseEntry` is
    /// reachable via `/posts/a`, `/posts/a/`, and
    /// `/posts/a/index.html`. SSR-only routes (`prerender = false`) are
    /// intentionally absent — they are served by the existing SSR leg.
    /// Rebuilt atomically with the other tables at P4 so the index is
    /// never stale. Consumed by a later lazy-render sub-issue; dormant
    /// in this one.
    url_index: HashMap<String, RouteUniverseEntry>,
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
    /// Process-lifetime embedded-esbuild extraction (#994 item A): the
    /// `(TempDir, PathBuf)` pair from one boot-time
    /// [`crate::render_pipeline::embedded_binary`] call, threaded into
    /// `assemble_bundler_input` on every tick so the per-tick tempdir
    /// extraction (the bulk of the measured P1 `asm` cost) is skipped.
    /// The TempDir handle lives as long as the dev session — i.e. it
    /// outlives every `bundle()` call, trivially satisfying the
    /// [`crate::commands::bundler_input::AssembledBundlerInput`]
    /// lifetime contract. `None` when boot-time extraction failed
    /// (non-fatal: warn + per-tick fallback) or when `ZFB_ESBUILD_BIN`
    /// overrides the embedded binary.
    esbuild: Option<(tempfile::TempDir, PathBuf)>,
}

#[cfg(feature = "embed_v8")]
impl DevRebuildInputs {
    /// Path of the boot-time extracted esbuild binary (#994 item A), if any.
    fn esbuild_path(&self) -> Option<&Path> {
        self.esbuild.as_ref().map(|(_, path)| path.as_path())
    }
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

    /// Skip key from the last SUCCESSFUL `refresh_bundle_and_routes` call
    /// (issue #940 — Phase B). Hashes bundle bytes + the router scan's
    /// sorted source paths + route templates + static `pages/**.html`
    /// bodies (issue #956) so a no-op tick (identical bundle, identical
    /// route universe) skips V8 host boot + swap and the `paths()`
    /// re-expansion. Stored only after ALL of: host boot, swap, AND
    /// route-table rebuild succeed — a failed refresh must NOT poison
    /// this field so the next byte-identical tick can retry.
    ///
    /// `None` on first tick (cold start) — forces a full refresh.
    /// Wrapped in a `Mutex` so `&self.refresh_bundle_and_routes` can
    /// update it without `&mut self`.
    #[cfg(feature = "embed_v8")]
    last_successful_skip_key: Mutex<Option<[u8; 32]>>,

    /// Frontmatter gate cache for content-edit narrowing (issue #958,
    /// fallback G4): SHA-256 of the canonical JSON of each collection
    /// file's parsed frontmatter, keyed by the file's absolute path
    /// exactly as the watcher delivers it. Seeded at boot
    /// ([`seed_frontmatter_hashes`]) and updated on every narrowing-
    /// candidate Content tick. A missing or differing entry means the
    /// frontmatter may feed cross-page props (sidebar titles, prev/next
    /// labels) — no narrowing that tick.
    #[cfg(feature = "embed_v8")]
    fm_hashes: Mutex<HashMap<PathBuf, [u8; 32]>>,

    /// Persistent dev shadow-tree session (issue #993): the bundler
    /// reuses one shadow tempdir across all ticks, skipping
    /// byte-identical rewrites ("compute always, write only if
    /// changed" — see [`zfb_build::bundler::ShadowSession`] for the
    /// safety model). Created at boot ([`boot_dev_renderer`]) and used
    /// for the boot bundle AND every refresh, so both go through the
    /// identical assembly path + shadow tree (#659 parity by
    /// construction). The lock is held only across the P1 bundle step
    /// of [`DevRenderSession::refresh_bundle_and_routes`] — it never
    /// overlaps the renderer mutex, so SSR latency is unaffected.
    #[cfg(feature = "embed_v8")]
    shadow_session: Mutex<Option<ShadowSession>>,

    /// Cross-tick [`PathsCache`] (#994 item B): seeded at boot and
    /// passed into every route-table build, so a `paths()` JSON output
    /// identical to a previous tick's skips the Rust-side
    /// validate/URL-build in `resolve_paths`. NOTE the corrected scope
    /// (#992): the cache lookup happens AFTER the V8 `/__paths__/`
    /// dispatch — the key includes the hash of the dispatch RESULT — so
    /// persistence does NOT skip the per-tick V8 evals; the saving is
    /// the ~1–5 ms Rust tail only. Sound by construction: a changed
    /// `paths()` output can never hit a stale entry (key = template +
    /// JSON hash) and stale entries are inert. The lock is scoped to
    /// the P3 route-table build; the renderer lock for phase-2 eval is
    /// taken inside the build exactly as before, so renderer-mutex
    /// hold time is unchanged.
    #[cfg(feature = "embed_v8")]
    paths_cache: Mutex<PathsCache>,

    /// Lazy dev render (issue #1025): per-route staleness map + tick
    /// generation. See [`StaleRoutes`]. Present on both V8 paths — the
    /// state machine itself has no V8 dependency; only the callback
    /// split that feeds it is `embed_v8`-gated.
    stale: Mutex<StaleRoutes>,

    /// Lazy dev render switch (issue #1025), resolved once at boot via
    /// [`lazy_dev_render_enabled`]. `false` (today's default) keeps the
    /// render callback fully eager — zero behaviour change; `true`
    /// routes ticks through [`lazy_render_tick`]'s eager-vs-stale
    /// split.
    lazy_render: bool,
}

/// Per-source route filter for one narrowed tick (issue #958): which of a
/// source's expanded routes [`DevRenderSession::render_one`] should fan
/// out to this invocation.
#[cfg_attr(not(feature = "embed_v8"), allow(dead_code))]
#[derive(Debug)]
enum RouteFilter {
    /// Render every entry of the source — today's full behaviour.
    All,
    /// Render only the entries whose `output_path` is in the set.
    Only(HashSet<PathBuf>),
}

impl RouteFilter {
    fn allows(&self, output_path: &Path) -> bool {
        match self {
            RouteFilter::All => true,
            RouteFilter::Only(set) => set.contains(output_path),
        }
    }
}

/// The whole tick's narrowing decision, computed once per render-callback
/// invocation by [`compute_tick_narrowing`] (issue #958).
#[cfg_attr(not(feature = "embed_v8"), allow(dead_code))]
#[derive(Debug)]
enum TickNarrowing {
    /// No narrowing — every selected page renders its full fan-out.
    Off,
    /// Per-source filters. The map contains ONLY narrowed sources; a
    /// source absent from the map renders in full ([`RouteFilter::All`]) —
    /// that absence IS the always-rendered set (statics, single-entry
    /// sources, aggregate dynamic consumers that fell back via S1/S2).
    PerSource(HashMap<PathBuf, RouteFilter>),
}

#[cfg(feature = "embed_v8")]
impl DevRenderInner {
    /// Phase B (issue #940) — true when `new_key` matches the skip key of
    /// the last SUCCESSFUL refresh, i.e. the refresh may legally skip the
    /// V8 host boot + swap and the `paths()` re-expansion. `new_key =
    /// None` (key not computable) never skips.
    fn should_skip_refresh(&self, new_key: Option<[u8; 32]>) -> bool {
        let prev = *self
            .last_successful_skip_key
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        new_key.is_some() && prev == new_key
    }

    /// Phase B (issue #940) — record the refresh tick's skip key after a
    /// FULLY successful refresh (host boot + swap + route-table rebuild).
    ///
    /// Must NOT be called on a failed refresh — the previous successful
    /// key has to survive so the next byte-identical tick retries in full
    /// (Correctness Req 1: a failed refresh never poisons the key).
    ///
    /// Passing `None` (key was not computable this tick) CLEARS the stored
    /// key: a stale key no longer describes the live renderer's bundle,
    /// and a later tick matching it would skip against the wrong host
    /// state. Clearing forces the next tick to refresh fully (safe
    /// direction: false-invalidate, never false-reuse).
    fn commit_skip_key(&self, new_key: Option<[u8; 32]>) {
        let mut stored = self
            .last_successful_skip_key
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *stored = new_key;
    }
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
    ///
    /// `filter` (issue #958) narrows the fan-out to a subset of the
    /// source's entries on a narrowed content-edit tick; pass
    /// [`RouteFilter::All`] for the full fan-out. Filtering happens on
    /// the per-tick clone, strictly BEFORE the render loop — the shared
    /// tables are never mutated and `render_one_with`'s synthetic
    /// output-path `PageId` keying (#507 Guardrail 1) is untouched.
    fn render_one(
        &self,
        page: &PageId,
        dist_dir: &Path,
        filter: &RouteFilter,
    ) -> Result<Vec<RenderedPage>> {
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
            Some(es) => Self::filter_entries(es, filter),
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

    /// Apply a tick's per-source [`RouteFilter`] to a source's entries,
    /// cloning out the [`RouteUniverseEntry`] payloads that survive
    /// (issue #958). Extracted from [`Self::render_one`] so the narrowing
    /// seam is testable without a live V8 renderer.
    fn filter_entries(entries: &[DevRouteEntry], filter: &RouteFilter) -> Vec<RouteUniverseEntry> {
        entries
            .iter()
            .filter(|de| filter.allows(&de.entry.output_path))
            .map(|de| de.entry.clone())
            .collect()
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
            // produce two distinct keys, so that per-page prune mechanism can
            // never fire for it. Since #958 strengthened `diff_route_tables`
            // to full entry-set comparison, a stable-count output_path flip IS
            // re-rendered — but the old artifact is deleted by the GLOBAL
            // vanished-output diff (#804: the old path drops out of the live
            // route set and `route_vanished` prunes it from disk + cache), so
            // no orphan accumulates. If that global diff is ever weakened,
            // restore source-path keying for static routes (or key dynamic
            // entries on (source_path, output_path)).
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

    /// Look up the SSG [`RouteUniverseEntry`] for a request URL path
    /// (issue #1019 — reverse URL index).
    ///
    /// `request_path` must be:
    /// - Base-prefix-stripped (the server removes the prefix before
    ///   dispatching — the index never sees it).
    /// - May carry a query string (stripped here before lookup).
    /// - May be percent-encoded (decoded here before lookup so
    ///   `/posts/caf%C3%A9` resolves like `/posts/café`).
    ///
    /// Returns `Some(entry)` when a matching SSG route exists, `None` for
    /// SSR-only routes, dynamic routes that were never expanded, and any
    /// path not in the route universe. The returned entry is cloned out
    /// under a short read lock — no lock is held by the caller.
    // Dead until the lazy-adapter sub-issue consumes it (issue #1019).
    #[allow(dead_code)]
    fn lookup_by_url(&self, request_path: &str) -> Option<RouteUniverseEntry> {
        // Strip query string (everything from the first `?`).
        let path_only = request_path.split('?').next().unwrap_or(request_path);

        // Percent-decode (consistent with how `read_from_dist` treats paths).
        // On decode error fall back to the raw path so the lookup still
        // proceeds rather than panicking.
        let decoded = percent_decode_url(path_only);

        // Strip a leading slash so `url_index_lookup_keys` can prepend it
        // uniformly (the function expects the path WITHOUT a leading slash).
        let without_leading = decoded.trim_start_matches('/');

        let tables = self.inner.routes.read().unwrap_or_else(|p| p.into_inner());
        for key in url_index_lookup_keys(without_leading) {
            if let Some(entry) = tables.url_index.get(&key) {
                return Some(entry.clone());
            }
        }
        None
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
    ///
    /// A Phase-B skip during discovery is folded back into the empty
    /// change/vanish tuple here (pre-#956 behaviour): the discovery hook
    /// still reports `renderer_reloaded = true`, so the tick behaves as a
    /// completed refresh (issue #940 Inv 3).
    #[cfg(feature = "embed_v8")]
    fn discover_created(
        &self,
        created: &[PathBuf],
    ) -> Result<(Vec<PageId>, Vec<std::path::PathBuf>)> {
        if created.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        match self.refresh_bundle_and_routes()? {
            BundleRefresh::Skipped => Ok((Vec::new(), Vec::new())),
            BundleRefresh::Refreshed { changed, vanished } => Ok((changed, vanished)),
        }
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
    /// Returns [`BundleRefresh::Skipped`] when the Phase-B check (issue
    /// #940) proved nothing observable changed, otherwise
    /// [`BundleRefresh::Refreshed`] with:
    /// - `changed`: source [`PageId`]s whose route set changed
    ///   (empty for a plain content edit).
    /// - `vanished`: relative output paths (under dist) that
    ///   existed in the old live route set but are absent from the new one,
    ///   globally across all sources. Used by the caller to prune stale
    ///   HTML files and invalidate PageCache entries (issue #804).
    ///
    /// `embed_v8`-gated like `boot_dev_renderer` — the host start +
    /// `paths()` runtime eval need the embedded V8 host.
    #[cfg(feature = "embed_v8")]
    fn refresh_bundle_and_routes(&self) -> Result<BundleRefresh> {
        let project_root = &self.inner.project_root;
        let inputs = &self.inner.rebuild_inputs;

        // `ZFB_DEV_TIMING=1` — per-tick phase timing (issue #991).
        // Checked once here; all Instant::now() calls inside are skipped
        // when the flag is unset (zero overhead on the hot path).
        let timing_enabled = dev_timing_enabled();
        let tick_start = if timing_enabled {
            Some(std::time::Instant::now())
        } else {
            None
        };

        // P0 — router re-scan + route-universe plan.
        let p0_start = tick_start.map(|_| std::time::Instant::now());
        let pages_dir = project_root.join("pages");
        // Re-scan the router. `Router::scan` is unchanged by adding a
        // CONTENT file (the dynamic `[slug].tsx` source is the same), but
        // re-running it is cheap and keeps boot and rebuild symmetrical —
        // and it correctly picks up a brand-new `.tsx`/`.md` page placed
        // directly under `pages/` too.
        let router = zfb_router::Router::scan(&pages_dir).map_err(anyhow::Error::from)?;
        let plan = build_route_universe(router.routes());
        let p0_ms = p0_start.map(|t| t.elapsed().as_millis());

        // P1 — re-bundle with a fresh content snapshot (reads every
        //    configured collection from disk, so created AND edited
        //    entries are in the snapshot).
        //    Sub-phases: snapshot (content walk), assemble (BundlerInput
        //    construction incl. embedded esbuild extraction), bundle/esbuild
        //    (subprocess or embedded runner).
        let p1_snapshot_start = tick_start.map(|_| std::time::Instant::now());
        let bundle_result = {
            // #993 — the persistent shadow session lock is scoped to the
            // P1 bundle step only: it is released before the P2 renderer-
            // mutex work below, so it can never overlap the renderer
            // mutex (SSR latency unaffected). On a bundle error the `?`
            // drops the guard and propagates above the swap — the
            // previous renderer keeps serving and the session's dirty
            // flag (set inside bundle_with_session) forces a from-scratch
            // materialise next tick.
            let mut session_guard = self.inner.shadow_session.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    "shadow session mutex poisoned, recovered"
                );
                p.into_inner()
            });
            assemble_and_bundle_dev(
                project_root,
                &inputs.cfg,
                inputs.plugin_alias_entries.clone(),
                inputs.plugin_virtual_modules.clone(),
                timing_enabled,
                session_guard.as_mut(),
                inputs.esbuild_path(),
            )
            .context("dev refresh: re-bundle failed")?
        };
        let p1_total_ms = p1_snapshot_start.map(|t| t.elapsed().as_millis());
        // Sub-phase split is reported by assemble_and_bundle_dev into the
        // BundleSubTiming fields when timing_enabled.
        let p1_snapshot_ms = bundle_result.sub_timing.as_ref().map(|t| t.snapshot_ms);
        let p1_assemble_ms = bundle_result.sub_timing.as_ref().map(|t| t.assemble_ms);
        let p1_bundle_ms = bundle_result.sub_timing.as_ref().map(|t| t.bundle_ms);
        let bundler_out = bundle_result.output;

        // P1b — skip-key compute (SHA-256 over bundle + router + static HTML).
        // Phase B (issue #940) — skip key check.
        //
        // Compute a digest over bundle bytes + the router-scan signature
        // (sorted source paths + route templates) + static pages/**.html
        // bodies (issue #956 gate (a)). If the new key matches the previous
        // SUCCESSFUL tick's key, nothing observable changed:
        // - bundle bytes ≡ snapshot unchanged ≡ V8 host would observe
        //   identical globals,
        // - router signature unchanged ≡ pages/ universe identical,
        // - static-HTML bodies unchanged ≡ verbatim-copied routes identical.
        // Return `Skipped` — the discovery caller treats this as a
        // completed refresh (Inv 3: renderer_reloaded=true via the
        // DiscoveryOutcome path above us), while the per-tick reloader
        // propagates it so DevAssetPipeline bypasses the render fan-out
        // (issue #956).
        //
        // Key is stored only AFTER the full success path below (host swap +
        // route rebuild). A failed refresh — e.g. host start error — must
        // NOT update last_successful_skip_key so the next byte-identical
        // tick retries in full (Correctness Req 1, issue #940).
        let p1b_start = tick_start.map(|_| std::time::Instant::now());
        let new_skip_key = compute_bundle_skip_key(&bundler_out, router.routes());
        let p1b_ms = p1b_start.map(|t| t.elapsed().as_millis());
        if self.inner.should_skip_refresh(new_skip_key) {
            tracing::debug!(
                site = "refresh_bundle_and_routes",
                "bundle skip: byte-identical bundle + unchanged route universe; \
                 skipping V8 host boot and paths() re-expansion"
            );
            // Early return — the live host and route tables were left
            // untouched. The skip key is already stored from the last
            // successful tick; no update needed.
            return Ok(BundleRefresh::Skipped);
        }

        // P2 — V8 host boot, mutex swap, old-host shutdown (three separate
        //      sub-timers: boot vs eval vs teardown; split matters for the
        //      host-reuse decision).
        // 2. Start a NEW embedded V8 host against the rebuilt bundle,
        //    swap it into the existing mutex (the render callback + SSR
        //    adapter share this exact Arc), and shut the old host down
        //    only after the swap — a host-start failure must leave the
        //    previous renderer serving (see the method docs).
        let p2_boot_start = tick_start.map(|_| std::time::Instant::now());
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
        let p2_boot_ms = p2_boot_start.map(|t| t.elapsed().as_millis());

        let p2_swap_start = tick_start.map(|_| std::time::Instant::now());
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
        let p2_swap_ms = p2_swap_start.map(|t| t.elapsed().as_millis());

        let p2_shutdown_start = tick_start.map(|_| std::time::Instant::now());
        if let Some(prev) = previous {
            if let Err(err) = shutdown(prev) {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    error = %err,
                    "old renderer shutdown failed; continuing with new host"
                );
            }
        }
        let p2_shutdown_ms = p2_shutdown_start.map(|t| t.elapsed().as_millis());

        // Phase B (issue #940, review fix) — the live renderer just
        // diverged from whatever the stored skip key described. Invalidate
        // it NOW, before the fallible route-table rebuild below: if that
        // rebuild fails, the error `?`-returns without committing, and a
        // stale key from the pre-swap bundle could otherwise match a later
        // tick (e.g. the user undoing the edit) and skip — freezing the
        // failed tick's host in place. Note this is distinct from a
        // host-START failure, which `?`-returns ABOVE the swap and
        // correctly keeps the previous key (the live renderer was never
        // touched, so the old key still describes it).
        self.inner.commit_skip_key(None);

        // P3 — route-table rebuild (re-expands `paths()` through the new
        //      V8 host) with paths()-expansion sub-timing and this tick's
        //      PathsCache hit/miss deltas. The cache persists across
        //      ticks (#994 item B) — note the lookup happens AFTER the
        //      V8 dispatch (keyed on its result), so hits save only the
        //      Rust-side validate/URL-build, not the V8 evals.
        // 3. Rebuild the route tables through the reloaded host (re-expands
        //    `paths()`, so the dynamic source now resolves the new URL).
        let p3_start = tick_start.map(|_| std::time::Instant::now());
        let (new_routes_by_source, new_ssr_routes, new_url_index, p3_cache_hits, p3_cache_misses) = {
            let mut paths_cache = self.inner.paths_cache.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "refresh_bundle_and_routes",
                    "paths cache mutex poisoned, recovered"
                );
                p.into_inner()
            });
            build_dev_route_tables_timed(
                &router,
                &plan,
                project_root,
                &self.inner.renderer,
                &mut paths_cache,
            )
            .context("dev refresh: route-table rebuild failed")?
        };
        let p3_ms = p3_start.map(|t| t.elapsed().as_millis());

        // P4 — diff + RwLock table swap + skip-key commit.
        // 4. Diff against the frozen table — see [`diff_route_tables`]
        //    for the exact semantics (issue #958: the `changed` set is
        //    the G5 narrowing gate, so it compares full entry SETS, not
        //    just counts).
        let p4_start = tick_start.map(|_| std::time::Instant::now());
        let (changed, vanished_output_paths) = {
            let old = self.inner.routes.read().unwrap_or_else(|p| p.into_inner());
            diff_route_tables(&old.routes_by_source, &new_routes_by_source)
        };
        {
            let mut tables = self.inner.routes.write().unwrap_or_else(|p| p.into_inner());
            tables.routes_by_source = new_routes_by_source;
            tables.ssr_routes = new_ssr_routes;
            // Swap the url_index atomically with the other tables (issue #1019).
            tables.url_index = new_url_index;
        }
        // Issue #1025 — the route tables just moved: advance the stale
        // tick generation and evict stale entries whose output routes
        // vanished from the live route set (#804). Runs on EVERY full
        // refresh — including all-lazy ticks with zero eager renders —
        // and never on a Phase-B skip (early return above, #956).
        self.inner.note_table_swap(&vanished_output_paths);
        let p4_ms = p4_start.map(|t| t.elapsed().as_millis());

        // Phase B (issue #940) — commit-after-success.
        //
        // Store the skip key only here, after ALL of: host boot, swap, and
        // route-table rebuild have succeeded.  Any failure above returns via
        // `?` without reaching this point, leaving last_successful_skip_key
        // at its previous value so the next byte-identical tick rebuilds
        // fully (Correctness Req 1). See `commit_skip_key` for the
        // `None`-clears-the-key rationale.
        self.inner.commit_skip_key(new_skip_key);

        // Print one stderr line per tick when ZFB_DEV_TIMING=1.
        if let Some(tick_elapsed) = tick_start.map(|t| t.elapsed().as_millis()) {
            let p0 = p0_ms.unwrap_or(0);
            let p1 = p1_total_ms.unwrap_or(0);
            let p1_snap = p1_snapshot_ms.unwrap_or(0);
            let p1_asm = p1_assemble_ms.unwrap_or(0);
            let p1_bnd = p1_bundle_ms.unwrap_or(0);
            let p1b = p1b_ms.unwrap_or(0);
            let p2b = p2_boot_ms.unwrap_or(0);
            let p2s = p2_swap_ms.unwrap_or(0);
            let p2d = p2_shutdown_ms.unwrap_or(0);
            let p3 = p3_ms.unwrap_or(0);
            let p4 = p4_ms.unwrap_or(0);
            eprintln!(
                "[zfb-timing] tick={tick_elapsed}ms \
                 P0(router)={p0}ms \
                 P1(bundle)={p1}ms[snap={p1_snap}ms,asm={p1_asm}ms,esbuild={p1_bnd}ms] \
                 P1b(skip-key)={p1b}ms \
                 P2(host)[boot={p2b}ms,swap={p2s}ms,shutdown={p2d}ms] \
                 P3(routes)={p3}ms[cache-hits={p3_cache_hits},miss={p3_cache_misses}] \
                 P4(diff+swap)={p4}ms"
            );
        }

        Ok(BundleRefresh::Refreshed {
            changed,
            vanished: vanished_output_paths,
        })
    }
}

/// Result of one [`DevRenderSession::refresh_bundle_and_routes`] call —
/// the dev session's internal counterpart of
/// [`zfb_build::RefreshOutcome`] (issue #956), carrying the extra
/// `changed` source set the discovery path needs.
#[cfg(feature = "embed_v8")]
enum BundleRefresh {
    /// Phase-B skip (issue #940): byte-identical bundle + unchanged route
    /// universe (including static `pages/**.html` bodies). The live V8
    /// host and route tables were left untouched.
    Skipped,
    /// Full refresh completed (host swap + route-table rebuild).
    Refreshed {
        /// Source [`PageId`]s whose route-entry set changed (see
        /// [`diff_route_tables`]).
        changed: Vec<PageId>,
        /// Relative output paths (under dist) that vanished from the
        /// global live route set.
        vanished: Vec<std::path::PathBuf>,
    },
}

/// Diff a freshly-rebuilt `routes_by_source` map against the frozen one:
///
/// - `changed`: sources whose route-entry SET changed — new sources, and
///   sources whose entries differ in any way (URL, output path, params).
///   NOT a count-only comparison: issue #958's narrowing gate (fallback
///   G5) rides on this set, and a `paths()` refresh can replace routes
///   without changing their count (e.g. a two-page URL swap), which a
///   count diff reports as unchanged — a narrowed render would then skip
///   the brand-new route (silent under-render; review finding on #958).
///   The full-set comparison also feeds the watch-ADD discovery path,
///   where firing more often only re-renders more (safe direction).
/// - `vanished`: output paths that were live before but are absent from
///   every source in the new table. The diff is GLOBAL on purpose: if
///   route A loses /x while route B simultaneously gains /x, /x must NOT
///   be considered vanished (#727 two-page swap).
#[cfg(feature = "embed_v8")]
fn diff_route_tables(
    old: &HashMap<PathBuf, Vec<DevRouteEntry>>,
    new: &HashMap<PathBuf, Vec<DevRouteEntry>>,
) -> (Vec<PageId>, Vec<std::path::PathBuf>) {
    let changed: Vec<PageId> = new
        .iter()
        .filter(|(src, entries)| old.get(*src).map(|prev| prev != *entries).unwrap_or(true))
        .map(|(src, _)| PageId::new(src.clone()))
        .collect();

    // Collect the globally-live output_path sets for old and new. Use
    // HashSet for O(1) membership checks.
    let old_live: HashSet<&PathBuf> = old
        .values()
        .flat_map(|entries| entries.iter().map(|e| &e.entry.output_path))
        .collect();
    let new_live: HashSet<&PathBuf> = new
        .values()
        .flat_map(|entries| entries.iter().map(|e| &e.entry.output_path))
        .collect();
    let vanished: Vec<std::path::PathBuf> = old_live
        .difference(&new_live)
        .map(|p| (*p).clone())
        .collect();

    (changed, vanished)
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

// ---------------------------------------------------------------------------
// ZFB_DEV_TIMING — per-tick phase timing (issue #991)
// ---------------------------------------------------------------------------

/// Read `ZFB_DEV_TIMING` and decide whether to emit per-tick timing lines.
/// Truthy values: `1`, `true` (case-insensitive). Everything else — including
/// unset, empty, and unrecognized values — is off, so the hot path has zero
/// overhead.
#[cfg(feature = "embed_v8")]
fn dev_timing_enabled() -> bool {
    std::env::var("ZFB_DEV_TIMING")
        .ok()
        .as_deref()
        .map(|raw| {
            let t = raw.trim();
            t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Per-P1-sub-phase wall-clock durations collected inside
/// [`assemble_and_bundle_dev`] when `ZFB_DEV_TIMING=1`. Only populated when
/// `timing_enabled` is `true`; the caller always has `Some(...)` in that
/// case.
#[cfg(feature = "embed_v8")]
struct BundleSubTiming {
    /// `build_content_snapshot_json` — full content-collection walk.
    snapshot_ms: u128,
    /// `assemble_bundler_input` — BundlerInput construction incl. embedded
    /// esbuild/node_modules tempdir extraction.
    assemble_ms: u128,
    /// `bundle()` — esbuild subprocess / embedded runner.
    bundle_ms: u128,
}

/// Return value of [`assemble_and_bundle_dev`].
///
/// Wraps the `BundlerOutput` with an optional timing record that is `Some`
/// when `ZFB_DEV_TIMING=1` and `None` otherwise (zero allocation on the hot
/// path).
#[cfg(feature = "embed_v8")]
struct AssembledBundleResult {
    output: BundlerOutput,
    sub_timing: Option<BundleSubTiming>,
}

/// Assemble the dev-mode bundler input and run the bundler, returning the
/// fresh [`BundlerOutput`] wrapped in [`AssembledBundleResult`] (issue #659
/// — extracted from `boot_dev_renderer` so the watch-ADD re-bundle reuses
/// the EXACT same configuration the boot bundle used; any drift here would
/// make a newly-added page render differently in dev than it did at boot).
/// The embedded node_modules tempdir handle lives only for the
/// synchronous `bundle()` call (which writes `bundle_path` to disk), so
/// scoping it to this function is correct. The esbuild binary is the
/// boot-time process-lifetime extraction passed via `pre_resolved_esbuild`
/// (#994 item A); the per-call extraction only runs as a fallback when
/// that is `None`.
///
/// `recompute snapshot` is implicit: `build_content_snapshot_json` re-reads
/// the content collections from disk on every call, so a re-bundle here
/// picks up a content file created after boot.
///
/// `timing_enabled` — when `true`, record sub-phase wall-clock durations
/// into the returned [`BundleSubTiming`]. When `false`, no `Instant::now()`
/// calls are made (zero overhead on the hot path).
#[cfg(feature = "embed_v8")]
fn assemble_and_bundle_dev(
    project_root: &Path,
    cfg: &config::Config,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
    timing_enabled: bool,
    // Persistent dev shadow-tree session (issue #993). Both the boot
    // bundle and every refresh pass the SAME session through this seam,
    // so boot and refresh share one assembly path AND one shadow tree —
    // #659 configuration parity by construction.
    shadow_session: Option<&mut ShadowSession>,
    // Boot-time extracted embedded esbuild binary (#994 item A) — see
    // [`DevRebuildInputs::esbuild`]. Both boot and refresh pass the same
    // path so every tick skips the per-call tempdir extraction.
    pre_resolved_esbuild: Option<&Path>,
) -> Result<AssembledBundleResult> {
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
    let snap_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let content_snapshot_json =
        crate::commands::build::build_content_snapshot_json(project_root, cfg);
    let snapshot_ms = snap_start.map(|t| t.elapsed().as_millis()).unwrap_or(0);

    // The full ~25-field BundlerInput assembly is shared with `zfb build`
    // via `commands::bundler_input::assemble_bundler_input`. The two
    // per-command differences passed here:
    //   • BundleMode::Development  (build uses Production)
    //   • CssModuleFailMode::WarnAndEmpty  (build uses HardFail)
    let asm_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let crate::commands::bundler_input::AssembledBundlerInput {
        bundler_input,
        _node_modules_handle: _embedded_nm_handle,
        _esbuild_handle: _embedded_esbuild_handle,
    } = crate::commands::bundler_input::assemble_bundler_input(
        project_root,
        cfg,
        BundleMode::Development,
        crate::commands::bundler_input::CssModuleFailMode::WarnAndEmpty,
        content_snapshot_json,
        plugin_alias_entries,
        plugin_virtual_modules,
        pre_resolved_esbuild,
    )?;
    let assemble_ms = asm_start.map(|t| t.elapsed().as_millis()).unwrap_or(0);

    let bnd_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let bundler_out: BundlerOutput =
        bundle_with_session(bundler_input, shadow_session).context("bundler step failed")?;
    let bundle_ms = bnd_start.map(|t| t.elapsed().as_millis()).unwrap_or(0);

    let sub_timing = if timing_enabled {
        Some(BundleSubTiming {
            snapshot_ms,
            assemble_ms,
            bundle_ms,
        })
    } else {
        None
    };
    Ok(AssembledBundleResult {
        output: bundler_out,
        sub_timing,
    })
}

/// Compute the Phase-B skip key for a single dev refresh tick (issue #940).
///
/// The key is a SHA-256 digest of:
///
/// 1. **Bundle bytes** — the full content of `BundlerOutput.bundle_path` on
///    disk.  The content snapshot JSON is baked into the bundle by
///    `assemble_and_bundle_dev`, so identical bytes imply the snapshot (and
///    therefore everything the V8 host and `paths()` can observe) is
///    unchanged.
///
/// 2. **Router-scan signature** — a sorted list of
///    `(source_path, template)` pairs derived from `router.routes()`.
///    This defeats any `pages/` change that leaves bundle bytes identical
///    (e.g. a `.tsx` page added with no JS-visible impact on the snapshot),
///    satisfying Inv 2 (§3 of the roadmap, issue #935).
///
/// 3. **Static `pages/**.html` bodies** (issue #956 gate (a)) — routes
///    with `static_html = true` bypass the JS bundle entirely: the
///    renderer copies the source file from disk at render time
///    (`zfb-build/src/renderer.rs`), so neither the bundle bytes nor the
///    route signature reflect an edit to such a file's CONTENT. Hashing
///    the bodies here keeps a static-HTML edit from being swallowed by
///    the skip.
///
/// Returns `None` if the bundle file or any static-HTML source cannot be
/// read — the caller treats that as a forced full refresh (safe
/// direction: false-invalidate, never false-reuse).
#[cfg(feature = "embed_v8")]
fn compute_bundle_skip_key(
    bundler_out: &BundlerOutput,
    routes: &[zfb_router::Route],
) -> Option<[u8; 32]> {
    let bundle_bytes = match std::fs::read(&bundler_out.bundle_path) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(
                site = "compute_bundle_skip_key",
                path = %bundler_out.bundle_path.display(),
                error = %err,
                "could not read bundle file for skip-key; will perform full refresh"
            );
            return None;
        }
    };

    let mut hasher = Sha256::new();

    // Part 1: bundle bytes.
    hasher.update(b"bundle:");
    hasher.update((bundle_bytes.len() as u64).to_le_bytes());
    hasher.update(b":");
    hasher.update(&bundle_bytes);
    hasher.update(b"\n");

    // Part 2: router-scan signature — sorted (source_path, template) pairs.
    // Sorting by source_path gives a deterministic order regardless of the
    // filesystem walk order.
    let mut pairs: Vec<(String, String)> = routes
        .iter()
        .map(|r| (r.source_path.to_string_lossy().into_owned(), r.template()))
        .collect();
    pairs.sort_unstable();
    hasher.update(b"routes:");
    hasher.update((pairs.len() as u64).to_le_bytes());
    hasher.update(b":");
    for (src, tpl) in &pairs {
        hasher.update(src.as_bytes());
        hasher.update(b"=");
        hasher.update(tpl.as_bytes());
        hasher.update(b"\n");
    }

    // Part 3: static pages/**.html bodies (issue #956 gate (a)). Sorted by
    // source_path for a deterministic order; the paths come straight from
    // the router's WalkDir over `<project_root>/pages`, so reading them
    // here sees exactly the files the renderer would copy at render time.
    let mut static_html_routes: Vec<&zfb_router::Route> =
        routes.iter().filter(|r| r.static_html).collect();
    static_html_routes.sort_unstable_by(|a, b| a.source_path.cmp(&b.source_path));
    hasher.update(b"static-html:");
    hasher.update((static_html_routes.len() as u64).to_le_bytes());
    hasher.update(b":");
    for route in static_html_routes {
        let body = match std::fs::read(&route.source_path) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(
                    site = "compute_bundle_skip_key",
                    path = %route.source_path.display(),
                    error = %err,
                    "could not read static .html page for skip-key; \
                     will perform full refresh"
                );
                return None;
            }
        };
        hasher.update(route.source_path.to_string_lossy().as_bytes());
        hasher.update(b"=");
        hasher.update((body.len() as u64).to_le_bytes());
        hasher.update(b":");
        hasher.update(&body);
        hasher.update(b"\n");
    }

    Some(hasher.finalize().into())
}

/// `(routes_by_source, ssr_routes, url_index)` — the triple
/// [`build_dev_route_tables`] produces and [`DevRouteTables`] stores.
#[cfg(feature = "embed_v8")]
type BuiltRouteTables = (
    HashMap<PathBuf, Vec<DevRouteEntry>>,
    Vec<RouteUniverseEntry>,
    HashMap<String, RouteUniverseEntry>,
);

/// `(routes_by_source, ssr_routes, url_index, paths_cache_hits,
/// paths_cache_misses)` — the 5-tuple [`build_dev_route_tables_timed`]
/// returns with PathsCache stats exposed for ZFB_DEV_TIMING instrumentation
/// (issue #991).
#[cfg(feature = "embed_v8")]
type TimedRouteTables = (
    HashMap<PathBuf, Vec<DevRouteEntry>>,
    Vec<RouteUniverseEntry>,
    HashMap<String, RouteUniverseEntry>,
    u64,
    u64,
);

/// Timed variant of [`build_dev_route_tables`]: returns the same tables plus
/// the [`PathsCache`] hit and miss DELTAS for this call, for P3 diagnostics
/// (issue #991). The cache persists across ticks (#994 item B) and its
/// counters are cumulative, so the inner build reports per-call deltas.
#[cfg(feature = "embed_v8")]
fn build_dev_route_tables_timed(
    router: &zfb_router::Router,
    plan: &crate::render_pipeline::RouteUniversePlan,
    project_root: &Path,
    renderer: &Arc<Mutex<Option<RendererState>>>,
    paths_cache: &mut PathsCache,
) -> Result<TimedRouteTables> {
    let (routes_by_source, ssr_routes, url_index, hits, misses) =
        build_dev_route_tables_inner(router, plan, project_root, renderer, paths_cache)?;
    Ok((routes_by_source, ssr_routes, url_index, hits, misses))
}

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
    paths_cache: &mut PathsCache,
) -> Result<BuiltRouteTables> {
    let (routes_by_source, ssr_routes, url_index, _hits, _misses) =
        build_dev_route_tables_inner(router, plan, project_root, renderer, paths_cache)?;
    Ok((routes_by_source, ssr_routes, url_index))
}

/// Percent-decode a URL path segment, returning a borrowed `str` when the
/// input has no percent-encoded sequences or an owned `String` when decoding
/// produces a different value (issue #1019).
///
/// Consistent with how `read_from_dist` (zfb-server) handles incoming paths:
/// the Axum HTTP layer decodes the path before handing it to the handler, so
/// the index must decode too before doing a lookup. On invalid UTF-8 (malformed
/// percent-sequence) falls back to the raw input so the lookup can still
/// proceed (graceful degradation — the entry probably won't match, which is
/// the correct answer for a malformed URL).
// Dead until the lazy-adapter sub-issue consumes `lookup_by_url`.
#[allow(dead_code)]
fn percent_decode_url(path: &str) -> std::borrow::Cow<'_, str> {
    // Fast path: no `%` in the path means nothing to decode.
    if !path.contains('%') {
        return std::borrow::Cow::Borrowed(path);
    }
    // Decode percent-encoded sequences; keep the path as UTF-8.
    let bytes: Vec<u8> = percent_decode_bytes(path.as_bytes());
    match String::from_utf8(bytes) {
        Ok(s) => std::borrow::Cow::Owned(s),
        Err(_) => std::borrow::Cow::Borrowed(path),
    }
}

/// Decode percent-encoded sequences in a byte slice.
#[allow(dead_code)]
fn percent_decode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (from_hex_digit(input[i + 1]), from_hex_digit(input[i + 2]))
            {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// Convert an ASCII hex digit (`0`–`9`, `a`–`f`, `A`–`F`) to its numeric
/// value; returns `None` for any other byte.
#[allow(dead_code)]
fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Candidate lookup keys for a single SSG route's `url_path` (issue #1019).
///
/// Mirrors `lookup_keys` in `crates/zfb-server/src/routes.rs:1674` so a
/// request URL resolves to the index entry under the same normalisation rules
/// the HTTP layer applies:
///
/// - Trailing slashes are stripped before candidate generation so
///   `/posts/a` and `/posts/a/` map to the same entry.
/// - `/posts/a` → candidates `["/posts/a", "/posts/a/index.html",
///   "/posts/a/"]` covering both slash policies.
/// - Root `/` and the all-slashes edge case get the root candidates
///   `["/", "/index.html"]`.
/// - Routes that already end with a file extension (e.g. `feed.xml`) are
///   returned as-is — no `index.html` or slash variants.
///
/// Query strings and percent-encoding are NOT handled here; the caller
/// (`DevRenderSession::lookup_by_url`) normalises the input before
/// indexing into the table.
fn url_index_lookup_keys(url_path: &str) -> Vec<String> {
    let stripped = url_path.trim_end_matches('/');
    if stripped.is_empty() {
        // Root or all-slashes path.
        return vec!["/".to_string(), "/index.html".to_string()];
    }
    // Routes with an explicit file extension (e.g. `/feed.xml`, `/sitemap.xml`)
    // are served verbatim — no slash/index.html variants.
    let last_segment = stripped.rsplit('/').next().unwrap_or(stripped);
    if last_segment.contains('.') {
        return vec![format!("/{stripped}")];
    }
    vec![
        format!("/{stripped}"),
        format!("/{stripped}/index.html"),
        format!("/{stripped}/"),
    ]
}

/// Build the reverse URL-lookup index for all SSG routes (issue #1019).
///
/// Iterates every `DevRouteEntry` in `routes_by_source` (which contains only
/// SSG routes — SSR routes live in `ssr_routes` and are excluded by
/// construction) and inserts each entry under all candidate keys produced by
/// [`url_index_lookup_keys`]. When two SSG routes would claim the same
/// normalised key (unlikely in a well-formed project but possible if two
/// sources share an output path), the first writer wins and a warning is
/// emitted so the user knows a route is shadowed.
fn build_url_index(
    routes_by_source: &HashMap<PathBuf, Vec<DevRouteEntry>>,
) -> HashMap<String, RouteUniverseEntry> {
    let mut index: HashMap<String, RouteUniverseEntry> = HashMap::new();
    for entries in routes_by_source.values() {
        for dev_entry in entries {
            let entry = &dev_entry.entry;
            // Strip any leading slash before normalising — `url_path` values
            // in `RouteUniverseEntry` already start with `/`; passing them
            // through `url_index_lookup_keys` works because that function
            // re-adds the leading slash in its output.
            let url_no_leading = entry.url_path.trim_start_matches('/');
            for key in url_index_lookup_keys(url_no_leading) {
                if let Some(existing) = index.get(&key) {
                    if existing.url_path != entry.url_path {
                        crate::output::warn(format!(
                            "url_index: key {key:?} claimed by both {:?} and {:?}; \
                             the first entry wins",
                            existing.url_path, entry.url_path,
                        ));
                    }
                } else {
                    index.insert(key, entry.clone());
                }
            }
        }
    }
    index
}

/// Inner implementation shared by [`build_dev_route_tables`] and
/// [`build_dev_route_tables_timed`]. Returns the route tables plus this
/// call's PathsCache hit/miss deltas (#991 instrumentation; the cache is
/// caller-owned and persists across ticks per #994 item B, so the
/// cumulative counters are differenced here).
#[cfg(feature = "embed_v8")]
fn build_dev_route_tables_inner(
    router: &zfb_router::Router,
    plan: &crate::render_pipeline::RouteUniversePlan,
    project_root: &Path,
    renderer: &Arc<Mutex<Option<RendererState>>>,
    paths_cache: &mut PathsCache,
) -> Result<TimedRouteTables> {
    let prerender_map = build_prerender_map(router.routes(), project_root, |msg| {
        crate::output::warn(msg)
    });

    // Build the source-path → entries map once. Router source paths are
    // project-relative; PageId keys on the same value (the orchestrator
    // tracks pages by their source path). Each value is a Vec so a dynamic
    // SSG source can hold its N `paths()`-expanded entries (#502/#507);
    // static routes contribute a single-element vec.
    let mut routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
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
                // Static routes carry no `paths()` provenance (#958) —
                // their sources always render in full (fallback S1).
                routes_by_source
                    .entry(route.source_path.clone())
                    .or_default()
                    .push(DevRouteEntry {
                        entry: entry.clone(),
                        params: None,
                    });
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
    // PathsCache hit/miss DELTAS for this call — issue #991
    // instrumentation. The cache is caller-owned and persists across
    // ticks (#994 item B), so its cumulative counters are snapshotted
    // here and differenced at the end to feed the ZFB_DEV_TIMING P3
    // line.
    let hits_before = paths_cache.hit_count();
    let misses_before = paths_cache.miss_count();

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

        // Phase 1 — literal `paths()` arrays (no runtime needed).
        // A missing `paths()` export on an SSG route is a hard error here
        // too — consistent with `zfb build` (issue #520).
        let static_expansion = expand_dynamic_routes(&ssg_deferred, project_root, paths_cache)?;

        // Phase 2 — evaluate the routes phase 1 couldn't resolve statically
        // through the running embedded V8 host. We borrow the live host out
        // of the dev session's renderer mutex (the same `Arc<Mutex<Option<
        // RendererState>>>` the SSG render callback and the SSR adapter
        // share) and dispatch via `WorkerDispatch::EmbeddedV8`, exactly like
        // `commands/build.rs::eval_deferred_paths`.
        //
        // The renderer mutex is only taken when phase 1 left anything to
        // evaluate (#994 item B): when every `paths()` resolved
        // statically there is nothing to dispatch, so borrowing the live
        // host would be a pointless lock (and previously made an
        // all-literal project hard-require a started renderer here).
        let runtime_expansion = if static_expansion.deferred.is_empty() {
            crate::render_pipeline::DynamicExpansion::default()
        } else {
            let mut lock = renderer.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "boot_dev_renderer.paths",
                    "mutex poisoned, recovered"
                );
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
                paths_cache,
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
        //
        // Issue #958 — retain each URL's `paths()` params provenance by
        // zipping `resolved` with the parallel-ordered
        // `resolved_with_params` (documented invariant of
        // `DynamicExpansion`; both are built together in
        // `push_resolved_paths`). A length mismatch means the provenance
        // cannot be trusted: warn and file `params: None` for that whole
        // expansion, which downstream forces the affected sources to
        // always render in full (fallback S1) — degraded performance,
        // never under-rendering.
        for expansion in [static_expansion, runtime_expansion] {
            let params_aligned = expansion.resolved.len() == expansion.resolved_with_params.len();
            if !params_aligned {
                crate::output::warn(format!(
                    "dynamic route expansion: resolved/params length mismatch \
                     ({} vs {}); content-edit narrowing disabled for the \
                     affected source(s)",
                    expansion.resolved.len(),
                    expansion.resolved_with_params.len(),
                ));
            }
            let mut params_iter = expansion.resolved_with_params.into_iter();
            for entry in expansion.resolved {
                let params = if params_aligned {
                    params_iter.next().map(|p| p.params)
                } else {
                    None
                };
                if let Some(source) = template_to_source.get(&entry.route_key) {
                    routes_by_source
                        .entry(source.clone())
                        .or_default()
                        .push(DevRouteEntry { entry, params });
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
    }

    // Build the reverse URL-lookup index from all SSG entries (issue #1019).
    // SSR-only routes are already in `ssr_routes`, not `routes_by_source`,
    // so the index is naturally restricted to SSG routes.
    let url_index = build_url_index(&routes_by_source);

    Ok((
        routes_by_source,
        ssr_routes,
        url_index,
        paths_cache.hit_count() - hits_before,
        paths_cache.miss_count() - misses_before,
    ))
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
        // #994 item A — extract the embedded esbuild binary ONCE here;
        // the handle lives on `DevRebuildInputs` for the whole dev
        // session, so every tick's `assemble_bundler_input` skips its
        // per-call tempdir extraction. Skipped when `ZFB_ESBUILD_BIN`
        // overrides the embedded binary (the assemble-side skip
        // condition would ignore the path anyway). Extraction failure
        // is non-fatal, matching the assemble-side handling: warn and
        // fall back to the per-tick extraction by passing `None`.
        esbuild: if std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
            match crate::render_pipeline::embedded_binary("esbuild") {
                Ok(pair) => Some(pair),
                Err(e) => {
                    crate::output::warn(format!(
                        "could not extract embedded esbuild at dev boot ({e}); \
                         falling back to per-tick extraction"
                    ));
                    None
                }
            }
        } else {
            None
        },
    };

    // Persistent dev shadow-tree session (issue #993) — created once
    // here, used for the boot bundle below, then stored on
    // `DevRenderInner` so every refresh reuses the same shadow tree.
    let mut shadow_session = ShadowSession::new()?;

    // Boot path — timing not collected here (one-shot at startup, not a
    // hot-path tick). `timing_enabled = false` so no Instant::now() overhead.
    let bundler_out: BundlerOutput = assemble_and_bundle_dev(
        project_root,
        cfg,
        plugin_alias_entries,
        plugin_virtual_modules,
        false,
        Some(&mut shadow_session),
        rebuild_inputs.esbuild_path(),
    )?
    .output;

    let state = start(RendererStartInput {
        bundle_path: bundler_out.bundle_path.clone(),
        sourcemap_path: bundler_out.sourcemap_path.clone(),
        backend: Backend::EmbeddedV8 {
            host_factory: crate::v8_host_adapter::make_v8_host_factory_with_hooks(v8_plugin_hooks),
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
    //
    // #994 item B — the PathsCache is seeded here and stored on
    // `DevRenderInner` below, so every refresh tick reuses the entries
    // this boot-time build populated (boot/refresh parity: both go
    // through the same cache).
    let mut paths_cache = PathsCache::new();
    let (routes_by_source, ssr_routes, url_index) =
        build_dev_route_tables(&router, &plan, project_root, &renderer, &mut paths_cache)?;

    // Issue #958 — seed the frontmatter gate cache so the FIRST body-only
    // edit of a pre-existing collection file can already narrow (G4 only
    // fires for genuinely missing/changed frontmatter). ~1 read + parse +
    // hash per collection file; trivial against the bundle step above.
    let fm_hashes = seed_frontmatter_hashes(project_root, cfg);

    Ok(DevRenderSession {
        inner: Arc::new(DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
                url_index,
            }),
            renderer,
            project_root: project_root.to_path_buf(),
            rebuild_inputs,
            // No successful refresh yet — first tick always runs fully.
            last_successful_skip_key: Mutex::new(None),
            fm_hashes: Mutex::new(fm_hashes),
            shadow_session: Mutex::new(Some(shadow_session)),
            paths_cache: Mutex::new(paths_cache),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render: lazy_dev_render_enabled(),
        }),
    })
}

/// SHA-256 over the canonical JSON of a file's parsed frontmatter
/// (issue #958, fallback-G4 gate). `serde_json`'s default BTreeMap-backed
/// objects serialize with sorted keys, so the string is canonical: a YAML
/// reformat that parses to the same value hashes identically and does not
/// trip the gate. `None` when the file is unreadable or unparseable.
#[cfg(feature = "embed_v8")]
fn frontmatter_hash(path: &Path) -> Option<[u8; 32]> {
    let source = std::fs::read_to_string(path).ok()?;
    let uf = zfb_content::frontmatter::extract(path, &source).ok()?;
    let json = serde_json::to_string(&uf.value).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    Some(hasher.finalize().into())
}

/// Boot-seed the frontmatter gate cache (issue #958): hash every
/// configured collection file's frontmatter, keyed by absolute path.
/// Membership routes through `derive_slug_for_file` so the seeded set is
/// exactly the walker's. Unreadable / unparseable files (and collections
/// with uncompilable filter globs) are simply not seeded — their first
/// edit falls back to a full render (G4) and seeds the hash then.
#[cfg(feature = "embed_v8")]
fn seed_frontmatter_hashes(
    project_root: &Path,
    cfg: &config::Config,
) -> HashMap<PathBuf, [u8; 32]> {
    let mut hashes: HashMap<PathBuf, [u8; 32]> = HashMap::new();
    for collection in &cfg.collections {
        let root = project_root.join(normalize_relative(&collection.path));
        let filter = match zfb_content::collection::CollectionFilter::new(
            collection.include.as_deref(),
            collection.exclude.as_deref(),
            collection.id_strip_suffix.as_deref(),
        ) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for entry in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if zfb_content::collection::derive_slug_for_file(&root, path, &filter).is_none() {
                continue;
            }
            if let Some(hash) = frontmatter_hash(path) {
                hashes.insert(path.to_path_buf(), hash);
            }
        }
    }
    hashes
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

/// `true` when one of the edited entry's slug candidates appears among a
/// route's resolved params (issue #958, spec §4). ANY-param semantics:
/// scalars match by value, catchall arrays match by their `/`-join (an
/// empty catchall joins to `""`, matching the bare-root-index candidate);
/// a locale scalar never blocks a slug-array match. Exact byte equality —
/// no case folding, no percent-decoding.
#[cfg(feature = "embed_v8")]
fn params_match(
    p: &crate::render_pipeline::ResolvedRouteParams,
    candidates: &std::collections::BTreeSet<String>,
) -> bool {
    p.scalars.values().any(|v| candidates.contains(v))
        || p.arrays.values().any(|a| candidates.contains(&a.join("/")))
}

/// Compute the tick's narrowing decision from the plan's content-
/// narrowing hint (issue #958, spec §4). Runs once per render-callback
/// invocation. Every failure mode degrades to [`TickNarrowing::Off`] —
/// i.e. today's full fan-out — never to silent under-rendering:
///
/// - G2: a changed file outside every configured collection (or failing
///   its include/exclude globs, or an uncompilable filter glob),
/// - G3: file read / frontmatter parse error,
/// - G4: frontmatter hash missing or changed (frontmatter feeds
///   cross-page props — titles in sidebars/prev-next — so a frontmatter
///   delta re-renders the full selected set). The new hash is ALWAYS
///   stored, including on the Off path, so the next body-only edit
///   narrows.
///
/// Per-source fallbacks (source simply absent from the returned map ⇒
/// [`RouteFilter::All`]):
///
/// - S1: any entry lacks params provenance (static routes; zip-length
///   mismatch at table build time),
/// - S2: zero entries matched the candidate set (aggregate dynamic
///   consumers — tags/pagination — whose params are not slug-shaped).
#[cfg(feature = "embed_v8")]
fn compute_tick_narrowing(
    session: &DevRenderSession,
    hint: Option<&zfb_build::ContentNarrowing>,
) -> TickNarrowing {
    let Some(hint) = hint else {
        return TickNarrowing::Off;
    };
    if hint.changed_content.is_empty() {
        return TickNarrowing::Off;
    }
    let inner = &session.inner;

    let TickCandidates {
        candidates,
        gate_tripped,
    } = derive_tick_candidates(inner, hint, true);
    if gate_tripped || candidates.is_empty() {
        return TickNarrowing::Off;
    }

    // Steps 4+5 — per-source selection against the POST-refresh route
    // tables (the reloader ran before the render callback, so params are
    // fresh). Only narrowed sources enter the map; everything else — the
    // always-rendered set — renders in full by absence.
    let tables = inner.routes.read().unwrap_or_else(|p| p.into_inner());
    let per_source = match_candidate_routes(&tables, &candidates);
    if per_source.is_empty() {
        return TickNarrowing::Off;
    }
    tracing::debug!(
        narrowed_sources = per_source.len(),
        "content-edit narrowing active for tick (issue #958)"
    );
    TickNarrowing::PerSource(
        per_source
            .into_iter()
            .map(|(source, matched)| (source, RouteFilter::Only(matched)))
            .collect(),
    )
}

/// Result of [`derive_tick_candidates`] — the tick's slug candidate set
/// plus whether any of the #958 whole-tick gates tripped while deriving
/// it.
#[cfg(feature = "embed_v8")]
struct TickCandidates {
    /// Union of slug candidates across the changed files that passed
    /// their per-file gates.
    candidates: std::collections::BTreeSet<String>,
    /// True when any gate tripped: bad collection glob, G2 (file outside
    /// every collection), G3 (read/parse error), or G4 (missing/changed
    /// frontmatter). The #958 eager-narrowing caller treats a trip as
    /// "no narrowing this tick"; the #1025 lazy caller ignores it (a
    /// non-eager route is stale, never under-rendered).
    gate_tripped: bool,
}

/// Per-tick slug-candidate derivation shared by the #958 narrowing gate
/// ([`compute_tick_narrowing`]) and the #1025 lazy eager set
/// ([`compute_lazy_eager_sets`]). Spec §4 steps 1–3.
///
/// `frontmatter_gate_skips_candidates` selects the G4 semantics: `true`
/// (#958) suppresses a frontmatter-changed file's candidates — its
/// cross-page props force the full fan-out; `false` (#1025 lazy mode)
/// still collects them — frontmatter edits eager-render the entry's own
/// routes, and the cross-page fallout is covered by staling the
/// remainder. The gate HASH is stored in both modes (store-then-compare)
/// so the bookkeeping stays warm for whichever mode the next tick runs
/// in.
#[cfg(feature = "embed_v8")]
fn derive_tick_candidates(
    inner: &DevRenderInner,
    hint: &zfb_build::ContentNarrowing,
    frontmatter_gate_skips_candidates: bool,
) -> TickCandidates {
    use std::collections::BTreeSet;

    // Compile each collection's (root, filter) pair once for the tick. A
    // bad glob means membership cannot be evaluated reliably — trip the
    // gate and yield no candidates.
    let mut compiled: Vec<(PathBuf, zfb_content::collection::CollectionFilter)> =
        Vec::with_capacity(inner.rebuild_inputs.cfg.collections.len());
    for collection in &inner.rebuild_inputs.cfg.collections {
        match zfb_content::collection::CollectionFilter::new(
            collection.include.as_deref(),
            collection.exclude.as_deref(),
            collection.id_strip_suffix.as_deref(),
        ) {
            Ok(filter) => compiled.push((
                inner
                    .project_root
                    .join(normalize_relative(&collection.path)),
                filter,
            )),
            Err(err) => {
                tracing::warn!(
                    site = "derive_tick_candidates",
                    error = %err,
                    "collection filter failed to compile; no slug candidates this tick"
                );
                return TickCandidates {
                    candidates: BTreeSet::new(),
                    gate_tripped: true,
                };
            }
        }
    }

    let mut gate_tripped = false;
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    let mut fm_hashes = inner.fm_hashes.lock().unwrap_or_else(|p| p.into_inner());
    for file in &hint.changed_content {
        // Step 1 — collection resolution. A file may belong to
        // multiple collections; candidates are unioned. Zero
        // memberships ⇒ G2 (whole tick falls back), but keep
        // processing the other files so their gate hashes update.
        let slugs: Vec<String> = compiled
            .iter()
            .filter_map(|(root, filter)| {
                zfb_content::collection::derive_slug_for_file(root, file, filter)
            })
            .collect();
        if slugs.is_empty() {
            gate_tripped = true; // G2
            continue;
        }

        // Step 2 — frontmatter gate (G3/G4).
        let fm_value = match std::fs::read_to_string(file)
            .map_err(anyhow::Error::from)
            .and_then(|source| {
                zfb_content::frontmatter::extract(file, &source)
                    .map(|uf| uf.value)
                    .map_err(anyhow::Error::from)
            }) {
            Ok(value) => value,
            Err(_) => {
                // G3 — and the stored hash no longer describes the
                // file: drop it so the edit that FIXES the parse
                // error re-seeds via the G4 miss path.
                gate_tripped = true;
                fm_hashes.remove(file);
                continue;
            }
        };
        let hash: [u8; 32] = match serde_json::to_string(&fm_value) {
            Ok(json) => {
                let mut hasher = Sha256::new();
                hasher.update(json.as_bytes());
                hasher.finalize().into()
            }
            Err(_) => {
                gate_tripped = true;
                fm_hashes.remove(file);
                continue;
            }
        };
        // Store-then-compare: the new hash must land even when the
        // gate trips (G4), so the NEXT body-only edit narrows.
        let prev = fm_hashes.insert(file.clone(), hash);
        if prev != Some(hash) {
            gate_tripped = true; // G4 — missing or changed frontmatter.
            if frontmatter_gate_skips_candidates {
                continue;
            }
        }

        // Step 3 — slug candidate set.
        for slug in slugs {
            if slug == "index" {
                candidates.insert(String::new());
            }
            if let Some(stripped) = slug.strip_suffix("/index") {
                candidates.insert(stripped.to_string());
            }
            candidates.insert(slug);
        }
        // Frontmatter `slug:` override candidate (Docusaurus-style):
        // verbatim AND with one leading `/` stripped.
        if let Some(fm_slug) = fm_value.get("slug").and_then(|v| v.as_str()) {
            if let Some(stripped) = fm_slug.strip_prefix('/') {
                candidates.insert(stripped.to_string());
            }
            candidates.insert(fm_slug.to_string());
        }
    }
    TickCandidates {
        candidates,
        gate_tripped,
    }
}

/// Per-source candidate matching shared by the #958 narrowing gate and
/// the #1025 lazy eager set (spec §4 steps 4+5): map each dynamic
/// source to the output paths whose `paths()` params match a candidate.
/// Fallbacks are expressed by ABSENCE from the returned map:
///
/// - S1: any entry lacks params provenance (static routes; zip-length
///   mismatch at table build time),
/// - S2: zero entries matched the candidate set (aggregate dynamic
///   consumers — tags/pagination — whose params are not slug-shaped).
#[cfg(feature = "embed_v8")]
fn match_candidate_routes(
    tables: &DevRouteTables,
    candidates: &std::collections::BTreeSet<String>,
) -> HashMap<PathBuf, HashSet<PathBuf>> {
    let mut per_source: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
    for (source, entries) in &tables.routes_by_source {
        if entries.iter().any(|de| de.params.is_none()) {
            continue; // S1
        }
        let matched: HashSet<PathBuf> = entries
            .iter()
            .filter(|de| {
                de.params
                    .as_ref()
                    .is_some_and(|p| params_match(p, candidates))
            })
            .map(|de| de.entry.output_path.clone())
            .collect();
        if matched.is_empty() {
            continue; // S2
        }
        per_source.insert(source.clone(), matched);
    }
    per_source
}

/// Compute the lazy tick's EAGER set (issue #1025): for a content-edit
/// tick, the edited entries' own routes — the same candidate machinery
/// the #958 narrowing gate uses, with two lazy-mode differences:
///
/// - G4 is NOT a fallback: a frontmatter edit still eager-renders the
///   edited entry's own routes. The cross-page fallout frontmatter can
///   have (sidebar titles, prev/next labels) is covered by staling the
///   remainder of the fan-out instead of eagerly re-rendering it.
/// - The remaining gates degrade to "fewer eager routes" instead of a
///   full fan-out — "not eager" is safe in lazy mode because every
///   non-eager selected route is marked stale and re-renders on
///   request, never silently under-rendered.
///
/// Returns source → eager output-path set. An empty map (no hint —
/// `.tsx`/G5/G6/data/discovery ticks —, no candidates, bad globs, or
/// S1/S2-only sources) means a fully-lazy tick: every selected route is
/// marked stale and nothing renders eagerly.
#[cfg(feature = "embed_v8")]
fn compute_lazy_eager_sets(
    session: &DevRenderSession,
    hint: Option<&zfb_build::ContentNarrowing>,
) -> HashMap<PathBuf, HashSet<PathBuf>> {
    let Some(hint) = hint else {
        return HashMap::new();
    };
    if hint.changed_content.is_empty() {
        return HashMap::new();
    }
    let inner = &session.inner;
    let TickCandidates { candidates, .. } = derive_tick_candidates(inner, hint, false);
    if candidates.is_empty() {
        return HashMap::new();
    }
    let tables = inner.routes.read().unwrap_or_else(|p| p.into_inner());
    match_candidate_routes(&tables, &candidates)
}

/// Lazy dev render tick (issue #1025): the switch-ON replacement for
/// the fully-eager fan-out in [`make_render_callback`].
///
/// Eager-vs-stale split per tick class:
///
/// - Content edit with slug-derivable own routes (body OR frontmatter
///   edit): eager = the edited entries' own routes
///   ([`compute_lazy_eager_sets`]); stale = the remainder of the
///   selected fan-out — explicitly including S2 aggregate-heavy sources
///   (tag indexes, paginated lists) and S1 statics.
/// - Everything else (component/page `.tsx`, G5/G6 route-structure,
///   data edits, `renderer_fresh` discovery ticks — the hint is `None`
///   for all of these): eager = nothing; ALL selected routes are marked
///   stale.
///
/// A source unknown to the renderer contributes nothing (same no-op as
/// the eager path). An eager render FAILURE re-marks the eager set
/// stale so the request-time path retries against the live host, and is
/// logged without killing the watcher (same per-page tolerance as the
/// eager path).
///
/// ORDERING: the stale-state commit happens strictly BEFORE this
/// function returns. The return value flows through the pipeline's
/// write loop into the tick's `BuildOutcome`, so by the time the
/// outcome reaches `on_outcome` (the future SSE Page event) the
/// staleness map and the route tables (swapped by the reloader earlier
/// in the tick) already describe the new world — a reloading browser
/// can always resolve the stale route.
#[cfg(feature = "embed_v8")]
fn lazy_render_tick(
    session: &DevRenderSession,
    dist_dir: &Path,
    pages: &[PageId],
    narrowing: Option<&zfb_build::ContentNarrowing>,
) -> Result<Vec<RenderedPage>> {
    let eager_sets = compute_lazy_eager_sets(session, narrowing);

    let mut out: Vec<RenderedPage> = Vec::new();
    let mut stale: HashSet<PathBuf> = HashSet::new();
    let mut rendered_paths: Vec<PathBuf> = Vec::new();

    for page in pages {
        // Snapshot this source's output paths under a short read lock
        // (mirrors `render_one`'s clone-then-release discipline).
        let outputs: Vec<PathBuf> = {
            let tables = session
                .inner
                .routes
                .read()
                .unwrap_or_else(|p| p.into_inner());
            match tables.routes_by_source.get(page.path()) {
                Some(entries) => entries
                    .iter()
                    .map(|de| de.entry.output_path.clone())
                    .collect(),
                None => continue, // unknown to the renderer — no-op
            }
        };
        let eager = eager_sets.get(page.path());
        stale.extend(
            outputs
                .iter()
                .filter(|o| !eager.is_some_and(|set| set.contains(*o)))
                .cloned(),
        );

        let Some(eager) = eager else {
            continue; // fully-lazy source — nothing renders eagerly
        };
        let filter = RouteFilter::Only(eager.clone());
        match session.render_one(page, dist_dir, &filter) {
            Ok(rendered) => {
                rendered_paths.extend(
                    rendered
                        .iter()
                        .map(|r| r.output_path.as_path().to_path_buf()),
                );
                out.extend(rendered);
            }
            Err(err) => {
                // The eager routes did NOT reach disk — mark them stale
                // so the request-time path retries, and keep the
                // watcher alive (same tolerance as the eager callback).
                stale.extend(eager.iter().cloned());
                output::error(format!(
                    "renderer error for {}: {err:#}",
                    page.path().display()
                ));
            }
        }
    }

    // Commit the stale state BEFORE returning (see ORDERING above). The
    // eager-rendered routes are cleared last — fresh output supersedes
    // any staling from earlier ticks; the two sets are disjoint within
    // this tick.
    session.inner.mark_stale(stale);
    session.inner.clear_stale(&rendered_paths);
    Ok(out)
}

/// Build the [`PageRenderer`] callback that the orchestrator hands to
/// [`DevAssetPipeline`].
fn make_render_callback(session: DevRenderSession, dist_dir: PathBuf) -> PageRenderer {
    Arc::new(move |pages: &[PageId], narrowing| {
        // Issue #1025 — lazy dev render. Switch ON routes the tick
        // through the eager-vs-stale split; OFF (today's default) falls
        // through to the fully-eager fan-out below, untouched.
        #[cfg(feature = "embed_v8")]
        if session.inner.lazy_render {
            return lazy_render_tick(&session, &dist_dir, pages, narrowing);
        }
        // Issue #958 — one narrowing decision per tick; per-page filters
        // fall out of the per-source map. The V8-off path has no
        // collection configs to match against, so it never narrows.
        #[cfg(feature = "embed_v8")]
        let tick_narrowing = compute_tick_narrowing(&session, narrowing);
        #[cfg(not(feature = "embed_v8"))]
        let tick_narrowing = {
            let _ = narrowing;
            TickNarrowing::Off
        };
        let mut out = Vec::with_capacity(pages.len());
        for page in pages {
            let filter = match &tick_narrowing {
                TickNarrowing::Off => &RouteFilter::All,
                TickNarrowing::PerSource(map) => map.get(page.path()).unwrap_or(&RouteFilter::All),
            };
            match session.render_one(page, &dist_dir, filter) {
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
    let dispatcher: Arc<dyn SsrDispatcher> = Arc::new(
        crate::ssr_adapter::EmbeddedV8SsrAdapter::new(renderer_handle),
    );
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
/// - `:name{.+}?`   → `[[...name]]`   (optional catchall — zero or more)
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
            // Optional catchall (`:name{.+}?`) wins over required
            // (`:name{.+}`), which wins over single-segment.
            if let Some(name) = rest.strip_suffix("{.+}?") {
                out.push_str("[[...");
                out.push_str(name);
                out.push_str("]]");
            } else if let Some(name) = rest.strip_suffix("{.+}") {
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

    /// Wrap bare [`RouteUniverseEntry`]s in provenance-free
    /// [`DevRouteEntry`]s (issue #958) for tests that don't exercise
    /// narrowing.
    fn no_params(entries: Vec<RouteUniverseEntry>) -> Vec<DevRouteEntry> {
        entries
            .into_iter()
            .map(|entry| DevRouteEntry {
                entry,
                params: None,
            })
            .collect()
    }

    /// Build a stub [`DevRenderInner`] for the route-plumbing seam tests
    /// (no live V8 host). The discovery (#659) `rebuild_inputs` are filled
    /// with defaults — these tests never call `discover_created`.
    #[cfg(feature = "embed_v8")]
    fn stub_dev_inner(
        routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        stub_dev_inner_at(
            PathBuf::new(),
            config::Config::default(),
            routes_by_source,
            ssr_routes,
        )
    }

    /// [`stub_dev_inner`] with an explicit project root + config, for the
    /// content-narrowing tests (issue #958) that need real collection
    /// files on disk.
    #[cfg(feature = "embed_v8")]
    fn stub_dev_inner_at(
        project_root: PathBuf,
        cfg: config::Config,
        routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        let url_index = build_url_index(&routes_by_source);
        DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
                url_index,
            }),
            renderer: Arc::new(Mutex::new(None)),
            project_root,
            rebuild_inputs: DevRebuildInputs {
                cfg,
                v8_plugin_hooks: zfb_render::PluginRegistryHooks::default(),
                plugin_alias_entries: Vec::new(),
                plugin_virtual_modules: Vec::new(),
                esbuild: None,
            },
            last_successful_skip_key: Mutex::new(None),
            fm_hashes: Mutex::new(HashMap::new()),
            shadow_session: Mutex::new(None),
            paths_cache: Mutex::new(PathsCache::new()),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render: false,
        }
    }

    /// V8-off counterpart of [`stub_dev_inner`] — no `rebuild_inputs`
    /// field exists when `embed_v8` is disabled.
    #[cfg(not(feature = "embed_v8"))]
    fn stub_dev_inner(
        routes_by_source: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ssr_routes: Vec<RouteUniverseEntry>,
    ) -> DevRenderInner {
        let url_index = build_url_index(&routes_by_source);
        DevRenderInner {
            routes: std::sync::RwLock::new(DevRouteTables {
                routes_by_source,
                ssr_routes,
                url_index,
            }),
            renderer: Arc::new(Mutex::new(None)),
            project_root: PathBuf::new(),
            stale: Mutex::new(StaleRoutes::default()),
            lazy_render: false,
        }
    }

    #[test]
    fn default_watch_roots_includes_zfb_config_json() {
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.json"));
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.ts"));
    }

    /// #994 item B — the PathsCache is caller-owned and persists across
    /// route-table builds: a second build sharing the cache with
    /// unchanged `paths()` JSON reports cache hits (delta counters) and
    /// produces identical tables.
    #[test]
    #[cfg(feature = "embed_v8")]
    fn build_dev_route_tables_shares_paths_cache_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(pages.join("blog")).unwrap();
        std::fs::write(
            pages.join("index.tsx"),
            "export default function Home() { return null; }",
        )
        .unwrap();
        std::fs::write(
            pages.join("blog").join("[slug].tsx"),
            r#"
            export function paths() {
                return [
                    { params: { slug: "hello" } },
                    { params: { slug: "world" } },
                ];
            }
            export default function P() { return null; }
            "#,
        )
        .unwrap();

        let router = zfb_router::Router::scan(&pages).unwrap();
        let plan = build_route_universe(router.routes());
        // No live V8 host: the literal `paths()` resolves in phase 1, so
        // the renderer mutex is never borrowed (see the phase-2 gate in
        // `build_dev_route_tables_inner`).
        let renderer: Arc<Mutex<Option<RendererState>>> = Arc::new(Mutex::new(None));

        let mut cache = PathsCache::new();
        let (tables1, ssr1, idx1, hits1, misses1) =
            build_dev_route_tables_timed(&router, &plan, dir.path(), &renderer, &mut cache)
                .expect("first build");
        assert_eq!(hits1, 0, "first build runs against a cold cache");
        assert!(misses1 > 0, "first build must record a miss");
        assert_eq!(
            tables1
                .get(&pages.join("blog").join("[slug].tsx"))
                .map(Vec::len),
            Some(2),
            "literal paths() expands to 2 entries: {tables1:?}"
        );

        let (tables2, ssr2, idx2, hits2, misses2) =
            build_dev_route_tables_timed(&router, &plan, dir.path(), &renderer, &mut cache)
                .expect("second build");
        assert!(
            hits2 > 0,
            "unchanged paths() JSON must hit the shared cache"
        );
        assert_eq!(misses2, 0, "second build must add no misses");
        assert_eq!(
            tables1, tables2,
            "cache-sharing builds must produce identical tables"
        );
        assert_eq!(ssr1, ssr2);
        // The url_index must be consistent across both builds for the same input.
        assert_eq!(
            idx1.keys().collect::<std::collections::BTreeSet<_>>(),
            idx2.keys().collect::<std::collections::BTreeSet<_>>(),
            "url_index keys must be identical across cache-sharing builds"
        );
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
        let out = cb(&pages, None).unwrap();
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
        let out = DevRenderSession::render_one_with(&source_page, &entries, |entry| {
            let dest = tmp_path.join(&entry.output_path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(&dest, format!("<html>{}</html>", entry.url_path)).unwrap();
            Ok(dest)
        })
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
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            no_params(vec![RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
                static_html: false,
                source_path: None,
            }]),
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages, None).unwrap();
        assert!(out.is_empty(), "errors must yield empty list, not panic");
    }

    /// Content-edit narrowing seam tests (issue #958, locked-spec §11).
    ///
    /// These drive `compute_tick_narrowing` + `filter_entries` — the two
    /// seams a narrowed tick traverses before the (untouched)
    /// `render_one_with` fan-out — against real collection files in a
    /// tempdir, exactly as the dev render callback composes them.
    #[cfg(feature = "embed_v8")]
    mod narrowing {
        use super::*;
        use crate::render_pipeline::ResolvedRouteParams;
        use std::collections::BTreeMap;

        fn route_entry(url: &str, out: &str, key: &str) -> RouteUniverseEntry {
            RouteUniverseEntry {
                url_path: url.into(),
                output_path: PathBuf::from(out),
                route_key: key.into(),
                static_html: false,
                source_path: None,
            }
        }

        fn scalar_params(pairs: &[(&str, &str)]) -> ResolvedRouteParams {
            ResolvedRouteParams {
                scalars: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                arrays: BTreeMap::new(),
            }
        }

        fn array_params(key: &str, parts: &[&str]) -> ResolvedRouteParams {
            ResolvedRouteParams {
                scalars: BTreeMap::new(),
                arrays: BTreeMap::from([(
                    key.to_string(),
                    parts.iter().map(|s| s.to_string()).collect(),
                )]),
            }
        }

        fn with_params(entry: RouteUniverseEntry, params: ResolvedRouteParams) -> DevRouteEntry {
            DevRouteEntry {
                entry,
                params: Some(params),
            }
        }

        /// Project scaffold: tempdir root + one `blog` collection under
        /// `content/blog` with the given `(relative name, frontmatter)`
        /// entry files.
        fn scaffold(files: &[(&str, &str)]) -> (tempfile::TempDir, config::Config) {
            let tmp = tempfile::tempdir().unwrap();
            for (name, fm) in files {
                write_entry(tmp.path(), name, fm, "body");
            }
            let cfg = config::Config {
                collections: vec![config::CollectionDef {
                    name: "blog".into(),
                    path: PathBuf::from("content/blog"),
                    schema: None,
                    include: None,
                    exclude: None,
                    id_strip_suffix: None,
                }],
                ..config::Config::default()
            };
            (tmp, cfg)
        }

        fn write_entry(project_root: &Path, name: &str, fm: &str, body: &str) -> PathBuf {
            let path = project_root.join("content/blog").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("---\n{fm}\n---\n\n{body}\n")).unwrap();
            path
        }

        /// Stub session with boot-seeded frontmatter hashes, like
        /// `boot_dev_renderer` produces.
        fn session_at(
            tmp: &tempfile::TempDir,
            cfg: config::Config,
            routes: HashMap<PathBuf, Vec<DevRouteEntry>>,
        ) -> DevRenderSession {
            let seeded = seed_frontmatter_hashes(tmp.path(), &cfg);
            let inner = stub_dev_inner_at(tmp.path().to_path_buf(), cfg, routes, Vec::new());
            *inner.fm_hashes.lock().unwrap() = seeded;
            DevRenderSession {
                inner: Arc::new(inner),
            }
        }

        fn hint_for(paths: &[PathBuf]) -> zfb_build::ContentNarrowing {
            zfb_build::ContentNarrowing {
                changed_content: paths.to_vec(),
            }
        }

        /// Run the narrowed filter for `source` against the session's
        /// tables and return the surviving output paths — the set
        /// `render_one` would fan out (its `render_one_with` loop is
        /// byte-identical pre- and post-#958).
        fn surviving_outputs(
            session: &DevRenderSession,
            narrowing: &TickNarrowing,
            source: &Path,
        ) -> Vec<PathBuf> {
            let filter = match narrowing {
                TickNarrowing::Off => &RouteFilter::All,
                TickNarrowing::PerSource(map) => map.get(source).unwrap_or(&RouteFilter::All),
            };
            let tables = session.inner.routes.read().unwrap();
            DevRenderSession::filter_entries(&tables.routes_by_source[source], filter)
                .into_iter()
                .map(|e| e.output_path)
                .collect()
        }

        /// Epic acceptance test 1: a body edit of entry A renders A's
        /// route plus the static (always-rendered) source — NOT sibling
        /// B's route.
        #[test]
        fn narrowed_content_edit_renders_only_matching_routes_plus_statics() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A"), ("b.mdx", "title: B")]);
            let dynamic_source = PathBuf::from("pages/blog/[slug].tsx");
            let static_source = PathBuf::from("pages/index.tsx");
            let mut routes = HashMap::new();
            routes.insert(
                dynamic_source.clone(),
                vec![
                    with_params(
                        route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "a")]),
                    ),
                    with_params(
                        route_entry("/blog/b", "blog/b/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "b")]),
                    ),
                ],
            );
            routes.insert(
                static_source.clone(),
                no_params(vec![route_entry("/", "index.html", "/")]),
            );
            let session = session_at(&tmp, cfg, routes);

            // Body-only edit of A (frontmatter byte-identical).
            let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");

            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert!(
                matches!(narrowing, TickNarrowing::PerSource(_)),
                "a seeded body-only edit must narrow; got {narrowing:?}"
            );

            assert_eq!(
                surviving_outputs(&session, &narrowing, &dynamic_source),
                vec![PathBuf::from("blog/a/index.html")],
                "dynamic source must render exactly the edited entry's route"
            );
            assert_eq!(
                surviving_outputs(&session, &narrowing, &static_source),
                vec![PathBuf::from("index.html")],
                "the static source must stay in the render set in full (S1)"
            );
            if let TickNarrowing::PerSource(map) = &narrowing {
                assert!(
                    !map.contains_key(&static_source),
                    "always-rendered sources are expressed by ABSENCE from the map"
                );
            }
        }

        /// Epic acceptance test 2 / S2: a dynamic source whose params are
        /// not slug-shaped (tag pages) never matches the edited entry's
        /// candidates and must fall back to its FULL fan-out — that is
        /// the aggregate-page freshness mechanism.
        #[test]
        fn zero_match_dynamic_source_falls_back_to_full_fanout() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let blog_source = PathBuf::from("pages/blog/[slug].tsx");
            let tags_source = PathBuf::from("pages/tags/[tag].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                blog_source.clone(),
                vec![with_params(
                    route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "a")]),
                )],
            );
            routes.insert(
                tags_source.clone(),
                vec![
                    with_params(
                        route_entry("/tags/rust", "tags/rust/index.html", "/tags/:tag"),
                        scalar_params(&[("tag", "rust")]),
                    ),
                    with_params(
                        route_entry("/tags/cli", "tags/cli/index.html", "/tags/:tag"),
                        scalar_params(&[("tag", "cli")]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));

            let mut tag_outputs = surviving_outputs(&session, &narrowing, &tags_source);
            tag_outputs.sort();
            assert_eq!(
                tag_outputs,
                vec![
                    PathBuf::from("tags/cli/index.html"),
                    PathBuf::from("tags/rust/index.html"),
                ],
                "zero-match source must keep its full fan-out (S2)"
            );
            // The slug-shaped source still narrows on the same tick.
            assert_eq!(
                surviving_outputs(&session, &narrowing, &blog_source),
                vec![PathBuf::from("blog/a/index.html")],
            );
        }

        /// S1: a source where ANY entry lacks params provenance (zip-
        /// length mismatch at table-build time files `params: None`)
        /// must render in full even when another entry would match.
        #[test]
        fn missing_params_provenance_forces_source_full_fanout() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "a")]),
                    ),
                    DevRouteEntry {
                        entry: route_entry("/blog/b", "blog/b/index.html", "/blog/:slug"),
                        params: None,
                    },
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));

            let mut outputs = surviving_outputs(&session, &narrowing, &source);
            outputs.sort();
            assert_eq!(
                outputs,
                vec![
                    PathBuf::from("blog/a/index.html"),
                    PathBuf::from("blog/b/index.html"),
                ],
                "missing provenance on any entry must force the source's full fan-out (S1)"
            );
        }

        /// G4, changed direction: a frontmatter delta disables narrowing
        /// for the tick (frontmatter feeds cross-page props) — but the
        /// new hash is stored on the Off path, so the NEXT body-only
        /// edit narrows.
        #[test]
        fn frontmatter_change_disables_narrowing_for_tick() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![with_params(
                    route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "a")]),
                )],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "a.mdx", "title: A (renamed)", "body");
            let first =
                compute_tick_narrowing(&session, Some(&hint_for(std::slice::from_ref(&edited))));
            assert!(
                matches!(first, TickNarrowing::Off),
                "a frontmatter change must disable narrowing for the tick (G4); got {first:?}"
            );

            // Same file, body-only follow-up: the hash stored on the Off
            // path makes this tick narrow.
            let edited = write_entry(tmp.path(), "a.mdx", "title: A (renamed)", "body v2");
            let second = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert!(
                matches!(second, TickNarrowing::PerSource(_)),
                "the hash must be stored on the Off path so the next body edit narrows"
            );
        }

        /// G4, missing direction: a file with no seeded hash (boot
        /// seeding failed / session-created file) renders in full on its
        /// first edit and narrows from the second on.
        #[test]
        fn first_edit_without_seeded_hash_renders_full_then_narrows() {
            let (tmp, cfg) = scaffold(&[]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![with_params(
                    route_entry("/blog/new", "blog/new/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "new")]),
                )],
            );
            // Session seeded BEFORE the file exists — like a file created
            // mid-session through the discovery path.
            let session = session_at(&tmp, cfg, routes);
            let created = write_entry(tmp.path(), "new.mdx", "title: New", "body");

            let first =
                compute_tick_narrowing(&session, Some(&hint_for(std::slice::from_ref(&created))));
            assert!(
                matches!(first, TickNarrowing::Off),
                "first edit without a seeded hash must render in full (G4); got {first:?}"
            );

            let created = write_entry(tmp.path(), "new.mdx", "title: New", "body v2");
            let second = compute_tick_narrowing(&session, Some(&hint_for(&[created])));
            assert!(
                matches!(second, TickNarrowing::PerSource(_)),
                "second (body-only) edit must narrow once the hash is seeded"
            );
        }

        /// Spec §10: `x/index.mdx` must match catchall params `["x"]`
        /// via the `/index`-stripped candidate, and a root `index.mdx`
        /// must match the bare root-index `[]` (joins to `""`).
        #[test]
        fn index_entry_slug_variants_match_catchall_params() {
            let (tmp, cfg) = scaffold(&[("x/index.mdx", "title: X"), ("index.mdx", "title: Root")]);
            let source = PathBuf::from("pages/docs/[[...slug]].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry("/docs/x", "docs/x/index.html", "/docs/:slug{.+}?"),
                        array_params("slug", &["x"]),
                    ),
                    with_params(
                        route_entry("/docs", "docs/index.html", "/docs/:slug{.+}?"),
                        array_params("slug", &[]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "x/index.mdx", "title: X", "v2");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("docs/x/index.html")],
                "x/index.mdx must match the [\"x\"] catchall route"
            );

            let edited = write_entry(tmp.path(), "index.mdx", "title: Root", "v2");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("docs/index.html")],
                "root index.mdx must match the empty-catchall route"
            );
        }

        /// Spec §10: `idStripSuffix` applies inside the slug derivation,
        /// so `post.en.mdx` (suffix `.en`) matches params slug `post`.
        #[test]
        fn id_strip_suffix_slug_matches_params() {
            let (tmp, mut cfg) = scaffold(&[("post.en.mdx", "title: Post")]);
            cfg.collections[0].id_strip_suffix = Some(".en".into());
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry("/blog/post", "blog/post/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "post")]),
                    ),
                    with_params(
                        route_entry("/blog/other", "blog/other/index.html", "/blog/:slug"),
                        scalar_params(&[("slug", "other")]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(tmp.path(), "post.en.mdx", "title: Post", "v2");
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("blog/post/index.html")],
                "the idStripSuffix-stripped slug must match the route params"
            );
        }

        /// Spec §4 step 3: a frontmatter `slug:` override contributes a
        /// candidate (verbatim + leading-`/`-stripped), so a body edit of
        /// a file whose route URL comes from frontmatter still narrows.
        #[test]
        fn frontmatter_slug_override_body_edit_narrows_via_fm_candidate() {
            let (tmp, cfg) = scaffold(&[("weird-file-name.mdx", "title: C\nslug: /custom/path")]);
            let source = PathBuf::from("pages/docs/[[...slug]].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![
                    with_params(
                        route_entry(
                            "/docs/custom/path",
                            "docs/custom/path/index.html",
                            "/docs/:slug{.+}?",
                        ),
                        array_params("slug", &["custom", "path"]),
                    ),
                    with_params(
                        route_entry("/docs/other", "docs/other/index.html", "/docs/:slug{.+}?"),
                        array_params("slug", &["other"]),
                    ),
                ],
            );
            let session = session_at(&tmp, cfg, routes);

            let edited = write_entry(
                tmp.path(),
                "weird-file-name.mdx",
                "title: C\nslug: /custom/path",
                "v2",
            );
            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[edited])));
            assert_eq!(
                surviving_outputs(&session, &narrowing, &source),
                vec![PathBuf::from("docs/custom/path/index.html")],
                "the frontmatter slug candidate must match the override route"
            );
        }

        /// Review finding on #958 (G5 gate integrity): the refresh diff's
        /// `changed` set must compare full route-entry SETS, not entry
        /// counts — a `paths()` refresh that replaces a route with a new
        /// URL at the same cardinality must mark the source changed, or
        /// a narrowed tick could skip the brand-new route.
        #[test]
        fn diff_route_tables_flags_same_count_route_replacement() {
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let old_entries = vec![with_params(
                route_entry("/blog/x", "blog/x/index.html", "/blog/:slug"),
                scalar_params(&[("slug", "x")]),
            )];
            let new_entries = vec![with_params(
                route_entry("/blog/y", "blog/y/index.html", "/blog/:slug"),
                scalar_params(&[("slug", "y")]),
            )];
            let old = HashMap::from([(source.clone(), old_entries)]);
            let new = HashMap::from([(source.clone(), new_entries)]);

            let (changed, vanished) = diff_route_tables(&old, &new);
            assert_eq!(
                changed,
                vec![PageId::new(source)],
                "a same-count URL replacement must mark the source changed (G5)"
            );
            assert_eq!(
                vanished,
                vec![PathBuf::from("blog/x/index.html")],
                "the replaced output path globally vanished"
            );
        }

        /// `diff_route_tables` reports identical tables as unchanged, and
        /// the vanished diff stays GLOBAL: a route moving between sources
        /// (#727 two-page swap) flags both sources changed but vanishes
        /// nothing.
        #[test]
        fn diff_route_tables_identity_and_cross_source_swap() {
            let src_a = PathBuf::from("pages/a/[slug].tsx");
            let src_b = PathBuf::from("pages/b/[slug].tsx");
            let entry_x = with_params(
                route_entry("/x", "x/index.html", "/a/:slug"),
                scalar_params(&[("slug", "x")]),
            );
            let entry_y = with_params(
                route_entry("/y", "y/index.html", "/b/:slug"),
                scalar_params(&[("slug", "y")]),
            );

            let old = HashMap::from([
                (src_a.clone(), vec![entry_x.clone()]),
                (src_b.clone(), vec![entry_y.clone()]),
            ]);

            // Identity: nothing changed, nothing vanished.
            let (changed, vanished) = diff_route_tables(&old, &old.clone());
            assert!(changed.is_empty(), "identical tables must diff empty");
            assert!(vanished.is_empty());

            // Swap: A now serves /y, B now serves /x.
            let new = HashMap::from([
                (src_a.clone(), vec![entry_y]),
                (src_b.clone(), vec![entry_x]),
            ]);
            let (mut changed, vanished) = diff_route_tables(&old, &new);
            changed.sort_by(|a, b| a.path().cmp(b.path()));
            assert_eq!(changed, vec![PageId::new(src_a), PageId::new(src_b)]);
            assert!(
                vanished.is_empty(),
                "globally-live paths swapped between sources must not vanish"
            );
        }

        /// G2: a Content-classified file outside every configured
        /// collection (e.g. a bare `content/`-segment file with no
        /// matching collection) must disable narrowing for the tick.
        #[test]
        fn file_outside_every_collection_disables_narrowing() {
            let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
            let source = PathBuf::from("pages/blog/[slug].tsx");
            let mut routes = HashMap::new();
            routes.insert(
                source.clone(),
                vec![with_params(
                    route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                    scalar_params(&[("slug", "a")]),
                )],
            );
            let session = session_at(&tmp, cfg, routes);

            let outside = tmp.path().join("content/notes/x.md");
            std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
            std::fs::write(&outside, "---\ntitle: X\n---\n\nbody\n").unwrap();

            let narrowing = compute_tick_narrowing(&session, Some(&hint_for(&[outside])));
            assert!(
                matches!(narrowing, TickNarrowing::Off),
                "a file outside every collection must fall back to full fan-out (G2)"
            );
        }

        /// Staleness model seam tests (issue #1025): the claim/clear
        /// protocol on [`StaleRoutes`] and the lazy eager-vs-stale split
        /// (`compute_lazy_eager_sets` + `lazy_render_tick`), driven
        /// against real collection files exactly like the narrowing
        /// tests above. The orchestrator-layer tick-class matrix lives
        /// in `crates/zfb-build/tests/integration_lazy_staleness.rs`.
        mod stale_model {
            use super::*;

            /// [`session_at`] with the lazy-render switch forced ON.
            fn lazy_session_at(
                tmp: &tempfile::TempDir,
                cfg: config::Config,
                routes: HashMap<PathBuf, Vec<DevRouteEntry>>,
            ) -> DevRenderSession {
                let seeded = seed_frontmatter_hashes(tmp.path(), &cfg);
                let mut inner =
                    stub_dev_inner_at(tmp.path().to_path_buf(), cfg, routes, Vec::new());
                *inner.fm_hashes.lock().unwrap() = seeded;
                inner.lazy_render = true;
                DevRenderSession {
                    inner: Arc::new(inner),
                }
            }

            fn bare_inner() -> DevRenderInner {
                stub_dev_inner(HashMap::new(), Vec::new())
            }

            fn out(p: &str) -> PathBuf {
                PathBuf::from(p)
            }

            // ── claim / clear_if_current protocol ────────────────────

            /// The activation default must stay OFF this wave — the
            /// wave-5 sub-issue flips this single constant.
            // The constant IS the subject under test: this pin makes a
            // premature default flip fail loudly in review.
            #[allow(clippy::assertions_on_constants)]
            #[test]
            fn lazy_dev_render_default_is_off() {
                assert!(
                    !LAZY_DEV_RENDER_DEFAULT,
                    "the lazy dev-render switch must default OFF until the \
                     activation sub-issue flips it"
                );
            }

            #[test]
            fn claim_returns_token_for_stale_route_and_none_for_fresh() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);

                let claim = inner
                    .claim(Path::new("blog/a/index.html"))
                    .expect("stale route must be claimable");
                assert_eq!(claim.output_path, out("blog/a/index.html"));
                assert_eq!(claim.generation, 0, "boot generation is 0");

                assert!(
                    inner.claim(Path::new("blog/b/index.html")).is_none(),
                    "a route that was never staled must not claim"
                );
            }

            #[test]
            fn clear_if_current_clears_unsuperseded_claim() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);
                let claim = inner.claim(Path::new("blog/a/index.html")).unwrap();

                inner.clear_if_current(&claim);
                assert!(
                    inner.claim(Path::new("blog/a/index.html")).is_none(),
                    "an unsuperseded claim must clear the stale entry"
                );
            }

            /// THE ABA case (issue #1025 pinned design): claim at
            /// generation N, tick swap bumps to N+1 and re-stales the
            /// route — `clear_if_current` with the stale N-claim must
            /// NOT clear; the route stays stale for the next request.
            #[test]
            fn aba_re_staled_route_survives_clear_of_older_claim() {
                let inner = bare_inner();
                inner.mark_stale([out("blog/a/index.html")]);
                let old_claim = inner.claim(Path::new("blog/a/index.html")).unwrap();
                assert_eq!(old_claim.generation, 0);

                // A new tick: P4 table swap bumps the generation, then
                // the tick's render callback re-stales the same route.
                inner.note_table_swap(&[]);
                inner.mark_stale([out("blog/a/index.html")]);

                inner.clear_if_current(&old_claim);

                let still = inner
                    .claim(Path::new("blog/a/index.html"))
                    .expect("route re-staled at a newer generation must survive the old clear");
                assert_eq!(
                    still.generation, 1,
                    "the surviving entry is the re-staled one"
                );
            }

            #[test]
            fn table_swap_evicts_vanished_stale_entries() {
                let inner = bare_inner();
                inner.mark_stale([out("gone/index.html"), out("kept/index.html")]);

                inner.note_table_swap(&[out("gone/index.html")]);

                assert!(
                    inner.claim(Path::new("gone/index.html")).is_none(),
                    "a vanished route must be evicted from the stale set (#804)"
                );
                assert!(
                    inner.claim(Path::new("kept/index.html")).is_some(),
                    "unrelated stale entries must survive the swap"
                );
            }

            #[test]
            fn take_tick_stale_drains_once_sorted() {
                let inner = bare_inner();
                inner.mark_stale([out("b.html"), out("a.html"), out("b.html")]);

                assert_eq!(
                    inner.take_tick_stale(),
                    vec![out("a.html"), out("b.html")],
                    "tick buffer drains sorted + deduped"
                );
                assert!(
                    inner.take_tick_stale().is_empty(),
                    "second drain in the same tick must be empty"
                );
                assert!(
                    inner.claim(Path::new("a.html")).is_some(),
                    "draining the tick buffer must not clear the stale entries"
                );
            }

            #[test]
            fn clear_stale_drops_rendered_routes() {
                let inner = bare_inner();
                inner.mark_stale([out("a.html"), out("b.html")]);

                inner.clear_stale(&[out("a.html")]);

                assert!(inner.claim(Path::new("a.html")).is_none());
                assert!(inner.claim(Path::new("b.html")).is_some());
            }

            /// Vanished routes are also dropped from the CURRENT tick's
            /// stale buffer so `pages_stale` never announces a route
            /// that no longer resolves.
            #[test]
            fn table_swap_drops_vanished_from_tick_buffer() {
                let inner = bare_inner();
                inner.mark_stale([out("gone.html"), out("kept.html")]);

                inner.note_table_swap(&[out("gone.html")]);

                assert_eq!(inner.take_tick_stale(), vec![out("kept.html")]);
            }

            // ── lazy eager-vs-stale split ────────────────────────────

            /// Routes for the canonical three-source matrix: a slug
            /// blog source (own routes), an S2 tag-index aggregate
            /// (params not slug-shaped), and an S1 static index.
            fn matrix_routes() -> HashMap<PathBuf, Vec<DevRouteEntry>> {
                let mut routes = HashMap::new();
                routes.insert(
                    PathBuf::from("pages/blog/[slug].tsx"),
                    vec![
                        with_params(
                            route_entry("/blog/a", "blog/a/index.html", "/blog/:slug"),
                            scalar_params(&[("slug", "a")]),
                        ),
                        with_params(
                            route_entry("/blog/b", "blog/b/index.html", "/blog/:slug"),
                            scalar_params(&[("slug", "b")]),
                        ),
                    ],
                );
                routes.insert(
                    PathBuf::from("pages/tags/[tag].tsx"),
                    vec![with_params(
                        route_entry("/tags/rust", "tags/rust/index.html", "/tags/:tag"),
                        scalar_params(&[("tag", "rust")]),
                    )],
                );
                routes.insert(
                    PathBuf::from("pages/index.tsx"),
                    no_params(vec![route_entry("/", "index.html", "/")]),
                );
                routes
            }

            fn matrix_pages() -> Vec<PageId> {
                vec![
                    PageId::new(PathBuf::from("pages/blog/[slug].tsx")),
                    PageId::new(PathBuf::from("pages/tags/[tag].tsx")),
                    PageId::new(PathBuf::from("pages/index.tsx")),
                ]
            }

            /// Body edit: eager = the edited entry's own routes only;
            /// the S2 aggregate and the S1 static are NOT eager (they
            /// become the stale remainder).
            #[test]
            fn lazy_eager_sets_body_edit_selects_own_routes_only() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");
                let eager = compute_lazy_eager_sets(&session, Some(&hint_for(&[edited])));

                assert_eq!(
                    eager.get(Path::new("pages/blog/[slug].tsx")),
                    Some(&HashSet::from([out("blog/a/index.html")])),
                    "the edited entry's own route is the eager set"
                );
                assert!(
                    !eager.contains_key(Path::new("pages/tags/[tag].tsx")),
                    "S2 aggregate sources must not be eager"
                );
                assert!(
                    !eager.contains_key(Path::new("pages/index.tsx")),
                    "S1 static sources must not be eager"
                );
            }

            /// THE G4 divergence from #958: a FRONTMATTER edit still
            /// eager-renders the edited entry's own routes in lazy mode
            /// (the cross-page fallout is staled, not eagerly
            /// re-rendered) — while the eager-narrowing gate of the
            /// switch-OFF path keeps returning Off for the same tick.
            #[test]
            fn lazy_eager_sets_frontmatter_edit_still_selects_own_routes() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let edited = write_entry(tmp.path(), "a.mdx", "title: A (renamed)", "body");
                let eager = compute_lazy_eager_sets(
                    &session,
                    Some(&hint_for(std::slice::from_ref(&edited))),
                );

                assert_eq!(
                    eager.get(Path::new("pages/blog/[slug].tsx")),
                    Some(&HashSet::from([out("blog/a/index.html")])),
                    "a frontmatter edit must still yield the entry's own routes (no G4 fallback in lazy mode)"
                );
            }

            /// No hint (`.tsx`, G5/G6, data, discovery ticks) ⇒ no
            /// eager routes at all.
            #[test]
            fn lazy_eager_sets_empty_without_hint() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                assert!(compute_lazy_eager_sets(&session, None).is_empty());
            }

            /// `.tsx`-style tick (hint = None): every selected route is
            /// marked stale, nothing renders, unknown sources are
            /// no-ops.
            #[test]
            fn lazy_tick_without_hint_marks_all_selected_stale() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let mut pages = matrix_pages();
                pages.push(PageId::new(PathBuf::from("pages/unknown.tsx")));
                let rendered =
                    lazy_render_tick(&session, tmp.path(), &pages, None).expect("tick succeeds");

                assert!(rendered.is_empty(), "an all-lazy tick renders nothing");
                assert_eq!(
                    session.inner.take_tick_stale(),
                    vec![
                        out("blog/a/index.html"),
                        out("blog/b/index.html"),
                        out("index.html"),
                        out("tags/rust/index.html"),
                    ],
                    "every selected route (and only those) is staled"
                );
                assert!(
                    session
                        .inner
                        .claim(Path::new("blog/a/index.html"))
                        .is_some(),
                    "stale entries persist past the tick-buffer drain"
                );
            }

            /// Body edit through the full lazy tick: the S2 aggregate
            /// and the S1 static are marked stale (NOT rendered); the
            /// own route is attempted eagerly — and because this stub
            /// session has no live renderer, the failed eager render
            /// re-stales it so the request path can retry.
            #[test]
            fn lazy_tick_body_edit_stales_remainder_and_failed_eager() {
                let (tmp, cfg) = scaffold(&[("a.mdx", "title: A")]);
                let session = lazy_session_at(&tmp, cfg, matrix_routes());

                let edited = write_entry(tmp.path(), "a.mdx", "title: A", "updated body");
                let hint = hint_for(&[edited]);
                let rendered = lazy_render_tick(&session, tmp.path(), &matrix_pages(), Some(&hint))
                    .expect("a failed eager render keeps the watcher alive");

                assert!(rendered.is_empty(), "no live renderer in this stub");
                assert_eq!(
                    session.inner.take_tick_stale(),
                    vec![
                        // own route: re-staled by the failed eager render
                        out("blog/a/index.html"),
                        // remainder: sibling + S1 static + S2 aggregate
                        out("blog/b/index.html"),
                        out("index.html"),
                        out("tags/rust/index.html"),
                    ],
                );
            }

            /// The switch itself: lazy ON routes the render callback
            /// through the stale-marking split; lazy OFF (default)
            /// leaves the stale set untouched on an identical tick.
            #[test]
            fn render_callback_respects_lazy_switch() {
                // ON: the static route is marked stale instead of rendered.
                let (tmp_on, cfg_on) = scaffold(&[]);
                let session_on = lazy_session_at(&tmp_on, cfg_on, matrix_routes());
                let cb_on = make_render_callback(session_on.clone(), tmp_on.path().to_path_buf());
                let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
                let out_on = cb_on(&pages, None).expect("lazy tick succeeds");
                assert!(out_on.is_empty());
                assert!(
                    session_on.inner.claim(Path::new("index.html")).is_some(),
                    "switch ON must mark the selected route stale"
                );

                // OFF: same tick shape goes down the eager path and the
                // stale set stays empty (render errors are swallowed by
                // the existing per-page tolerance).
                let (tmp_off, cfg_off) = scaffold(&[]);
                let session_off = session_at(&tmp_off, cfg_off, matrix_routes());
                let cb_off =
                    make_render_callback(session_off.clone(), tmp_off.path().to_path_buf());
                let out_off = cb_off(&pages, None).expect("eager tick tolerates render errors");
                assert!(out_off.is_empty());
                assert!(
                    session_off.inner.claim(Path::new("index.html")).is_none(),
                    "switch OFF must never touch the stale set"
                );
                assert!(
                    session_off.inner.take_tick_stale().is_empty(),
                    "switch OFF must never announce stale routes"
                );
            }
        }
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
        // Optional catchall (`:name{.+}?`) → `[[...name]]`.
        assert_eq!(
            colon_template_to_bracket("/docs/:rest{.+}?"),
            "/docs/[[...rest]]",
        );
        assert_eq!(colon_template_to_bracket("/:rest{.+}?"), "/[[...rest]]",);
        assert_eq!(colon_template_to_bracket("/a/:b/c/:d"), "/a/[b]/c/[d]",);
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
        assert_eq!(
            patterns,
            vec!["/blog/[slug]".to_string(), "/api/x".to_string()]
        );
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
        let gen2 = vec![make_companion("islands-chunk-GEN2CCCC.js", b"gen2-c")];
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

        assert!(
            assets.join(shared_chunk).exists(),
            "kept chunk must still exist"
        );
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

        let bad_names = ["../escape.js", "subdir/chunk.js", "subdir\\chunk.js", ""];
        for name in bad_names {
            let companion = make_companion(name, b"bytes");
            let result = refresh_dev_island_chunks(&assets, &[companion], &HashSet::new());
            assert!(result.is_err(), "should reject unsafe filename {:?}", name);
        }
    }

    // ── Phase B skip-key tests (issue #940) ─────────────────────────────────
    //
    // All `embed_v8`-gated: `compute_bundle_skip_key` and the
    // `last_successful_skip_key` seams only exist on the V8 path.

    /// Helper: write `bytes` to a temp file and return its path.
    #[cfg(feature = "embed_v8")]
    fn write_temp_bundle(dir: &tempfile::TempDir, bytes: &[u8]) -> PathBuf {
        let p = dir.path().join("bundle.js");
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// Build a minimal `BundlerOutput` pointing at an existing file.
    #[cfg(feature = "embed_v8")]
    fn make_bundler_out(bundle_path: PathBuf) -> BundlerOutput {
        use zfb_build::bundler::BundleManifest;
        BundlerOutput {
            bundle_path: bundle_path.clone(),
            sourcemap_path: bundle_path.with_extension("js.map"),
            manifest: BundleManifest {
                framework: "preact".into(),
                jsx_import_source: "preact".into(),
                hydrate_shim_specifier: "zfb:internal/hydrate".into(),
                bundle_basename: "bundle.js".into(),
                routes: Vec::new(),
            },
        }
    }

    /// `compute_bundle_skip_key` returns `Some` for a valid bundle file.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_returns_some_for_readable_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"bundle-content"));
        let key = compute_bundle_skip_key(&out, &[]);
        assert!(key.is_some(), "should produce a key for a readable bundle");
    }

    /// `compute_bundle_skip_key` returns `None` when the bundle path does not
    /// exist — the caller must treat this as a forced full refresh.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_returns_none_for_missing_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(dir.path().join("nonexistent.js"));
        let key = compute_bundle_skip_key(&out, &[]);
        assert!(key.is_none(), "missing bundle must yield None (no skip)");
    }

    /// Same bundle bytes + same routes → identical keys.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_identical_for_same_bundle_and_routes() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"same-bytes"));
        let k1 = compute_bundle_skip_key(&out, &[]);
        let k2 = compute_bundle_skip_key(&out, &[]);
        assert_eq!(k1, k2, "identical inputs must produce identical keys");
    }

    /// Different bundle bytes → different keys (real edit defeats skip).
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_changes_on_different_bundle_bytes() {
        let dir = tempfile::tempdir().unwrap();

        let out1 = make_bundler_out(write_temp_bundle(&dir, b"bundle-v1"));
        let k1 = compute_bundle_skip_key(&out1, &[]);

        // Overwrite same file with different content.
        std::fs::write(&out1.bundle_path, b"bundle-v2").unwrap();
        let k2 = compute_bundle_skip_key(&out1, &[]);

        assert_ne!(k1, k2, "changed bundle bytes must change the skip key");
    }

    /// A `pages/` route change (different source paths) defeats the skip even
    /// when bundle bytes are identical — satisfies Inv 2 (issue #935 §3).
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_changes_on_route_universe_change() {
        use zfb_router::Route;

        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"identical-bundle"));

        // Build two Route slices with different source paths.
        let make_route = |src: &str, segs: Vec<zfb_router::Segment>| Route {
            source_path: PathBuf::from(src),
            segments: segs,
            kind: zfb_router::RouteKind::Static,
            specificity: 1,
            output_extension: None,
            static_html: false,
        };

        let routes_a = vec![make_route("pages/index.tsx", vec![])];
        let routes_b = vec![
            make_route("pages/index.tsx", vec![]),
            make_route(
                "pages/about.tsx",
                vec![zfb_router::Segment::Static("about".into())],
            ),
        ];

        let k_a = compute_bundle_skip_key(&out, &routes_a);
        let k_b = compute_bundle_skip_key(&out, &routes_b);

        assert!(k_a.is_some());
        assert!(k_b.is_some());
        assert_ne!(
            k_a, k_b,
            "different route universe must change the skip key \
             even when bundle bytes are identical"
        );
    }

    /// Phase B — the first tick after boot must never skip: a fresh
    /// `DevRenderInner` holds no skip key, so `should_skip_refresh` is
    /// false for any computed key.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn fresh_dev_inner_never_skips() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        assert!(
            !inner.should_skip_refresh(Some([0x11u8; 32])),
            "a freshly-constructed DevRenderInner must not skip — \
             the first tick always runs fully"
        );
    }

    /// Phase B — a no-op tick skips: after a successful refresh committed
    /// key K, the next tick computing the same K skips host boot +
    /// `paths()` re-expansion. A different key (real edit) does not skip.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn identical_key_skips_after_successful_commit_and_real_edit_does_not() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_k = [0xAAu8; 32];
        let key_edit = [0xBBu8; 32];

        // Tick 1: full refresh succeeds → commit K.
        inner.commit_skip_key(Some(key_k));

        // Tick 2: byte-identical bundle + unchanged routes → same key → skip.
        assert!(
            inner.should_skip_refresh(Some(key_k)),
            "an identical skip key after a successful commit must skip"
        );

        // Tick 3: real edit → different key → full refresh.
        assert!(
            !inner.should_skip_refresh(Some(key_edit)),
            "a different skip key (real edit) must defeat the skip"
        );
    }

    /// Phase B / Correctness Req 1 — a FAILED refresh must not poison the
    /// skip key: the failure path never calls `commit_skip_key`, so the
    /// next byte-identical tick still rebuilds fully.
    ///
    /// This drives the exact seam `refresh_bundle_and_routes` uses:
    /// `should_skip_refresh` at the top, `commit_skip_key` only on the
    /// all-steps-succeeded path (a host-start error `?`-returns before
    /// reaching it).
    ///
    /// Falsifiability: if the production code committed the key right
    /// after bundling (before host start), the failed tick here would
    /// have stored `key_k` and the second `should_skip_refresh(key_k)`
    /// would return true — freezing the stale renderer in place.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn failed_refresh_does_not_poison_skip_key() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_k = [0xCCu8; 32];

        // Tick A: bundle computed key K, skip check says "no skip" (fresh
        // state) → full refresh attempted → host start FAILS. The error
        // propagates via `?` and commit_skip_key is never reached.
        assert!(!inner.should_skip_refresh(Some(key_k)));
        // (no commit — simulates the host-start failure)

        // Tick B: byte-identical bundle → same key K. MUST still rebuild
        // fully (no skip), because tick A never succeeded.
        assert!(
            !inner.should_skip_refresh(Some(key_k)),
            "a failed refresh must not poison the skip key — the next \
             byte-identical tick must rebuild fully"
        );

        // Tick B succeeds this time → commit. Now tick C may skip.
        inner.commit_skip_key(Some(key_k));
        assert!(
            inner.should_skip_refresh(Some(key_k)),
            "after the retry tick succeeds, the skip key is live again"
        );
    }

    /// Phase B (codex review fix) — a refresh that swapped the live host
    /// but then failed the route-table rebuild must invalidate the stored
    /// key at swap time. Otherwise a later tick that restores the OLD
    /// bundle (e.g. the user undoing the edit) would match the stale key
    /// and skip — freezing the failed tick's host in place.
    ///
    /// Drives the same seam sequence `refresh_bundle_and_routes` executes:
    /// `commit_skip_key(None)` immediately after the swap, final
    /// `commit_skip_key(Some(...))` never reached on the failure path.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn route_rebuild_failure_after_swap_invalidates_skip_key() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_old = [0x11u8; 32]; // tick N's bundle
        let key_new = [0x22u8; 32]; // tick N+1's edited bundle

        // Tick N: full refresh succeeds → commit old key.
        inner.commit_skip_key(Some(key_old));

        // Tick N+1: edit → new key, no skip → host start OK → SWAP →
        // invalidate-at-swap → route-table rebuild FAILS (final commit
        // never reached).
        assert!(!inner.should_skip_refresh(Some(key_new)));
        inner.commit_skip_key(None); // the swap-time invalidation
                                     // (route rebuild fails here — no final commit)

        // Tick N+2: user undoes the edit → old bundle bytes → old key.
        // MUST NOT skip: the live host is tick N+1's renderer, not the
        // one key_old describes.
        assert!(
            !inner.should_skip_refresh(Some(key_old)),
            "a post-swap failure must invalidate the stored key so a \
             restored old bundle cannot skip against the wrong live host"
        );
    }

    /// Phase B — an uncomputable key (`None`) never skips, and committing
    /// `None` (successful refresh whose bundle could not be hashed) CLEARS
    /// a previously-stored key so a later matching tick cannot skip
    /// against the wrong host state.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn uncomputable_key_never_skips_and_clears_stored_key() {
        let inner = stub_dev_inner(HashMap::new(), Vec::new());
        let key_k = [0xDDu8; 32];

        // None never skips, even against a stored key.
        inner.commit_skip_key(Some(key_k));
        assert!(
            !inner.should_skip_refresh(None),
            "an uncomputable key must force a full refresh"
        );

        // A successful refresh that could not hash its bundle clears the
        // stored key — the old key no longer describes the live renderer.
        inner.commit_skip_key(None);
        assert!(
            !inner.should_skip_refresh(Some(key_k)),
            "committing None must clear the stored key so a later tick \
             with the old key cannot skip against the wrong host state"
        );
    }

    // ── Static pages/**.html skip-key coverage (issue #956 gate (a)) ────────
    //
    // Static `.html` page routes bypass the JS bundle entirely (the
    // renderer copies the source body from disk at render time), so their
    // CONTENT is invisible to bundle bytes and to the route signature.
    // `compute_bundle_skip_key` must fold their bodies into the key.

    /// Build a `static_html` route pointing at `src`.
    #[cfg(feature = "embed_v8")]
    fn make_static_html_route(src: PathBuf) -> zfb_router::Route {
        zfb_router::Route {
            source_path: src,
            segments: vec![zfb_router::Segment::Static("about".into())],
            kind: zfb_router::RouteKind::Static,
            specificity: 100,
            output_extension: None,
            static_html: true,
        }
    }

    /// Editing a static `.html` page's bytes must change the skip key even
    /// when bundle bytes and the route signature are identical — otherwise
    /// the edit would be silently swallowed by the Phase-B skip.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_changes_on_static_html_edit() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"identical-bundle"));

        let html_path = dir.path().join("about.html");
        std::fs::write(&html_path, "<h1>v1</h1>").unwrap();
        let routes = vec![make_static_html_route(html_path.clone())];

        let k1 = compute_bundle_skip_key(&out, &routes);
        // Edit ONLY the static html body — same path, same route
        // signature, same bundle bytes.
        std::fs::write(&html_path, "<h1>v2</h1>").unwrap();
        let k2 = compute_bundle_skip_key(&out, &routes);

        assert!(k1.is_some());
        assert!(k2.is_some());
        assert_ne!(
            k1, k2,
            "a static pages/*.html content edit must defeat the skip \
             even when bundle bytes and route signature are unchanged"
        );
    }

    /// Unchanged static `.html` bodies keep the key stable — the skip
    /// still fires for genuine no-op ticks on projects with static pages.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_stable_when_static_html_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"identical-bundle"));

        let html_path = dir.path().join("about.html");
        std::fs::write(&html_path, "<h1>same</h1>").unwrap();
        let routes = vec![make_static_html_route(html_path)];

        let k1 = compute_bundle_skip_key(&out, &routes);
        let k2 = compute_bundle_skip_key(&out, &routes);
        assert!(k1.is_some());
        assert_eq!(k1, k2, "identical static html bodies → identical keys");
    }

    /// An unreadable static `.html` source forces `None` (full refresh) —
    /// the safe direction, mirroring the unreadable-bundle case.
    #[cfg(feature = "embed_v8")]
    #[test]
    fn bundle_skip_key_returns_none_for_unreadable_static_html() {
        let dir = tempfile::tempdir().unwrap();
        let out = make_bundler_out(write_temp_bundle(&dir, b"bundle"));

        let routes = vec![make_static_html_route(dir.path().join("missing.html"))];
        let key = compute_bundle_skip_key(&out, &routes);
        assert!(
            key.is_none(),
            "an unreadable static html source must force a full refresh"
        );
    }

    // ── URL→route reverse-lookup index tests (issue #1019) ───────────────────

    /// Build a minimal `DevRenderSession` with a single SSG route at
    /// `url_path` so lookup tests don't need to reach `stub_dev_inner`
    /// directly each time.
    fn session_with_route(url_path: &str) -> DevRenderSession {
        let entry = RouteUniverseEntry {
            url_path: url_path.into(),
            output_path: PathBuf::from("index.html"),
            route_key: url_path.into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            vec![DevRouteEntry {
                entry,
                params: None,
            }],
        );
        DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        }
    }

    /// Helper: assert `lookup_by_url` returns the expected entry `url_path`.
    fn assert_lookup(session: &DevRenderSession, request: &str, expected_url: &str) {
        let result = session.lookup_by_url(request);
        assert_eq!(
            result.as_ref().map(|e| e.url_path.as_str()),
            Some(expected_url),
            "lookup_by_url({request:?}) → expected {expected_url:?}, got {result:?}",
        );
    }

    /// Helper: assert `lookup_by_url` returns `None`.
    fn assert_no_lookup(session: &DevRenderSession, request: &str) {
        let result = session.lookup_by_url(request);
        assert!(
            result.is_none(),
            "lookup_by_url({request:?}) should be None but got {:?}",
            result.map(|e| e.url_path),
        );
    }

    /// Root `/` is reachable as `/` and `/index.html` (issue #1019).
    #[test]
    fn url_index_root_slash_and_index_html() {
        let session = session_with_route("/");
        assert_lookup(&session, "/", "/");
        assert_lookup(&session, "/index.html", "/");
    }

    /// `/posts/a` and `/posts/a/` resolve to the same entry (trailing-slash
    /// policy: both candidate forms are indexed).
    #[test]
    fn url_index_trailing_slash_equivalence() {
        let session = session_with_route("/posts/a");
        assert_lookup(&session, "/posts/a", "/posts/a");
        assert_lookup(&session, "/posts/a/", "/posts/a");
    }

    /// `/posts/a/index.html` resolves like `/posts/a/` (index.html duality).
    #[test]
    fn url_index_index_html_duality() {
        let session = session_with_route("/posts/a");
        assert_lookup(&session, "/posts/a/index.html", "/posts/a");
    }

    /// Query strings are stripped before lookup: `/posts/a/?x=1` → `/posts/a`.
    #[test]
    fn url_index_query_string_ignored() {
        let session = session_with_route("/posts/a");
        assert_lookup(&session, "/posts/a/?x=1", "/posts/a");
        assert_lookup(&session, "/posts/a?x=1&y=2", "/posts/a");
    }

    /// Percent-encoded paths are decoded before lookup:
    /// `/posts/caf%C3%A9` → decoded to `/posts/café` → resolves.
    #[test]
    fn url_index_percent_encoding_decoded() {
        let session = session_with_route("/posts/café");
        assert_lookup(&session, "/posts/caf%C3%A9", "/posts/café");
    }

    /// Non-HTML routes (e.g. `feed.xml`, `sitemap.xml`) are indexed verbatim
    /// because they have an explicit file extension — no slash/index variants.
    #[test]
    fn url_index_non_html_extension_routes() {
        let entry = RouteUniverseEntry {
            url_path: "/feed.xml".into(),
            output_path: PathBuf::from("feed.xml"),
            route_key: "/feed.xml".into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/feed.xml.tsx"),
            vec![DevRouteEntry {
                entry,
                params: None,
            }],
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        assert_lookup(&session, "/feed.xml", "/feed.xml");
        // Trailing-slash normalisation: `/feed.xml/` → stripped → same key.
        assert_lookup(&session, "/feed.xml/", "/feed.xml");
        // Extension routes must NOT generate an `index.html` sub-key.
        assert_no_lookup(&session, "/feed.xml/index.html");
    }

    /// SSR-only routes (`prerender = false`, stored in `ssr_routes`) are NOT
    /// present in the url_index — they are served by the SSR leg.
    #[test]
    fn url_index_excludes_ssr_only_routes() {
        let ssr_entry = RouteUniverseEntry {
            url_path: "/ssr-page".into(),
            output_path: PathBuf::new(),
            route_key: "/ssr-page".into(),
            static_html: false,
            source_path: None,
        };
        // SSR entries go into `ssr_routes`, NOT `routes_by_source`.
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(HashMap::new(), vec![ssr_entry])),
        };
        assert_no_lookup(&session, "/ssr-page");
        assert_no_lookup(&session, "/ssr-page/");
    }

    /// Unknown routes return `None`.
    #[test]
    fn url_index_unknown_route_returns_none() {
        let session = session_with_route("/posts/a");
        assert_no_lookup(&session, "/does-not-exist");
        assert_no_lookup(&session, "/posts/b");
    }

    /// Multiple SSG routes coexist in the index independently.
    #[test]
    fn url_index_multiple_routes() {
        let make_entry = |url: &str| RouteUniverseEntry {
            url_path: url.into(),
            output_path: PathBuf::from(format!("{}/index.html", url.trim_start_matches('/'))),
            route_key: url.into(),
            static_html: false,
            source_path: None,
        };
        let mut routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        routes.insert(
            PathBuf::from("pages/about.tsx"),
            vec![DevRouteEntry {
                entry: make_entry("/about"),
                params: None,
            }],
        );
        routes.insert(
            PathBuf::from("pages/blog/hello.tsx"),
            vec![DevRouteEntry {
                entry: make_entry("/blog/hello"),
                params: None,
            }],
        );
        let session = DevRenderSession {
            inner: Arc::new(stub_dev_inner(routes, Vec::new())),
        };
        assert_lookup(&session, "/about", "/about");
        assert_lookup(&session, "/about/", "/about");
        assert_lookup(&session, "/blog/hello", "/blog/hello");
        assert_lookup(&session, "/blog/hello/index.html", "/blog/hello");
    }

    /// `url_index_lookup_keys` normalisation edge cases for the root path.
    #[test]
    fn url_index_lookup_keys_root_variants() {
        // Empty string → root.
        let keys = url_index_lookup_keys("");
        assert!(keys.contains(&"/".to_string()));
        assert!(keys.contains(&"/index.html".to_string()));
        // All-slashes → root.
        let keys2 = url_index_lookup_keys("///");
        assert!(keys2.contains(&"/".to_string()));
    }

    /// Rebuild-swap coherence: after a table swap, lookups reflect the new
    /// tables and not the old ones (issue #1019).
    ///
    /// This test drives the swap seam directly — it constructs an inner with
    /// route A, then writes route B + a new url_index into the RwLock the same
    /// way `refresh_bundle_and_routes` does (without needing a live V8 host),
    /// and asserts the lookup after the swap returns B, not A.
    #[test]
    fn url_index_swap_coherence() {
        // Boot: one route at /old.
        let old_entry = RouteUniverseEntry {
            url_path: "/old".into(),
            output_path: PathBuf::from("old/index.html"),
            route_key: "/old".into(),
            static_html: false,
            source_path: None,
        };
        let mut old_routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        old_routes.insert(
            PathBuf::from("pages/old.tsx"),
            vec![DevRouteEntry {
                entry: old_entry,
                params: None,
            }],
        );
        let inner = Arc::new(stub_dev_inner(old_routes, Vec::new()));
        let session = DevRenderSession {
            inner: Arc::clone(&inner),
        };

        // Verify old state.
        assert_lookup(&session, "/old", "/old");
        assert_no_lookup(&session, "/new");

        // Simulate P4 swap: new route at /new, old route removed.
        let new_entry = RouteUniverseEntry {
            url_path: "/new".into(),
            output_path: PathBuf::from("new/index.html"),
            route_key: "/new".into(),
            static_html: false,
            source_path: None,
        };
        let mut new_routes: HashMap<PathBuf, Vec<DevRouteEntry>> = HashMap::new();
        new_routes.insert(
            PathBuf::from("pages/new.tsx"),
            vec![DevRouteEntry {
                entry: new_entry,
                params: None,
            }],
        );
        let new_url_index = build_url_index(&new_routes);
        {
            let mut tables = inner.routes.write().unwrap();
            tables.routes_by_source = new_routes;
            tables.ssr_routes = Vec::new();
            tables.url_index = new_url_index;
        }

        // After swap: old route gone, new route visible.
        assert_no_lookup(&session, "/old");
        assert_lookup(&session, "/new", "/new");
        assert_lookup(&session, "/new/", "/new");
        assert_lookup(&session, "/new/index.html", "/new");
    }
}
