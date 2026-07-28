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
//! - **SSR / edge**: skip writing HTML to disk; emit it into a
//!   workerd-shaped runtime bundle.
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
//! - `zfb-render` carries the SWC TSX→JS pipeline and the
//!   embedded V8 render host. Keeping that out of the
//!   orchestrator's surface lets `zfb-build` compile cheaply for tests
//!   and for callers that only need orchestration types.
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

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use zfb_graph::PageId;

use crate::plan::RebuildPlan;

pub mod dev;
pub mod orchestrator;
pub mod prod;

pub use dev::{DevAssetPipeline, GuardedWriteOutcome, RequestWriteOutcome, RequestWriter};
pub use orchestrator::{
    apply_prod_asset_pipeline, synthesize_page_id_from_output, AssetEmitterPayload,
    ProdAssetEmitterInputs, ProdRenderedFile,
};
pub use prod::{
    validate_companion_file_set, AssetEmitter, AssetKind, CompanionFile, EmittedAsset,
    ProductionAssetPipeline, ProductionEmitters,
};

/// A validated relative path under the dist root.
///
/// Constructed via [`RelDistPath::new`], which rejects:
///
/// - absolute paths,
/// - Windows path prefixes,
/// - `..` components that would escape the dist root,
///
/// and normalises forward slashes on all platforms. Consumers that hold a
/// `RelDistPath` can trust the path is safe to join onto any `dist_root`
/// without running the full [`crate::atomic::validate_output_path`] check.
///
/// The inner `PathBuf` is stored in OS-native separator form but all
/// validation is performed on the component list, so the type is correct on
/// both Unix and Windows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelDistPath(PathBuf);

impl RelDistPath {
    /// Construct a `RelDistPath`, returning an error when:
    ///
    /// - `path` has an absolute root or Windows prefix,
    /// - any `..` component would escape the dist root (i.e. `..` when
    ///   there is no preceding normal component to consume).
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut depth: usize = 0;
        for c in path.components() {
            match c {
                Component::Prefix(_) | Component::RootDir => {
                    return Err(anyhow!(
                        "RelDistPath: path {:?} must be relative (got absolute root or prefix)",
                        path,
                    ));
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if depth == 0 {
                        return Err(anyhow!(
                            "RelDistPath: path {:?} would escape dist root via `..`",
                            path,
                        ));
                    }
                    depth -= 1;
                }
                Component::Normal(_) => {
                    depth += 1;
                }
            }
        }
        Ok(Self(path))
    }

    /// Borrow the inner path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Consume self and return the inner `PathBuf`.
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl std::fmt::Display for RelDistPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl AsRef<Path> for RelDistPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

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
/// ## Output extension precedence
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
/// See `zfb_router::route::Route::output_filename` for the precedence
/// implementation.
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

    /// Validated relative path under the dist root. The pipeline joins
    /// this onto its `dist_root` and writes the bytes atomically.
    /// Constructed via [`RelDistPath::new`], which guarantees the path
    /// is relative and cannot escape the dist root.
    pub output_path: RelDistPath,

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
    /// extension". The dev server uses
    /// `zfb_server::routes::content_type_for_extension` for that lookup.
    pub content_type: Option<String>,
}

/// Function that renders a batch of pages to HTML.
///
/// Boxed-trait-object alias: each rebuild tick may select N pages, and
/// the renderer can decide to render them serially (cheap, dev) or in
/// parallel (production). Errors abort the tick — the watcher stays
/// alive but the rebuild is reported as failed.
///
/// The second argument is the tick's optional content-narrowing hint
/// (issue #958, see [`crate::ContentNarrowing`]): when `Some`, the
/// renderer MAY narrow each dynamic source's route fan-out to the routes
/// derived from the changed entries — but narrowing only ever removes
/// routes from the selected render set, never adds. `None` means "render
/// every selected page fully" (today's behaviour); implementations that
/// don't narrow simply ignore the argument. The production pipeline
/// always passes `None`.
pub type PageRenderer = Arc<
    dyn Fn(&[PageId], Option<&crate::ContentNarrowing>) -> Result<Vec<RenderedPage>>
        + Send
        + Sync
        + 'static,
>;

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

