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

/// One rendered page's HTML output.
///
/// The pipeline writes `html` to `<dist_root>/<output_path>` atomically.
/// `output_path` should be relative to the dist root and must be a safe
/// subpath (no `..`). Producers typically derive this from the page's
/// route template — e.g. `/blog/foo` → `blog/foo/index.html`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    /// The page id this output belongs to. Echoed back verbatim from
    /// the [`PageSelection`] so callers can correlate inputs/outputs.
    pub page: PageId,

    /// Path under the dist root, in URL-style forward-slash form. The
    /// pipeline joins this onto its `dist_root` and writes the HTML
    /// atomically.
    pub output_path: PathBuf,

    /// HTML to write.
    pub html: String,
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

/// Function that runs the islands bundler once and returns whether the
/// emitted asset is new. Same semantics as [`CssRunner`].
pub type IslandsRunner = Arc<dyn Fn() -> Result<bool> + Send + Sync + 'static>;

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
}

impl DevAssetPipeline {
    /// Construct a fresh pipeline with no last-bytes cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the last-bytes cache. Useful when the dist root is wiped
    /// from outside the orchestrator and the caller wants the next
    /// rebuild to re-emit every page.
    pub fn reset_cache(&self) {
        self.last_bytes
            .lock()
            .expect("DevAssetPipeline::last_bytes lock poisoned")
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
            let rendered = (ctx.render_pages)(&pages)?;
            outcome.pages_rendered = rendered.len();

            for r in rendered {
                let dest = ctx.dist_root.join(&r.output_path);
                let new_bytes = r.html.into_bytes();

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
                outcome.islands_changed = run()?;
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
    fn all_must_be_resolved_before_reaching_pipeline() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: None,
            run_islands: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::All,
            rerun_css: false,
            rerun_islands: false,
            triggers: vec![],
        };
        assert!(pipeline.apply(&plan, &ctx).is_err());
    }
}
