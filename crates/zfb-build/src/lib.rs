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
//!   miniflare/esbuild npm subprocess wrappers in `zfb-css` /
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
pub mod head_inject;
pub mod orchestrator;
pub mod pipeline;
pub mod plan;
pub mod plugin_runner;
pub mod policy;
pub mod renderer;

pub use adapter::{
    ensure_no_ssr_without_adapter, run_adapter_bundle, run_adapter_bundle_with, AdapterBundleInput,
    AdapterBundleOutput, AdapterChoice, AdapterRunner, DefaultAdapterRunner, SsrRouteRef,
};
pub use atomic::{atomic_write, atomic_write_string, validate_output_path};
pub use bundler::{
    bundle, BundleManifest, BundleMode, BundlerInput, BundlerOutput, ContentCollectionSpec,
    RouteEntry,
};
pub use head_inject::{
    css_link_tag, inject_prod_head_assets, island_module_script_tag, ProdHeadAssets,
};
pub use orchestrator::{BuildOrchestrator, OrchestratorConfig};
pub use plugin_runner::{
    annotate_with_plugin_error, extract_plugin_error, BuildHookContext, DevRegisterContext,
    DevRegistration, DevRequest, DevResponse, PluginError, PluginHost, PluginSpec,
};
pub use pipeline::{
    apply_prod_asset_pipeline, synthesize_page_id_from_output, AssetEmitter, AssetEmitterPayload,
    AssetKind, AssetPipeline, BuildContext, BuildMode, BuildOutcome, CssRunner, DevAssetPipeline,
    DevBuildContext, EmittedAsset, IslandsBundleInfo, IslandsRunner, PageRenderer,
    ProdAssetEmitterInputs, ProdBuildContext, ProdRenderedFile, ProductionAssetPipeline,
    ProductionEmitters, RelDistPath, RenderedPage, RendererReloader,
};
pub use plan::{PageSelection, RebuildPlan};
pub use policy::{classify_change, GranularityPolicy, PathClass};
pub use renderer::{
    render_all, render_one, shutdown, start, Backend, RendererError, RendererInput,
    RendererOutput, RendererStartInput, RendererState, RouteUniverseEntry, SsrManifest,
    SsrRouteEntry,
};