/// Information about a freshly-emitted islands bundle.
///
/// Populated by an [`IslandsRunner`] when a re-bundle was attempted and
/// surfaces the per-component identifiers + the bundle's public URL so
/// the dev-server SSE layer can fan out one
/// `ReloadEvent::Islands { component, bundle_url }` per island. The
/// SSE layer never reaches into `zfb-islands` directly — it consumes
/// this side-channel through `BuildOutcome::islands_bundle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandsBundleInfo {
    /// `true` if the re-bundle produced a new asset URL (the input
    /// islands set or any of their bytes changed). When `false` the
    /// SSE layer still sees the info but emits no events.
    pub changed: bool,
    /// Public URL of the freshly-emitted bundle, e.g.
    /// `/assets/islands-abc12345.js`. Producers should use
    /// `zfb_islands::bundle_link_href` (or its production-pipeline
    /// equivalent) to derive this from the asset path so the URL the
    /// browser hits matches the URL the renderer embeds in HTML.
    pub bundle_url: String,
    /// Per-component identifiers (mirrors
    /// `zfb_islands::Island::component_name`). Order is the bundler's
    /// stable order so the dev-mode reload stream is deterministic
    /// across runs for a given input.
    pub components: Vec<String>,
}

/// Function that runs the islands bundler once and returns the
/// per-bundle metadata, or `None` when the runner ran but produced no
/// bundle (e.g. there are no `"use client"` components today).
///
/// Returning `IslandsBundleInfo { changed: false, .. }` is the right
/// shape when the bundler ran but the output was byte-identical to the
/// previous run; the orchestrator records the rerun in
/// [`BuildOutcome::islands_rerun`] but emits no SSE event.
pub type IslandsRunner = Arc<dyn Fn() -> Result<Option<IslandsBundleInfo>> + Send + Sync + 'static>;

/// Function that re-bundles all `*.client.{ts,tsx,js,jsx}` entries and
/// writes the stable per-entry files under `dist/assets/client/`.
///
/// Returns `true` when at least one client-script file was written (new
/// bytes or new entry), `false` when everything was byte-identical. The
/// dev pipeline emits a `ReloadEvent::Page` when this returns `true`.
///
/// The runner is responsible for pruning stale files (removed or renamed
/// entries) as part of the same call. Returning `Ok(false)` on a
/// no-change tick avoids a spurious full page reload.
pub type ClientScriptsRunner = Arc<dyn Fn() -> Result<bool> + Send + Sync + 'static>;

/// Outcome of one [`RendererReloader`] invocation (issue #956).
///
/// An explicit three-state result — deliberately not a bare
/// `bool + Vec` — so "skipped", "refreshed, nothing vanished", and
/// "refreshed with vanished routes" stay unambiguous at every call
/// site (`renderer_fresh` handling, discovery refreshes, and
/// plan-carried prune paths all read differently against these
/// states).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The reloader determined nothing observable changed (e.g. the
    /// Phase-B byte-identical-bundle check, issue #940) and left the
    /// live renderer and route tables untouched. The dev pipeline
    /// bypasses the render fan-out for the tick: re-rendering against
    /// an identical bundle would re-emit identical HTML — the same
    /// determinism assumption [`dev::DevAssetPipeline`]'s byte-dedup
    /// cache already makes.
    Skipped,
    /// The renderer bundle was rebuilt and the route tables refreshed.
    /// `vanished` carries the **absolute** dist paths whose output
    /// routes vanished globally after the route-table rebuild — i.e.
    /// paths that existed before the refresh but are absent from every
    /// source's new entry set. The dev pipeline prunes those files from
    /// disk and evicts them from the in-memory page cache. An empty
    /// `vanished` means refreshed-no-vanish.
    Refreshed {
        vanished: Vec<std::path::PathBuf>,
        /// Source [`PageId`]s whose route-entry set changed in this
        /// tick's route-table rebuild (issue #958). Non-empty means the
        /// route structure moved — a body edit CAN change `paths()`
        /// output — so the dev pipeline must disable content narrowing
        /// for the tick (fallback G5): narrowing against a moved route
        /// table could orphan a brand-new URL.
        changed_sources: Vec<PageId>,
    },
}

