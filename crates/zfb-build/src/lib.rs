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
//! - the orchestrator's surface stays free of feature-gated dependencies
//!   (notably `zfb-render`'s `deno_core_host` feature), and
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

pub mod atomic;
pub mod bundler;
pub mod orchestrator;
pub mod pipeline;
pub mod plan;
pub mod policy;
pub mod renderer;

pub use atomic::{atomic_write, atomic_write_string};
pub use bundler::{
    bundle, BundleManifest, BundleMode, BundlerInput, BundlerOutput, RouteEntry,
};
pub use orchestrator::{BuildOrchestrator, OrchestratorConfig};
pub use pipeline::{
    AssetPipeline, BuildContext, BuildOutcome, CssRunner, DevAssetPipeline, IslandsBundleInfo,
    IslandsRunner, PageRenderer, RenderedPage, RendererReloader,
};
pub use plan::{PageSelection, RebuildPlan};
pub use policy::{classify_change, GranularityPolicy, PathClass};
pub use renderer::{
    render_all, render_one, shutdown, start, Backend, RendererError, RendererInput,
    RendererOutput, RendererStartInput, RendererState, RouteUniverseEntry, SsrManifest,
    SsrRouteEntry,
};
