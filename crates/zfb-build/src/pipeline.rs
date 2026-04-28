//! [`AssetPipeline`] trait + [`DevAssetPipeline`] (the default impl for
//! `zfb dev`).
//!
//! Production / SSR / edge builds will eventually plug in their own
//! [`AssetPipeline`] implementations here. The trait is the contract:
//! given a [`crate::RebuildPlan`] and a [`BuildContext`], execute the
//! plan and return a [`BuildOutcome`].
//!
//! ## Why a trait when there's only one impl today?
//!
//! The orchestrator has a fairly opinionated lifecycle: receive a plan,
//! call into renderer/CSS/islands in some order, atomically write
//! everything to `dist/`. That lifecycle is the same shape across dev,
//! production, and edge — but the details differ:
//!
//! - **Dev**: don't minify, watch the graph, error on first failure but
//!   keep the watcher alive.
//! - **Production**: minify, fail-fast on first error, generate
//!   sourcemaps separately, optionally post-process for hashed asset
//!   filenames in HTML.
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
//! (Epic 7's `zfb dev` command) will instantiate concrete renderers /
//! engines and pass closures here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use zfb_graph::PageId;

use crate::atomic::atomic_write_string;
use crate::plan::{PageSelection, RebuildPlan};

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
    /// the [`PageSelection`] so callers can correlate inputs/outputs.
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
/// in HTML will manage that explicitly.
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
pub type IslandsRunner =
    Arc<dyn Fn() -> Result<Option<IslandsBundleInfo>> + Send + Sync + 'static>;

/// Function the dev pipeline calls before re-rendering pages, when
/// the SSR worker bundle on disk may have changed (a `.tsx` page edit,
/// layout edit, or exported-handler change).
///
/// Implementations typically rebuild the worker bundle and respawn the
/// miniflare subprocess via [`crate::renderer::reload`]. Failure
/// surfaces as a regular tick error — the watcher stays alive and the
/// dev server keeps the previous state.
///
/// The hook is invoked once per tick when [`RebuildPlan::pages`] is
/// non-empty; the pipeline does not call it for CSS-only or
/// islands-only ticks (those don't move the SSR bundle).
pub type RendererReloader = Arc<dyn Fn() -> Result<()> + Send + Sync + 'static>;

/// Per-build-tick context handed to [`AssetPipeline::apply`].
///
/// Holds the absolute `dist_root` (where output HTML and assets land)
/// plus the closures the dev pipeline calls to render pages, run CSS,
/// and bundle islands.
#[derive(Clone)]
pub struct BuildContext {
    /// Absolute path to the dist directory. The pipeline writes
    /// `<dist_root>/<rendered_page.output_path>` atomically.
    pub dist_root: PathBuf,

    /// Page renderer callback.
    pub render_pages: PageRenderer,

    /// CSS pipeline callback. Optional: if `None`, CSS reruns are
    /// silently skipped. Used by tests that don't care about CSS.
    pub run_css: Option<CssRunner>,

    /// Islands bundler callback. Optional: if `None`, islands reruns are
    /// silently skipped.
    pub run_islands: Option<IslandsRunner>,

    /// Renderer-reload hook invoked once per tick when pages need
    /// re-rendering. See [`RendererReloader`] for the contract.
    /// Optional: tests and one-off callers that don't own a miniflare
    /// subprocess pass `None`.
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
                "reload_renderer",
                &self.reload_renderer.as_ref().map(|_| "<callback>"),
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

    /// Per-bundle metadata when the islands bundler produced output
    /// this tick. Populated by [`IslandsRunner`]; the SSE layer fans
    /// this out to one `ReloadEvent::Islands` per component when
    /// `changed` is true.
    pub islands_bundle: Option<IslandsBundleInfo>,

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

/// The default `zfb dev`-mode asset pipeline.
///
/// - Calls `ctx.render_pages` for every selected page (or "all known
///   pages" if the plan is `PageSelection::All` — but the orchestrator
///   resolves "All" before handing the plan in, so this impl just
///   handles `Specific`).
/// - Calls `ctx.run_css` if the plan asks for it.
/// - Calls `ctx.run_islands` if the plan asks for it.
/// - Writes each `RenderedPage.html` atomically under `ctx.dist_root`.
/// - Tracks bytes-changed-or-not so [`BuildOutcome::pages_written`]
///   reflects only pages whose bytes actually moved (cheap reload signal
///   for the dev server's WebSocket).
#[derive(Debug, Default)]
pub struct DevAssetPipeline {
    // Last-known bytes per output path, used to skip writes when the
    // renderer is deterministic and re-emits the same HTML. Wrapped in
    // a Mutex so &self.apply() can mutate it; the lock is held for the
    // duration of one tick which is fine for dev.
    last_bytes: Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>,