/// Function the dev pipeline calls after the render fan-out to collect
/// the routes the renderer marked STALE this tick instead of rendering
/// (issue #1025 — lazy dev render).
///
/// Returns the **relative output paths** (under the dist root) staled by
/// the tick, draining the producer's per-tick buffer — calling it twice
/// for one tick yields the list once, then an empty `Vec`. The dev
/// command wires this to its render session; while the lazy-render
/// switch is off the renderer never marks anything stale and the probe
/// always returns empty, so [`BuildOutcome::pages_stale`] stays empty
/// and behaviour is unchanged.
pub type StaleProbe = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync + 'static>;

/// Function the dev pipeline calls alongside [`StaleProbe`] to collect
/// the "SSR routes were (re)published this tick" bit (issue #1826, Dev
/// Self Heal epic #1999).
///
/// SSR (`prerender = false`) routes have no `dist/` output path, so they
/// can never appear in [`BuildOutcome::pages_stale`] — the staleness map
/// is keyed on output paths and only SSG routes have one. That is
/// precisely why an SSR-only project's self-heal channels drained an
/// empty stale set and broadcast nothing. This probe carries the
/// separate, boolean signal instead: it drains a one-shot flag the dev
/// session sets when it publishes the live SSR route handle, so
/// [`BuildOutcome::ssr_routes_published`] reaches
/// `zfb_server::outcome_to_events` and a tab sitting on the dev 404 body
/// is told to reload. Draining is one-shot, exactly like
/// [`StaleProbe`]: calling it twice for one tick yields `true` once,
/// then `false`.
///
/// This is a **signalling** channel only — it deliberately adds NO
/// server-side staleness machinery for SSR routes, which need none (an
/// SSR route renders per-request the instant its live handle is
/// published).
pub type SsrPublishProbe = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

/// Function the dev pipeline calls alongside [`StaleProbe`] to collect
/// the "previously-rendered DYNAMIC injected routes were re-staled this
/// tick" bit (issue #2097, MDX Reload Fix epic #2092).
///
/// A dynamic injected route (`/preset-articles/[slug]`) has no concrete
/// URL at boot, so it is never a member of `injected_static_seeds` and
/// never enters `routes_by_source`. The dev session therefore re-stales
/// it through its own dedicated channel (`restale_dynamic_injected`),
/// which is a pure staleness-map insert with **no** `tick_stale` push —
/// deliberately, since pushing from that site would reintroduce the
/// documented non-tick-drain race. The consequence was that for a project
/// whose injected routes are ALL dynamic, every SSE-visible channel
/// drained empty: `mark_injected_seeds_stale` early-returned (no static
/// seeds), `lazy_render_tick`'s per-page loop found nothing in
/// `routes_by_source`, [`BuildOutcome::pages_stale`] stayed empty, and
/// `zfb_server::outcome_to_events` emitted nothing — while the very next
/// request already served fresh bytes. An already-open tab never
/// reloaded; that is issue #2063's exact signature.
///
/// This probe carries the separate boolean signal instead, on the same
/// one-shot drain discipline as [`SsrPublishProbe`]: calling it twice for
/// one tick yields `true` once, then `false`.
///
/// Like [`SsrPublishProbe`] this is a **signalling** channel only — it
/// adds no staleness machinery and triggers no eager render, so the
/// hard-won lazy-render narrowing (#958 / #1025 / #1583) is untouched.
pub type DynamicInjectedProbe = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

