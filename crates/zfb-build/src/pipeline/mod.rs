//! [`AssetPipeline`] trait + the dev / production sibling impls.
//!
//! The [`AssetPipeline`] trait is the contract: given a [`crate::RebuildPlan`]
//! and a [`BuildContext`], execute the plan and return a [`BuildOutcome`].
//! Two implementations live in submodules:
//!
//! - [`dev::DevAssetPipeline`] (sub-module [`dev`]) — the watcher-driven
//!   incremental pipeline `zfb dev` runs.
//! - [`prod::ProductionAssetPipeline`] (sub-module [`prod`]) — the
//!   one-shot full build `zfb build` runs. Hashed asset filenames,
//!   content-addressed URLs in HTML, no SSE wiring.
//!
//! ## Why a trait when there are two impls (and possibly more)?
//!
//! The orchestrator has a fairly opinionated lifecycle: receive a plan,
//! call into renderer/CSS/islands in some order, atomically write
//! everything to `dist/`. That lifecycle is the same shape across dev,
//! production, and edge — but the details differ:
//!
//! - **Dev**: don't minify, watch the graph, error on first failure but
//!   keep the watcher alive. Stable filenames so the dev server's URL
//!   contract is unchanged across rebuilds. Reload signals over SSE.
//! - **Production**: minify, fail-fast on first error, **content-hashed**
//!   asset filenames + HTML rewrite so deployed assets can be cached
//!   forever. No SSE — `dist/` is a static tree the user uploads to a
//!   CDN.
//! - **SSR / edge**: skip writing HTML to disk; emit it into a deno-
//!   shaped runtime bundle.
//!
//! Locking the orchestrator to a concrete struct now would force a
//! refactor when production-build lands. Locking to a trait costs a
//! single virtual call per rebuild tick and keeps the door open.
//!
//! ## Why callbacks for renderer / css / islands?
//!
//! The orchestrator deliberately doesn't depend on `zfb-render`,
//! `zfb-css`, or `zfb-islands` directly:
//!
//! - `zfb-render` requires the `deno_core_host` feature (gigabytes of V8
//!   build artefacts) for a working host. The orchestrator must compile
//!   without that feature flag flipped on.
//! - The CSS / islands crates ship trait-based plug points
//!   (`CssEngine`, `ClientBundler`) plus subprocess wrappers around
//!   third-party CLIs (Tailwind, esbuild). Pulling them in transitively
//!   would force every consumer of `zfb-build` to pay that cost.
//! - Tests need fakes that count invocations without spawning binaries.
//!
//! So the public API takes function-typed inputs. The bin crate
//! (Epic 7's `zfb dev` / `zfb build` commands) instantiates concrete
//! renderers / engines and passes closures here.
//!
//! ## Selecting dev vs production
//!
//! The selection is intentionally a **call-site choice**, not an
//! orchestrator-internal flag. The bin crate constructs either
//! [`dev::DevAssetPipeline`] or [`prod::ProductionAssetPipeline`] and
//! hands it to [`crate::BuildOrchestrator`]. The trait dispatch keeps
//! the orchestrator free of `if mode == Production { … }` conditionals.
//!
//! [`BuildMode`] is provided as a convenience for callers (and for
//! tests) that want to thread mode through configuration without binding
//! to a specific impl, but the orchestrator itself never inspects it.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use zfb_graph::PageId;

use crate::plan::RebuildPlan;

pub mod dev;
pub mod prod;

pub use dev::DevAssetPipeline;
pub use prod::{
    AssetEmitter, AssetKind, EmittedAsset, ProductionAssetPipeline, ProductionEmitters,
};

/// Which mode the call site wants the asset pipeline configured for.
///
/// The orchestrator does **not** read this — selection is by trait
/// dispatch (the bin crate constructs the right impl). It exists so
/// configuration code can carry "are we building or watching?" without
/// pulling either pipeline impl into scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildMode {
    /// `zfb dev` — incremental, stable filenames, watcher-driven.
    Dev,
    /// `zfb build` — one-shot, content-hashed assets, no SSE.
    Production,
}