    // Last-known absolute output path per page id. Used to detect
    // stale-output transitions: when a page's `output_path` changes
    // between builds (typically because its frontmatter `extension`
    // flipped — e.g. `xml` → `rss`), the previous artifact is
    // deleted so dist/ doesn't accumulate orphaned files. Without
    // this the dev server would happily serve the stale `sitemap.xml`
    // alongside the new `sitemap.rss`.
    //
    // Stored as the absolute joined path (`<dist_root>/<output_path>`)
    // so the prune step doesn't need access to the dist root.
    last_output_path: Mutex<std::collections::HashMap<PageId, PathBuf>>,
}

impl DevAssetPipeline {
    /// Construct a fresh pipeline with no last-bytes cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the last-bytes and last-output caches. Useful when the
    /// dist root is wiped from outside the orchestrator and the caller
    /// wants the next rebuild to re-emit every page.
    pub fn reset_cache(&self) {
        self.last_bytes
            .lock()
            .expect("DevAssetPipeline::last_bytes lock poisoned")
            .clear();
        self.last_output_path
            .lock()
            .expect("DevAssetPipeline::last_output_path lock poisoned")
            .clear();
    }
}

impl AssetPipeline for DevAssetPipeline {
    fn apply(&self, plan: &RebuildPlan, ctx: &BuildContext) -> Result<BuildOutcome> {
        let mut outcome = BuildOutcome::default();

        // 1. Pages.
        let pages: Vec<PageId> = match &plan.pages {
            PageSelection::All => {
                // The orchestrator should have resolved this. If we still
                // see All here it means the caller bypassed the
                // orchestrator — surface it as an error rather than
                // silently no-op'ing.
                return Err(anyhow::anyhow!(
                    "DevAssetPipeline: PageSelection::All must be resolved to a concrete page list \
                     by the orchestrator before reaching the pipeline"
                ));
            }
            PageSelection::Specific(s) => s.iter().cloned().collect(),
        };

        if !pages.is_empty() {
            // The SSR bundle on disk may have moved (a TSX page,
            // layout, or exported-handler edit was the trigger). Give
            // the host the chance to rebuild the bundle and respawn
            // miniflare BEFORE we issue render requests against it,
            // so the request loop never hits stale module state.
            // Failures bubble up — the orchestrator surfaces them as
            // "tick failed; watcher staying alive".
            if let Some(reload) = &ctx.reload_renderer {
                reload()?;
            }
            let rendered = (ctx.render_pages)(&pages)?;
            outcome.pages_rendered = rendered.len();

            for r in rendered {
                let dest = ctx.dist_root.join(&r.output_path);
                let new_bytes = r.html.into_bytes();

                // Stale-output prune: if this page previously produced a
                // *different* absolute path (e.g. extension flipped
                // from xml to rss), delete the old artifact and forget
                // its byte cache so we never serve a stale file. The
                // delete is best-effort: if the file was already gone
                // we silently move on.
                let pruned = {
                    let mut last_out = self
                        .last_output_path
                        .lock()
                        .expect("DevAssetPipeline::last_output_path lock poisoned");
                    match last_out.insert(r.page.clone(), dest.clone()) {
                        Some(prev) if prev != dest => {
                            let _ = std::fs::remove_file(&prev);
                            self.last_bytes
                                .lock()
                                .expect("DevAssetPipeline::last_bytes lock poisoned")
                                .remove(&prev);
                            outcome.pages_pruned.push(prev);
                            true
                        }
                        _ => false,
                    }
                };

                let changed = {
                    let mut cache = self
                        .last_bytes
                        .lock()
                        .expect("DevAssetPipeline::last_bytes lock poisoned");
                    match cache.get(&dest) {
                        Some(prev) if prev == &new_bytes => false,
                        _ => {
                            cache.insert(dest.clone(), new_bytes.clone());
                            true
                        }
                    }
                };

                // After a prune the new path is by definition different
                // from anything we've written before, so we always
                // (re-)emit. `changed` already flips true via the
                // cache miss above; this `_pruned` line just documents
                // the invariant for future readers.
                let _ = pruned;

                if changed {
                    atomic_write_string(&dest, std::str::from_utf8(&new_bytes).unwrap_or(""))?;
                    outcome.pages_written.push(r.page);
                }
            }
        }

        // 2. CSS.
        if plan.rerun_css {
            outcome.css_rerun = true;
            if let Some(run) = &ctx.run_css {
                outcome.css_changed = run()?;
            }
        }

        // 3. Islands.
        if plan.rerun_islands {
            outcome.islands_rerun = true;
            if let Some(run) = &ctx.run_islands {
                if let Some(info) = run()? {
                    // SSE layer needs the URL + components even when
                    // changed=false (in case it wants to surface the
                    // rerun in diagnostics); only `changed=true`
                    // actually fans out to the browser.
                    outcome.islands_changed = info.changed;
                    outcome.islands_bundle = Some(info);
                }
            }
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    fn pid(s: &str) -> PageId {
        PageId::new(PathBuf::from(s))
    }

    fn ctx_with_renderer(
        dist_root: PathBuf,
        rendered: Vec<RenderedPage>,
        invocations: Arc<AtomicUsize>,
    ) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(move |_pages: &[PageId]| {
                invocations.fetch_add(1, Ordering::SeqCst);
                Ok(rendered.clone())
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        }
    }

    #[test]
    fn writes_rendered_pages_atomically() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_renderer(
            dir.path().to_path_buf(),
            vec![RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: PathBuf::from("a/index.html"),
                html: "<h1>A</h1>".into(),
                content_type: None,
            }],
            calls.clone(),
        );

        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(outcome.pages_rendered, 1);
        assert_eq!(outcome.pages_written, vec![pid("/p/a.tsx")]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/index.html")).unwrap(),
            "<h1>A</h1>"
        );
    }