/// Function the dev pipeline calls before re-rendering pages, when
/// the SSR worker bundle on disk may have changed (a `.tsx` page edit,
/// layout edit, or exported-handler change).
///
/// Implementations typically rebuild the worker bundle and reload the
/// embedded V8 host via [`crate::renderer::reload`]. Failure
/// surfaces as a regular tick error — the watcher stays alive and the
/// dev server keeps the previous state.
///
/// The hook is invoked once per tick when [`crate::RebuildPlan::pages`]
/// is non-empty; the pipeline does not call it for CSS-only or
/// islands-only ticks (those don't move the SSR bundle).
///
/// Returns a [`RefreshOutcome`]: `Refreshed { vanished }` after a real
/// refresh (see the variant docs for the vanished-paths contract), or
/// `Skipped` when the implementation proved nothing observable changed
/// and the pipeline may bypass the render fan-out (issue #956).
pub type RendererReloader = Arc<dyn Fn() -> Result<RefreshOutcome> + Send + Sync + 'static>;

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
///
/// # Deprecation note
///
/// `BuildContext` is superseded by the dedicated [`DevBuildContext`]
/// (for `zfb dev`) and [`ProdBuildContext`] (for `zfb build`). Callers
/// that currently hold a `BuildContext` should migrate: dev callers
/// should switch to [`DevBuildContext`]; prod callers (which never
/// populated the dev-only fields anyway) should switch to
/// [`ProdBuildContext`]. `BuildContext` is kept for the transition period
/// and will be removed once all callers are migrated.
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

    /// Client-scripts bundler callback. Optional: if `None`, client-script
    /// reruns are silently skipped. Consumed by [`DevAssetPipeline`] only.
    pub run_client_scripts: Option<ClientScriptsRunner>,

    /// Renderer-reload hook invoked once per tick when pages need
    /// re-rendering. See [`RendererReloader`] for the contract.
    /// Optional: tests and one-off callers that don't own a renderer
    /// session pass `None`. Consumed by [`DevAssetPipeline`] only.
    pub reload_renderer: Option<RendererReloader>,
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
            .field(
                "run_client_scripts",
                &self.run_client_scripts.as_ref().map(|_| "<callback>"),
            )
            .field(
                "reload_renderer",
                &self.reload_renderer.as_ref().map(|_| "<callback>"),
            )
            .finish()
    }
}

/// Context for the `zfb dev` (incremental watcher-driven) pipeline.
///
/// Carries only the fields the dev pipeline actually reads. Production
/// builds should use [`ProdBuildContext`] instead.
///
/// This is the replacement for [`BuildContext`] on the dev path. All
/// dev-only callbacks (`run_css`, `run_islands`, `reload_renderer`) live
/// here, not on a shared context type, so the prod pipeline can never
/// accidentally see or misuse them.
#[derive(Clone)]
pub struct DevBuildContext {
    /// Absolute path to the dist directory.
    pub dist_root: PathBuf,

    /// Page renderer callback.
    pub render_pages: PageRenderer,

    /// CSS pipeline callback. `None` = skip CSS reruns.
    pub run_css: Option<CssRunner>,

    /// Islands bundler callback. `None` = skip islands reruns.
    pub run_islands: Option<IslandsRunner>,

    /// Client-scripts bundler callback. `None` = skip client-script reruns.
    pub run_client_scripts: Option<ClientScriptsRunner>,

    /// Renderer-reload hook. `None` = no-op (e.g. no renderer session
    /// active in tests).
    pub reload_renderer: Option<RendererReloader>,
}

impl DevBuildContext {
    /// Convert to the legacy [`BuildContext`] shape. Provided for the
    /// transition period while call sites are being migrated.
    pub fn into_build_context(self) -> BuildContext {
        BuildContext {
            dist_root: self.dist_root,
            render_pages: self.render_pages,
            run_css: self.run_css,
            run_islands: self.run_islands,
            run_client_scripts: self.run_client_scripts,
            reload_renderer: self.reload_renderer,
        }
    }
}

impl std::fmt::Debug for DevBuildContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevBuildContext")
            .field("dist_root", &self.dist_root)
            .field("render_pages", &"<callback>")
            .field("run_css", &self.run_css.as_ref().map(|_| "<callback>"))
            .field(
                "run_islands",
                &self.run_islands.as_ref().map(|_| "<callback>"),
            )
            .field(
                "run_client_scripts",
                &self.run_client_scripts.as_ref().map(|_| "<callback>"),
            )
            .field(
                "reload_renderer",
                &self.reload_renderer.as_ref().map(|_| "<callback>"),
            )
            .finish()
    }
}

/// Context for the `zfb build` (one-shot production) pipeline.
///
/// Contains only the fields [`prod::ProductionAssetPipeline`] needs:
/// a `dist_root` and a `render_pages` callback. The dev-only CSS/islands
/// bool-runners and the renderer-reload hook are absent by design —
/// production assets are handled through the [`prod::AssetEmitter`]
/// mechanism, not through callbacks.
#[derive(Clone)]
pub struct ProdBuildContext {
    /// Absolute path to the dist directory.
    pub dist_root: PathBuf,

    /// Page renderer callback.
    pub render_pages: PageRenderer,
}

