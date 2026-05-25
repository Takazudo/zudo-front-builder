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

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use zfb_build::bundler::{bundle, BundleMode, BundlerInput, BundlerOutput};
use zfb_build::renderer::{
    render_one, shutdown, start, Backend, RendererStartInput, RendererState, RouteUniverseEntry,
};
use zfb_build::{
    BuildContext, BuildOrchestrator, BuildOutcome, CssRunner, DevAssetPipeline, IslandsBundleInfo,
    IslandsRunner, OrchestratorConfig, PageRenderer, RelDistPath, RenderedPage,
};
use zfb_graph::persist::{load_from_disk, save_to_disk, ManifestDigest};
use zfb_graph::{DependencyGraph, PageDeps, PageId};
use zfb_server::{
    outcome_to_events, serve, PageCache, ReloadEvent, ServeOpts, SsrDispatcher, SsrRouteRecord,
    SsrRouteSet,
};

use crate::cli::DevArgs;
use crate::commands::resolve::{resolve_port, resolve_under_root};
use crate::config;
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, cfg_framework_to_render, check_runtime_installed,
    embedded_node_modules,
};

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

    let host = resolve_host(args.host.as_deref(), cfg.host.as_deref());
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
                    return Err(e)
                        .map_err(zfb_build::annotate_with_plugin_error)
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
    let watch_roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
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
        initial_graph.unwrap_or_else(DependencyGraph::new),
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
    let orch_config = OrchestratorConfig::new(&project_root, watch_roots.clone())
        .with_extra_watch_paths(extra_watch_paths);
    let orchestrator = BuildOrchestrator::new(orch_config, graph, pipeline);

    let render_pages: PageRenderer = match dev_session.as_ref() {
        Some(session) => make_render_callback(session.clone(), dist_root.clone()),
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
        &islands_plugin_config,
    ) {
        Ok(Some(payload)) => {
            let url = prefixed_islands_url(payload.stable_url);
            if let Ok(mut guard) = islands_bundle_url_handle.write() {
                *guard = Some(url);
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
        let url_prefix = dev_islands_url_prefix.clone();
        let url_handle = Arc::clone(&islands_bundle_url_handle);
        Some(Arc::new(move || -> Result<Option<IslandsBundleInfo>> {
            let payload = crate::commands::build::build_default_islands_payload(
                &project_root,
                &dist_root_for_islands,
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
                return Ok(None);
            };
            let bundle_url = if url_prefix.is_empty() {
                payload.stable_url
            } else {
                format!("{url_prefix}{}", payload.stable_url)
            };
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
                    "initial CSS bundle: failed to write bytes to dist (no <link> until rebuild)"
                        .to_string(),
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

    let ctx = BuildContext {
        dist_root: dist_root.clone(),
        render_pages,
        run_css,
        run_islands,
        // The bundle-rebuild + renderer-reload wiring lands
        // here once the dev-mode bundler is available on a per-tick
        // basis; for now leave the hook empty so existing behaviour
        // is preserved (the renderer state stays bound to the
        // boot-time bundle).
        reload_renderer: None,
    };

    // 4. on_outcome — translate each tick into reload events.
    let tx_cb = tx.clone();
    let on_outcome = move |outcome: &BuildOutcome| {
        for ev in outcome_to_events(outcome) {
            let _ = tx_cb.send(ev);
        }
    };

    // 5. Spawn the orchestrator's watcher loop.
    let orch_handle = tokio::spawn(async move {
        if let Err(err) = orchestrator.run(ctx, on_outcome).await {
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
    // Issue #367 — build the SSR route set from the dev session's
    // `prerender = false` pages, backed by the embedded V8 host the
    // SSG pipeline already owns. None when no session (renderer
    // disabled) or when the project has zero SSR pages.
    let ssr_route_set = build_ssr_route_set(dev_session.as_ref());

    let opts = ServeOpts {
        project_root,
        dist_root,
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

    output::ready_with_interfaces("http", &host, port);

    // 7. Run the server until Ctrl+C. Pass Ctrl+C as the graceful-shutdown
    //    signal so axum drains in-flight connections before exiting. The
    //    renderer guard tears down on drop here — the explicit `shutdown`
    //    call belt-and-braces keeps the surface symmetrical (start ↔ shutdown).
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let result = tokio::select! {
        res = serve(opts, ctrl_c) => {
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
// Renderer plumbing
// ---------------------------------------------------------------------------

/// Long-lived dev-session state that owns the renderer subprocess and
/// the route table. Cloned by the [`PageRenderer`] callback so each
/// orchestrator tick can map page ids → URLs.
#[derive(Clone)]
struct DevRenderSession {
    inner: Arc<DevRenderInner>,
}

struct DevRenderInner {
    /// Mapped from the page module's project-relative source path
    /// (which is what the dependency graph keys on) to the renderer
    /// entry. Built once at boot from the router scan.
    ///
    /// Issue #367: only pages with `prerender != false` are kept
    /// here. Pages that opted out of SSG go into [`ssr_routes`]
    /// instead and reach the V8 host at request time.
    routes_by_source: HashMap<PathBuf, RouteUniverseEntry>,
    /// URL patterns for `prerender = false` pages (issue #367 /
    /// Gap 1). Empty when every page in the project SSGs. The dev
    /// server reads this list (via [`DevRenderSession::ssr_patterns`])
    /// and builds an [`zfb_server::SsrRouteSet`] from it.
    ssr_routes: Vec<RouteUniverseEntry>,
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
}

impl DevRenderSession {
    /// Drive a single page id against the renderer. Returns a
    /// [`RenderedPage`] populated with the bytes the renderer just
    /// wrote, so the dev pipeline's atomic-write + cache layer can
    /// fold the result through the existing reload broadcast.
    fn render_one(&self, page: &PageId, dist_dir: &Path) -> Result<Option<RenderedPage>> {
        let entry = match self.inner.routes_by_source.get(page.path()) {
            Some(e) => e.clone(),
            None => return Ok(None),
        };
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        let state = lock
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("renderer not started"))?;
        let written = render_one(state, &entry, dist_dir, &self.inner.project_root).map_err(anyhow::Error::from)?;
        let html = std::fs::read_to_string(&written)
            .with_context(|| format!("failed to read rendered page {}", written.display()))?;
        // RouteUniverseEntry::output_path is a PathBuf validated by the
        // router/render_pipeline (relative, no escapes). Wrap it in
        // RelDistPath for the pipeline's type contract. If the path is
        // somehow invalid, surface an error rather than silently skipping.
        let output_path = RelDistPath::new(entry.output_path)
            .with_context(|| format!("renderer returned invalid output_path for {:?}", page))?;
        Ok(Some(RenderedPage {
            page: page.clone(),
            output_path,
            html,
            content_type: None,
        }))
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
            .routes_by_source
            .keys()
            .map(|p| PageId::new(p.clone()))
            .collect()
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
    if plan.static_routes.is_empty() {
        return Err(anyhow::anyhow!(
            "no static routes to render — dev mode skips renderer boot"
        ));
    }

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

    // Issue #367 — extract `export const prerender = …` per page so
    // we can keep SSG-eligible pages in `routes_by_source` (the SSG
    // render callback's lookup table) while routing `prerender =
    // false` pages into the request-time SSR set instead. Without this
    // split the SSG callback would stamp a stale snapshot to disk on
    // every watcher tick and the dist fallback would shadow the SSR
    // handler.
    let prerender_map = build_prerender_map(router.routes(), project_root, |msg| {
        crate::output::warn(msg.to_string())
    });

    // Build the source-path → entry map once. Router source paths are
    // project-relative; PageId keys on the same value (the orchestrator
    // tracks pages by their source path).
    let mut routes_by_source: HashMap<PathBuf, RouteUniverseEntry> = HashMap::new();
    let mut ssr_routes: Vec<RouteUniverseEntry> = Vec::new();
    for route in router.routes() {
        if let Some(entry) = plan
            .static_routes
            .iter()
            .find(|e| e.route_key == route.template())
        {
            // Default to SSG when the prerender map has no entry
            // (matches `crates/zfb/src/commands/build.rs` and the
            // `prerender_map`'s documented default).
            let is_prerender = prerender_map
                .get(&route.template())
                .copied()
                .unwrap_or(true);
            if is_prerender {
                routes_by_source.insert(route.source_path.clone(), entry.clone());
            } else {
                ssr_routes.push(entry.clone());
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
        if prerender_map.get(&deferred.template).copied().unwrap_or(true) {
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

    Ok(DevRenderSession {
        inner: Arc::new(DevRenderInner {
            routes_by_source,
            ssr_routes,
            renderer: Arc::new(Mutex::new(Some(state))),
            project_root: project_root.to_path_buf(),
        }),
    })
}

/// Build the [`PageRenderer`] callback that the orchestrator hands to
/// [`DevAssetPipeline`].
fn make_render_callback(session: DevRenderSession, dist_dir: PathBuf) -> PageRenderer {
    Arc::new(move |pages: &[PageId]| {
        let mut out = Vec::with_capacity(pages.len());
        for page in pages {
            match session.render_one(page, &dist_dir) {
                Ok(Some(rendered)) => out.push(rendered),
                Ok(None) => {
                    // Page not in the renderer's route map (e.g.
                    // dynamic route deferred to the paths() follow-up,
                    // or a page that was never in the router scan).
                    // Intentionally a no-op so the watcher tick still
                    // succeeds — the orchestrator will report the page
                    // as not-rendered, the user sees the warning at
                    // boot, and other pages keep rebuilding.
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

/// Build the [`SsrRouteSet`] for the dev server from the dev session
/// (issue #367 / Gap 1).
///
/// Returns `None` when the dev session is absent (renderer disabled —
/// the SSR layer would have no V8 host to dispatch through) or when
/// every page in the project is SSG (no `prerender = false` routes).
/// Otherwise constructs an [`crate::ssr_adapter::EmbeddedV8SsrAdapter`]
/// over the same renderer mutex the SSG callback uses, so the V8 host
/// is shared across build-time and request-time dispatches.
///
/// Live-reload of SSR page sources: editing a `prerender = false` page
/// during a `zfb dev` session does NOT currently re-evaluate the bundle
/// inside the running V8 host — `BuildContext::reload_renderer` is
/// `None` in dev today. The next dev-server restart picks up the new
/// code. Wiring `reload_renderer` is a follow-up for a future sub-task.
///
/// Compiled in only when the `embed_v8` feature is on (issue #371,
/// sub-task 4.1a) — the SSR adapter requires the V8 host.
#[cfg(feature = "embed_v8")]
fn build_ssr_route_set(session: Option<&DevRenderSession>) -> Option<SsrRouteSet> {
    let session = session?;
    let patterns = session.ssr_patterns();
    if patterns.is_empty() {
        return None;
    }
    let renderer_handle = session.renderer_handle();
    let dispatcher: Arc<dyn SsrDispatcher> =
        Arc::new(crate::ssr_adapter::EmbeddedV8SsrAdapter::new(
            renderer_handle,
        ));
    let records = patterns
        .into_iter()
        .map(|pattern| SsrRouteRecord { pattern })
        .collect();
    Some(SsrRouteSet::new(records, dispatcher))
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

fn resolve_host(cli: Option<&str>, cfg: Option<&str>) -> String {
    cli.or(cfg).unwrap_or(DEFAULT_DEV_HOST).to_owned()
}

fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let pair = format!("{host}:{port}");
    let mut iter = pair
        .to_socket_addrs()
        .with_context(|| format!("could not resolve bind address {pair}"))?;
    iter.next()
        .ok_or_else(|| anyhow::anyhow!("no socket addresses resolved for {pair}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_host_prefers_cli_over_config() {
        assert_eq!(resolve_host(Some("0.0.0.0"), Some("127.0.0.1")), "0.0.0.0");
    }

    #[test]
    fn resolve_host_falls_back_to_config_when_cli_absent() {
        assert_eq!(resolve_host(None, Some("127.0.0.1")), "127.0.0.1");
    }

    #[test]
    fn resolve_host_falls_back_to_builtin_when_neither_supplied() {
        assert_eq!(resolve_host(None, None), DEFAULT_DEV_HOST);
    }

    #[test]
    fn default_watch_roots_includes_zfb_config_json() {
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.json"));
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.ts"));
    }

    /// The render callback must:
    /// 1. Be tolerant of unknown page ids (dynamic routes, etc.) —
    ///    return an empty list, never error.
    /// 2. Hand back a `RenderedPage` for every page id mapped to a
    ///    [`RouteUniverseEntry`].
    ///
    /// We exercise both via a [`DevRenderSession`] that uses a `None`
    /// renderer; an unknown page id therefore always lands on the
    /// `Ok(None)` branch and never reaches the lock.
    #[test]
    fn render_callback_drops_unknown_pages_silently() {
        let session = DevRenderSession {
            inner: Arc::new(DevRenderInner {
                routes_by_source: HashMap::new(),
                ssr_routes: Vec::new(),
                renderer: Arc::new(Mutex::new(None)),
                project_root: PathBuf::new(),
            }),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages).unwrap();
        assert!(out.is_empty());
    }

    /// On a known page id, but a `None` renderer (boot half-failed),
    /// the callback logs an error and returns an empty list — the
    /// watcher must keep going.
    #[test]
    fn render_callback_keeps_watcher_alive_on_render_error() {
        let mut routes = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
                static_html: false,
                source_path: None,
            },
        );
        let session = DevRenderSession {
            inner: Arc::new(DevRenderInner {
                routes_by_source: routes,
                ssr_routes: Vec::new(),
                renderer: Arc::new(Mutex::new(None)),
                project_root: PathBuf::new(),
            }),
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
            inner: Arc::new(DevRenderInner {
                routes_by_source: HashMap::new(),
                ssr_routes: vec![
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
                renderer: Arc::new(Mutex::new(None)),
                project_root: PathBuf::new(),
            }),
        };
        let patterns = session.ssr_patterns();
        assert_eq!(patterns, vec!["/blog/[slug]".to_string(), "/api/x".to_string()]);
    }
}