impl BuildMode {
    /// True iff this is the production mode.
    pub fn is_prod(self) -> bool {
        matches!(self, BuildMode::Production)
    }
}

/// One rendered page's output.
///
/// The pipeline writes `html` to `<dist_root>/<output_path>` atomically.
/// `output_path` is relative to the dist root and must be a safe
/// subpath (no `..`).
///
/// ## Output extension precedence (Sub 49)
///
/// `output_path` is the load-bearing carrier of the page's output
/// extension — the pipeline does not re-derive it. The producer
/// (typically the renderer in `zfb-render`) is expected to apply the
/// precedence rule before constructing this struct:
///
/// 1. Frontmatter `extension` override (`export const extension = "rss"`),
/// 2. Filename convention (`pages/sitemap.xml.tsx` → `xml`,
///    `api.v2.json.tsx` → `json`),
/// 3. Default `.html`.
///
/// See `zfb_router::route::Route::output_filename` and
/// `zfb_render::meta::derive_output_extension` for the canonical
/// helpers. ADR-003 (Sub 7) documents the same rule for users.
///
/// ## Stale-output cleanup
///
/// [`DevAssetPipeline`] tracks the last-known `output_path` per
/// [`PageId`] and deletes the previous artifact when this field
/// changes (e.g. a page whose frontmatter flipped `extension` from
/// `xml` to `rss` won't leave an orphan `dist/sitemap.xml` behind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    /// The page id this output belongs to. Echoed back verbatim from
    /// the [`crate::PageSelection`] so callers can correlate inputs/outputs.
    pub page: PageId,

    /// Path under the dist root, in URL-style forward-slash form. The
    /// pipeline joins this onto its `dist_root` and writes the bytes
    /// atomically.
    pub output_path: PathBuf,

    /// Page body (HTML, XML, JSON, plain text — whatever the
    /// `output_path` extension implies). The renderer is responsible
    /// for serialising the value into a string before constructing
    /// this struct.
    pub html: String,

    /// Optional `Content-Type` to associate with this page. The build
    /// layer treats it as metadata only (static-file hosts derive
    /// the content type from the file extension); the dev server
    /// (`zfb-server`) reads it back from the page cache to set the
    /// HTTP response header.
    ///
    /// `None` means "let the consumer derive a default from the
    /// extension". See `zfb_render::meta::derive_content_type` for
    /// the canonical extension-to-content-type table.
    pub content_type: Option<String>,
}

/// Function that renders a batch of pages to HTML.
///
/// Boxed-trait-object alias: each rebuild tick may select N pages, and
/// the renderer can decide to render them serially (cheap, dev) or in
/// parallel (production). Errors abort the tick — the watcher stays
/// alive but the rebuild is reported as failed.
pub type PageRenderer =
    Arc<dyn Fn(&[PageId]) -> Result<Vec<RenderedPage>> + Send + Sync + 'static>;

/// Function that runs the CSS pipeline once and returns whether the
/// emitted asset is new (i.e. whether the asset URL changed).
///
/// `true` here triggers a re-render of any page that embeds the CSS asset
/// URL — but the orchestrator does *not* automatically schedule that
/// re-render in this version. Production builds that need URL stability
/// in HTML manage that explicitly through
/// [`prod::ProductionAssetPipeline`]'s [`AssetEmitter`] mechanism instead
/// of this opaque-bool runner.
pub type CssRunner = Arc<dyn Fn() -> Result<bool> + Send + Sync + 'static>;

/// Function that runs the islands bundler once and returns whether the
/// emitted asset is new. Same semantics as [`CssRunner`].
pub type IslandsRunner = Arc<dyn Fn() -> Result<bool> + Send + Sync + 'static>;

