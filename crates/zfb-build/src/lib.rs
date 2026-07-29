//! `zfb-build` — the dev-loop orchestrator for the zudo-front-builder
//! framework.
//!
//! Wires together the four moving parts of the dev pipeline:
//!
//! 1. [`zfb_watcher`] for filesystem change events.
//! 2. [`zfb_graph`] for "given this file changed, which pages need to be
//!    re-rendered?" queries.
//! 3. The page renderer (Epic 3 — `zfb-render` / `zfb-content` /
//!    `zfb-router`).
//! 4. The CSS pipeline (Epic 4 — `zfb-css`) and the islands bundler
//!    (Epic 5 — `zfb-islands`).
//!
//! The orchestrator does *not* call those last three crates directly. It
//! goes through pluggable callback functions ([`PageRenderer`],
//! [`CssRunner`], [`IslandsRunner`]) so:
//!
//! - the orchestrator's surface stays free of heavyweight transitive
//!   dependencies (SWC's TSX→JS pipeline in `zfb-render`, and the
//!   esbuild npm subprocess wrappers in `zfb-css` /
//!   `zfb-islands`), and
//! - tests can plug in fakes that count invocations rather than spinning
//!   up Tailwind/esbuild subprocesses.
//!
//! The whole thing is exposed via the [`AssetPipeline`] trait + the
//! default [`DevAssetPipeline`] impl. Production / SSR / edge builds can
//! ship their own `AssetPipeline` implementation later without rewriting
//! the orchestrator.
//!
//! ## Granularity policy
//!
//! See [`policy`] for the full rules and rationale. In short:
//!
//! - Edits to the registered "global" set (e.g. `zfb.config.ts`) → full
//!   rebuild (every page, CSS, islands).
//! - Edits to a `content/**` source → only the consumer pages re-render.
//! - Edits to a `styles/**` file → CSS pipeline rerun, no page re-render.
//! - Edits to a `"use client"` component → islands re-bundle, no CSS
//!   rerun unless its CSS also changed.
//! - Edits to a layout/component used by N pages → those N pages
//!   re-render (looked up via the dependency graph).
//!
//! ## Atomic dist write
//!
//! Every file the pipeline emits is written to `<final>.tmp-<rand>` in the
//! same directory and then `rename`d into place. `rename` is atomic on the
//! same filesystem, so a reader (the dev preview server, a fs notify
//! consumer, …) never observes a half-written file. The pipeline ships
//! [`atomic_write`] for crates that need the same guarantee.

pub mod adapter;
pub mod atomic;
pub mod bundler;
pub mod content_provenance;
pub mod glob_expand;
pub mod head_inject;
pub mod link_base_rewrite;
pub mod metafile_deps;
pub mod module_worker;
pub mod orchestrator;
pub mod pipeline;
pub mod plan;
pub mod plugin_refresh;
pub mod plugin_registries;
pub mod plugin_runner;
pub mod policy;
pub mod raw_import_expand;
pub mod renderer;

pub use adapter::{
    ensure_no_ssr_without_adapter, run_adapter_bundle, run_adapter_bundle_with, AdapterBundleInput,
    AdapterBundleOutput, AdapterChoice, AdapterRunner, DefaultAdapterRunner, SsrRouteRef,
};
pub use atomic::{atomic_write, atomic_write_string, validate_output_path};
pub use bundler::{
    bundle, bundle_with_session, resolve_esbuild_binary_with_env, BundleManifest, BundleMode,
    BundlerInput, BundlerOutput, ContentCollectionSpec, OnBrokenLinks, ResolveMarkdownLinksRoute,
    ResolveMarkdownLinksSpec, RouteEntry, ShadowSession, SiblingMirrorPlan, DEFAULT_ESBUILD_SLOT,
};
pub use content_provenance::{
    ContentCollectionId, ContentCollectionMembership, ContentEdgeGroup, ContentProvenance,
    ContentProvenanceError, TrackedContentRead,
};
pub use head_inject::{
    css_link_tag, inject_prod_head_assets, island_module_script_tag, needs_html5_doctype,
    ProdHeadAssets, HTML5_DOCTYPE_PREFIX,
};
pub use metafile_deps::{
    declared_first_party_package_for_source, route_module_deps, AcceptedPackage, RouteEntryRef,
    RouteModuleDeps,
};
pub use module_worker::{
    discover_module_preprocessing_with_context,
    discover_registered_virtual_preprocessing_with_context,
    remap_virtual_module_project_imports_to_shadow,
    remap_virtual_module_workspace_sibling_imports_to_shadow, rewrite_module_worker_urls,
    rewrite_module_worker_urls_with_context, shadow_mirror_prunes_path,
    ModulePreprocessingDiscovery, ModuleWorkerBuildContext, ModuleWorkerDependency,
    ModuleWorkerEdge, ModuleWorkerRawImportEdge, ModuleWorkerRewrite,
};
pub use orchestrator::{
    BuildOrchestrator, DiscoveryHook, DiscoveryOutcome, ExternalInvalidationHook,
    OrchestratorConfig, PreTickRefreshFuture, PreTickRefreshHook,
};
pub use pipeline::{
    apply_prod_asset_pipeline, synthesize_page_id_from_output, validate_companion_file_set,
    AssetEmitter, AssetEmitterPayload, AssetKind, AssetPipeline, BuildContext, BuildMode,
    BuildOutcome, ClientScriptsRunner, CssRunner, DevAssetPipeline, DevBuildContext,
    DynamicInjectedProbe, EmittedAsset, IslandsBundleInfo, IslandsRunner, PageRenderer,
    ProdAssetEmitterInputs, ProdBuildContext, ProdRenderedFile, ProductionAssetPipeline,
    ProductionEmitters, RefreshOutcome, RelDistPath, RenderedPage, RendererReloader,
    SsrPublishProbe, StaleProbe,
};
pub use plan::{ContentNarrowing, PageSelection, RebuildPlan};
pub use plugin_refresh::{
    PluginRefreshOutcome, PluginRefreshState, PluginVirtualModuleStore, PluginWatchOwnership,
};
pub use plugin_registries::{
    run_preview_setup, AliasEntry, AliasMap, ClientEntry, ClientEntryList, InjectedRoute,
    InjectedRouteList, SetupCommand, SetupRegistries, SetupRegistryError, VirtualLoaderId,
    VirtualModuleEntry, VirtualModuleRegistry,
};
pub use plugin_runner::{
    annotate_with_plugin_error, extract_plugin_error, resolve_hook_timeout, BuildHookContext,
    DevRegisterContext, DevRegistration, DevRequest, DevResponse, PluginError, PluginHost,
    PluginSpec, PostBuildParamValue, PostBuildRouteEntry, PostBuildRouteManifest, SetupHookContext,
};
pub use policy::{
    classify_change, classify_change_with_content_roots, GranularityPolicy, KnownContentEntries,
    PathClass, RawImportInvalidation,
};
pub use renderer::{
    reload, render_all, render_one, shutdown, start, Backend, EmbeddedV8Host,
    EmbeddedV8HostFactory, HttpResponseLike, RendererError, RendererInput, RendererOutput,
    RendererStartInput, RendererState, RouteUniverseEntry, SsrManifest, SsrRouteEntry,
};