impl ProdBuildContext {
    /// Convert to the legacy [`BuildContext`] shape. Provided for the
    /// transition period while call sites are being migrated.
    pub fn into_build_context(self) -> BuildContext {
        BuildContext {
            dist_root: self.dist_root,
            render_pages: self.render_pages,
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        }
    }
}

impl std::fmt::Debug for ProdBuildContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProdBuildContext")
            .field("dist_root", &self.dist_root)
            .field("render_pages", &"<callback>")
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

    /// Per-bundle metadata when the islands bundler produced output
    /// this tick. Populated by [`IslandsRunner`]; the SSE layer fans
    /// this out to one `ReloadEvent::Islands` per component when
    /// `changed` is true.
    pub islands_bundle: Option<IslandsBundleInfo>,

    /// Whether the client-scripts bundler ran this tick.
    pub client_scripts_rerun: bool,

    /// Whether the client-scripts bundler wrote at least one new or
    /// changed file. When `true`, the SSE layer emits a
    /// `ReloadEvent::Page` (full reload — v1 doesn't have a finer-
    /// grained client-script hot-swap event).
    pub client_scripts_changed: bool,

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

    /// Relative output paths (under the dist root) the renderer marked
    /// STALE this tick instead of rendering eagerly (issue #1025 — lazy
    /// dev render). Populated by [`DevAssetPipeline`] from its
    /// [`StaleProbe`], when one was supplied at construction.
    ///
    /// Empty when the lazy-render switch is off (the `ZFB_DEV_EAGER=1`
    /// escape hatch). Since the #1027 activation flip a non-empty list
    /// is part of the dev server's SSE reload gate
    /// (`zfb_server::outcome_to_events`), so a tick that rendered
    /// nothing eagerly still tells the browser to reload — the stale
    /// route then re-renders on request.
    pub pages_stale: Vec<PathBuf>,

    /// Whether this tick published the dev server's live SSR
    /// (`prerender = false`) route handle (issue #1826, Dev Self Heal
    /// epic #1999).
    ///
    /// The ONE shared signal both deferred-window self-heal channels in
    /// `zfb dev` set — the healthy deferred boot publish (#1182 / Cold
    /// premark #1808) and the cold-bootstrap recovery seam (#1809) — so
    /// the two paths self-heal an open tab identically. Populated by
    /// [`DevAssetPipeline`] from its [`SsrPublishProbe`] (watcher ticks)
    /// and by the dev command's boot hook (boot), and left `false` by
    /// every other pipeline.
    ///
    /// Exists because SSR routes have no `dist/` output path and so can
    /// never appear in [`Self::pages_stale`]: without this bit an
    /// SSR-only project's `BuildOutcome` is indistinguishable from an
    /// empty tick and `zfb_server::outcome_to_events` emits nothing,
    /// leaving a tab stuck on the dev 404 body even though the very next
    /// GET is already a 200. It folds into the SAME `ReloadEvent::Page`
    /// gate as `pages_stale`, so a mixed SSG/SSR project that sets both
    /// still emits exactly one `Page` event.
    pub ssr_routes_published: bool,

    /// Whether this tick re-staled at least one previously-rendered
    /// DYNAMIC injected route (issue #2097, MDX Reload Fix epic #2092).
    ///
    /// Populated by [`DevAssetPipeline`] from its
    /// [`DynamicInjectedProbe`] (watcher ticks) and by the dev command's
    /// boot hook (boot), and left `false` by every other pipeline.
    ///
    /// Exists for the same structural reason as
    /// [`Self::ssr_routes_published`]: a dynamic injected route is
    /// re-staled through a channel that deliberately performs no
    /// `tick_stale` push, so it can never appear in [`Self::pages_stale`].
    /// Without this bit, a project whose injected routes are all dynamic
    /// produces a `BuildOutcome` indistinguishable from an empty tick,
    /// and `zfb_server::outcome_to_events` emits nothing — leaving an
    /// open tab on stale HTML even though the next GET already serves
    /// fresh bytes (issue #2063). It folds into the SAME
    /// `ReloadEvent::Page` gate as `pages_stale`, so a mixed tick that
    /// sets several of them still emits exactly one `Page` event.
    ///
    /// `false` for any project with no dynamic injected routes — the bit
    /// is raised only when a non-empty set was actually re-staled, never
    /// on membership or on every swap.
    pub dynamic_injected_restaled: bool,
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