/// Per-build-tick context handed to [`AssetPipeline::apply`].
///
/// Holds the absolute `dist_root` (where output HTML and assets land)
/// plus the closures the dev pipeline calls to render pages, run CSS,
/// and bundle islands.
///
/// [`prod::ProductionAssetPipeline`] does **not** consume `run_css` or
/// `run_islands` from here — it owns its own [`AssetEmitter`] set passed
/// at construction time so it can read the asset bytes directly and
/// emit a content-hashed filename. The CSS / islands fields here remain
/// for the dev path that does not need to know about asset bytes.
#[derive(Clone)]
pub struct BuildContext {
    /// Absolute path to the dist directory. The pipeline writes
    /// `<dist_root>/<rendered_page.output_path>` atomically.
    pub dist_root: PathBuf,

    /// Page renderer callback.
    pub render_pages: PageRenderer,

    /// CSS pipeline callback. Optional: if `None`, CSS reruns are
    /// silently skipped. Used by tests that don't care about CSS.
    /// Consumed by [`DevAssetPipeline`] only.
    pub run_css: Option<CssRunner>,

    /// Islands bundler callback. Optional: if `None`, islands reruns are
    /// silently skipped. Consumed by [`DevAssetPipeline`] only.
    pub run_islands: Option<IslandsRunner>,
}

impl std::fmt::Debug for BuildContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildContext")
            .field("dist_root", &self.dist_root)
            .field("render_pages", &"<callback>")
            .field("run_css", &self.run_css.as_ref().map(|_| "<callback>"))
            .field(
                "run_islands",
                &self.run_islands.as_ref().map(|_| "<callback>"),
            )
            .finish()
    }
}

/// What an [`AssetPipeline::apply`] call did for the tick.
///
/// Counters mostly — handy for tests and for dev-server status logging
/// (`rendered N pages, CSS rerun, islands rerun`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildOutcome {
    /// Number of pages re-rendered this tick.
    pub pages_rendered: usize,

    /// Whether the CSS pipeline ran.
    pub css_rerun: bool,

    /// Whether the CSS pipeline reported a new asset (only meaningful
    /// when `css_rerun` is true).
    pub css_changed: bool,

    /// Whether the islands bundler ran.
    pub islands_rerun: bool,

    /// Whether the islands bundler reported a new asset (only
    /// meaningful when `islands_rerun` is true).
    pub islands_changed: bool,

    /// Pages whose HTML was actually written (the file was new or the
    /// bytes changed). Useful for the dev preview server's WebSocket
    /// reload path.
    pub pages_written: Vec<PageId>,

    /// Absolute paths that were pruned because the page now writes
    /// to a different `output_path` than the previous build (e.g. a
    /// frontmatter `extension` change flipped `dist/sitemap.xml` to
    /// `dist/sitemap.rss`). Useful for surfacing the cleanup to the
    /// dev server's reload logic and for tests.
    pub pages_pruned: Vec<PathBuf>,

    /// Hashed asset URLs emitted this tick, keyed by [`AssetKind`].
    ///
    /// Populated by [`prod::ProductionAssetPipeline`] (one entry per
    /// emitter that produced bytes) and left empty by
    /// [`DevAssetPipeline`]. Lets the bin crate log the URLs it just
    /// shipped without re-reading dist.
    pub hashed_asset_urls: Vec<(AssetKind, String)>,
}

/// The contract every asset pipeline implementation must satisfy.
///
/// `apply` is called once per rebuild tick, after the orchestrator has
/// folded watcher events through the granularity policy and dependency
/// graph into a [`RebuildPlan`].
///
/// Implementations should:
///
/// - Run only the sub-pipelines the plan requests.
/// - Fail fast on the first error inside a sub-pipeline (caller decides
///   whether to keep the watcher alive).
/// - Write outputs atomically (use [`crate::atomic_write`] or roll your
///   own equivalent).
pub trait AssetPipeline: Send + Sync {
    /// Apply `plan` against `ctx`. See module-level docs for the
    /// expected behaviour.
    fn apply(&self, plan: &RebuildPlan, ctx: &BuildContext) -> Result<BuildOutcome>;
}