    #[test]
    fn skips_write_when_html_unchanged() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let rendered = vec![RenderedPage {
            page: pid("/p/a.tsx"),
            output_path: PathBuf::from("a.html"),
            html: "<p>same</p>".into(),
            content_type: None,
        }];
        let ctx = ctx_with_renderer(dir.path().to_path_buf(), rendered, calls);

        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };

        let first = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(first.pages_written.len(), 1);

        let second = pipeline.apply(&plan, &ctx).unwrap();
        // Bytes are the same — nothing new to write.
        assert_eq!(second.pages_written.len(), 0);
        assert_eq!(second.pages_rendered, 1, "renderer still ran");
    }

    #[test]
    fn islands_runner_populates_bundle_info() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: None,
            run_islands: Some(Arc::new(move || {
                calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(Some(IslandsBundleInfo {
                    changed: true,
                    bundle_url: "/assets/islands-deadbeef.js".to_string(),
                    components: vec!["Counter".to_string(), "Search".to_string()],
                }))
            })),
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: false,
            rerun_islands: true,
            triggers: vec![],
        };
        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.islands_rerun);
        assert!(outcome.islands_changed);
        let info = outcome.islands_bundle.expect("bundle info populated");
        assert_eq!(info.bundle_url, "/assets/islands-deadbeef.js");
        assert_eq!(info.components, vec!["Counter", "Search"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn islands_runner_byte_identical_does_not_flag_changed() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: None,
            run_islands: Some(Arc::new(|| {
                Ok(Some(IslandsBundleInfo {
                    changed: false,
                    bundle_url: "/assets/islands-cafef00d.js".to_string(),
                    components: vec!["Counter".to_string()],
                }))
            })),
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: false,
            rerun_islands: true,
            triggers: vec![],
        };
        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.islands_rerun);
        assert!(!outcome.islands_changed);
        // We still record the URL — the SSE layer ignores the event
        // when changed=false but downstream diagnostics get the info.
        assert!(outcome.islands_bundle.is_some());
    }

    #[test]
    fn css_rerun_invokes_callback() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let css_calls = Arc::new(AtomicUsize::new(0));
        let css_calls_cb = css_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: Some(Arc::new(move || {
                css_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })),
            run_islands: None,
            reload_renderer: None,
        };

        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: true,
            rerun_islands: false,
            triggers: vec![],
        };

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.css_rerun);
        assert!(outcome.css_changed);
        assert_eq!(css_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_output_is_pruned_when_extension_changes() {
        // Sub 49 acceptance criterion: when a page's output_path
        // changes between builds (e.g. frontmatter extension flipped
        // from "xml" to "rss"), the previous artifact must be
        // deleted so dist/ doesn't accumulate orphaned files.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        // First build: emit dist/sitemap.xml.
        let first_render = vec![RenderedPage {
            page: pid("/p/sitemap.xml.tsx"),
            output_path: PathBuf::from("sitemap.xml"),
            html: "<urlset/>".into(),
            content_type: Some("application/xml".into()),
        }];
        let calls_a = Arc::new(AtomicUsize::new(0));
        let ctx_a = ctx_with_renderer(dir.path().to_path_buf(), first_render, calls_a);
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/sitemap.xml.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel.clone()),
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };
        let first = pipeline.apply(&plan, &ctx_a).unwrap();
        assert_eq!(first.pages_written.len(), 1);
        assert!(first.pages_pruned.is_empty(), "first build has nothing to prune");
        assert!(dir.path().join("sitemap.xml").exists());

        // Second build: same page, but output_path flipped to
        // sitemap.rss. The pipeline must delete the stale .xml.
        let second_render = vec![RenderedPage {
            page: pid("/p/sitemap.xml.tsx"),
            output_path: PathBuf::from("sitemap.rss"),
            html: "<rss/>".into(),
            content_type: Some("application/rss+xml".into()),
        }];
        let calls_b = Arc::new(AtomicUsize::new(0));
        let ctx_b = ctx_with_renderer(dir.path().to_path_buf(), second_render, calls_b);
        let second = pipeline.apply(&plan, &ctx_b).unwrap();

        assert_eq!(second.pages_written.len(), 1, "new artifact written");
        assert_eq!(second.pages_pruned.len(), 1, "old artifact reported as pruned");
        assert_eq!(second.pages_pruned[0], dir.path().join("sitemap.xml"));
        assert!(
            !dir.path().join("sitemap.xml").exists(),
            "stale sitemap.xml must be removed",
        );
        assert!(
            dir.path().join("sitemap.rss").exists(),
            "fresh sitemap.rss must exist",
        );
    }

    #[test]
    fn unchanged_output_path_does_not_prune() {
        // Re-rendering the same page to the same path is the common
        // case and must not produce any prune entries.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let render = vec![RenderedPage {
            page: pid("/p/a.tsx"),
            output_path: PathBuf::from("a/index.html"),
            html: "<h1>A</h1>".into(),
            content_type: None,
        }];
        let ctx = ctx_with_renderer(
            dir.path().to_path_buf(),
            render.clone(),
            Arc::new(AtomicUsize::new(0)),
        );
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };
        let first = pipeline.apply(&plan, &ctx).unwrap();
        let second = pipeline.apply(&plan, &ctx).unwrap();
        assert!(first.pages_pruned.is_empty());
        assert!(second.pages_pruned.is_empty());
    }

    #[test]
    fn all_must_be_resolved_before_reaching_pipeline() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::All,
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };
        assert!(pipeline.apply(&plan, &ctx).is_err());
    }

    #[test]
    fn reload_renderer_runs_before_render_pages_when_pages_dirty() {
        // The hook fires for each tick that has a non-empty page set,
        // so the host can rebuild the SSR bundle and respawn miniflare
        // before render_pages issues HTTP requests against it.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let order_reload = order.clone();
        let order_render = order.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(move |_pages: &[PageId]| {
                order_render
                    .lock()
                    .expect("order lock poisoned")
                    .push("render");
                Ok(vec![])
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(move || {
                order_reload
                    .lock()
                    .expect("order lock poisoned")
                    .push("reload");
                Ok(())
            })),
        };
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };
        pipeline.apply(&plan, &ctx).unwrap();
        let observed = order.lock().unwrap().clone();
        assert_eq!(observed, vec!["reload", "render"]);
    }

    #[test]
    fn reload_renderer_skipped_when_no_pages_dirty() {
        // CSS-only or islands-only ticks do not move the SSR bundle —
        // the hook must not be invoked.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_calls_cb = reload_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: Some(Arc::new(|| Ok(true))),
            run_islands: None,
            reload_renderer: Some(Arc::new(move || {
                reload_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })),
        };
        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: true,
            rerun_islands: false,
            triggers: vec![],
        };
        pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(reload_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reload_renderer_error_aborts_tick() {
        // A failed bundle rebuild / miniflare respawn must surface as
        // an error rather than letting render_pages run against stale
        // module state.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let render_calls = Arc::new(AtomicUsize::new(0));
        let render_calls_cb = render_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(move |_| {
                render_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(|| {
                Err(anyhow::anyhow!("bundle rebuild failed"))
            })),
        };
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };
        let err = pipeline.apply(&plan, &ctx).unwrap_err();
        assert!(err.to_string().contains("bundle rebuild failed"));
        assert_eq!(
            render_calls.load(Ordering::SeqCst),
            0,
            "render_pages must not run when the reload hook fails",
        );
    }
}
