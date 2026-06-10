//! [`DevAssetPipeline`] — the watcher-driven incremental pipeline.
//!
//! Behaviour:
//!
//! - Calls `ctx.render_pages` for every selected page (or "all known
//!   pages" if the plan is `PageSelection::All` — but the orchestrator
//!   resolves "All" before handing the plan in, so this impl just
//!   handles `Specific`).
//! - Calls `ctx.run_css` if the plan asks for it.
//! - Calls `ctx.run_islands` if the plan asks for it.
//! - Writes each `RenderedPage.html` atomically under `ctx.dist_root`.
//! - Tracks bytes-changed-or-not so [`super::BuildOutcome::pages_written`]
//!   reflects only pages whose bytes actually moved (cheap reload signal
//!   for the dev server's WebSocket).
//! - Filenames stay **stable across rebuilds**. The dev server's URL
//!   contract does not change between watcher ticks; production-style
//!   content hashing is the production pipeline's job (see
//!   [`super::prod::ProductionAssetPipeline`]).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use zfb_graph::PageId;

use crate::atomic::{atomic_write, validate_output_path};
use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome, RefreshOutcome};
use crate::plan::{PageSelection, RebuildPlan};

/// The default `zfb dev`-mode asset pipeline.
#[derive(Debug, Default)]
pub struct DevAssetPipeline {
    // Last-known bytes per output path, used to skip writes when the
    // renderer is deterministic and re-emits the same HTML. Wrapped in
    // a Mutex so &self.apply() can mutate it; the lock is held for the
    // duration of one tick which is fine for dev.
    last_bytes: Mutex<HashMap<PathBuf, Vec<u8>>>,

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
    last_output_path: Mutex<HashMap<PageId, PathBuf>>,
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
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "DevAssetPipeline.last_bytes",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            })
            .clear();
        self.last_output_path
            .lock()
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "DevAssetPipeline.last_output_path",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            })
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

        // Trigger the renderer reload for SSR-only projects (issue #807): when
        // every page is `prerender = false`, the plan's SSG page set is always
        // empty, so the normal pages-non-empty guard would never fire. But the
        // V8 host still needs a fresh bundle after each relevant edit so SSR
        // requests serve the updated code. Fire the reload here when:
        //   - the tick is SSR-relevant (Content/Page/Module/Data change), AND
        //   - the plan has no SSG pages (would be missed by the loop below), AND
        //   - the renderer was not already refreshed this tick.
        // For mixed projects (some SSG pages) this block is skipped — the pages
        // loop below fires the reload as before, so there is no double-reload.
        if pages.is_empty() && plan.ssr_reload_needed && !plan.renderer_fresh {
            if let Some(reload) = &ctx.reload_renderer {
                // Vanished paths from an SSR-only reload are appended to
                // prune_paths below; the pages loop's prune infrastructure
                // is bypassed here so we handle them in the plan-prune block.
                // A `Skipped` outcome (issue #956) means the live host and
                // route tables were left untouched — nothing vanished,
                // nothing to prune.
                let vanished = match reload()? {
                    RefreshOutcome::Skipped => Vec::new(),
                    RefreshOutcome::Refreshed { vanished, .. } => vanished,
                };
                if !vanished.is_empty() {
                    outcome.pages_pruned.extend(vanished.iter().cloned());
                    for prev in &vanished {
                        let _ = std::fs::remove_file(prev);
                        self.last_bytes
                            .lock()
                            .unwrap_or_else(|p| {
                                tracing::warn!(
                                    site = "DevAssetPipeline.last_bytes (ssr-only-prune)",
                                    "mutex poisoned, recovering"
                                );
                                p.into_inner()
                            })
                            .remove(prev);
                        {
                            let mut last_out = self.last_output_path.lock().unwrap_or_else(|p| {
                                tracing::warn!(
                                    site = "DevAssetPipeline.last_output_path (ssr-only-prune)",
                                    "mutex poisoned, recovering"
                                );
                                p.into_inner()
                            });
                            last_out.retain(|_, v| v != prev);
                        }
                    }
                }
            }
        }

        // Vanished route output paths consumed by the deferred-prune loop
        // inside the render block: plan-carried paths (issue #804 P2) plus
        // whatever the reloader reports. Hoisted out of the render block so
        // the Skipped path (issue #956) leaves them to the plan-prune block
        // (1b) below instead.
        let mut route_vanished: Vec<PathBuf> = plan.prune_paths.clone();

        // True when the reloader reported [`RefreshOutcome::Skipped`]
        // (issue #956): byte-identical bundle + unchanged route universe.
        // The render fan-out is bypassed for the tick — re-rendering
        // against an identical bundle would re-emit identical HTML, the
        // same determinism assumption the `last_bytes` byte-dedup cache
        // already makes. CSS, islands, and plan prune paths still run.
        let mut renderer_refresh_skipped = false;

        // Content-narrowing hint for the render callback (issue #958).
        // Defensive G6: the hint must never coexist with a discovery-
        // refreshed renderer (`renderer_fresh`) — the discovery regime
        // implies created files, which the orchestrator never produces a
        // hint for, but if both ever appear together the safe direction
        // is full fan-out.
        let mut narrowing: Option<&crate::ContentNarrowing> = plan.content_narrowing.as_ref();
        if plan.renderer_fresh {
            narrowing = None;
        }

        if !pages.is_empty() {
            // Reload the SSR renderer (embedded V8 host) before rendering
            // pages whenever the dirty set is non-empty — UNLESS the
            // renderer bundle was already refreshed this tick (boot's
            // eager initial render, or a watch-ADD discovery re-bundle;
            // see `RebuildPlan::renderer_fresh`). Errors abort the
            // tick — the watcher stays alive and the previous renderer
            // state is preserved by the orchestrator.
            //
            // The reloader also returns the globally-vanished absolute
            // output paths (routes that existed before this tick's
            // route-table rebuild but are absent from every source's new
            // entry set). These are fed into the prune loop below.
            //
            // When the renderer was already refreshed this tick (e.g. by
            // the discovery hook), reload_renderer is skipped — but the
            // plan may still carry vanished paths from that same discovery
            // refresh via RebuildPlan::prune_paths (issue #804 P2).
            if !plan.renderer_fresh {
                if let Some(reload) = &ctx.reload_renderer {
                    match reload()? {
                        RefreshOutcome::Skipped => renderer_refresh_skipped = true,
                        RefreshOutcome::Refreshed {
                            vanished,
                            changed_sources,
                        } => {
                            // Fallback G5 (issue #958): the refresh ran
                            // BEFORE the render, so the route table the
                            // narrowing would match against is the
                            // post-edit one — but if the refresh reports
                            // that any source's route-entry set changed
                            // (or any output path vanished), the route
                            // structure moved this tick and narrowing
                            // could orphan a brand-new URL. Render the
                            // full selected set instead.
                            if !vanished.is_empty() || !changed_sources.is_empty() {
                                narrowing = None;
                            }
                            route_vanished.extend(vanished);
                        }
                    }
                }
            }
        }

        // Prune safety on a skipped tick (issue #956 gate (c) / #727): when
        // the refresh was skipped, the route table did not move, so nothing
        // vanished this tick. By bypassing this whole block neither the
        // stale-output candidates nor the live-dests bookkeeping run — an
        // unrendered URL can never be mistaken for a vanished one.
        if !pages.is_empty() && !renderer_refresh_skipped {
            let rendered = (ctx.render_pages)(&pages, narrowing)?;
            outcome.pages_rendered = rendered.len();

            // Collect prune candidates and the live dest set during the
            // write loop; the actual deletes are deferred to after the
            // loop. This prevents a page's prune from deleting a path
            // that a sibling page in the same tick has already written
            // (or will write) to — i.e. the two-page output-path swap
            // scenario described in issue #727.
            let mut prune_candidates: Vec<PathBuf> = Vec::new();
            let mut live_dests: HashSet<PathBuf> = HashSet::new();

            for r in rendered {
                // Reject any output_path that escapes dist_root via
                // `..` or absolute roots before we touch the
                // filesystem. Paths come from the renderer/router but
                // we still validate at the write boundary.
                let dest = validate_output_path(&ctx.dist_root, r.output_path.as_path())
                    .with_context(|| format!("while building page {:?}", r.page))?;
                let new_bytes = r.html.into_bytes();

                // Every dest produced in this tick is "live" regardless
                // of whether its bytes changed. Record it before any
                // conditional logic so unchanged pages still protect
                // their path from a sibling's deferred prune.
                live_dests.insert(dest.clone());

                // Collect any stale-output prune candidate for this
                // page. The actual delete is deferred to after the loop
                // so we can cross-check against live_dests first.
                // We still defer updating `last_output_path` until the
                // write succeeds: a transient write failure aborts the
                // tick (via `?`) before the deferred prune runs, so the
                // previous path mapping is preserved for the next tick.
                {
                    let last_out = self.last_output_path.lock().unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_output_path",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    });
                    if let Some(prev) = last_out.get(&r.page) {
                        if prev != &dest {
                            prune_candidates.push(prev.clone());
                        }
                    }
                }

                let changed = {
                    let cache = self.last_bytes.lock().unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_bytes",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    });
                    !matches!(cache.get(&dest), Some(prev) if prev == &new_bytes)
                };

                // After a prune the new path is by definition different
                // from anything we've written before, so we always
                // (re-)emit. `changed` already flips true via the
                // cache miss above. We still want to write before we
                // delete the previous artifact below.

                if changed {
                    // Pass the raw bytes through — `atomic_write` is
                    // the canonical write helper. Going through
                    // `from_utf8(...).unwrap_or("")` previously turned
                    // any encoding hiccup into a silent blank file.
                    atomic_write(&dest, &new_bytes)?;
                    // Only after the write succeeds do we record the
                    // new bytes in the dedup cache. If we updated the
                    // cache before the write a transient I/O failure
                    // would poison it: the next tick's identical bytes
                    // would be deemed "unchanged" and the file would
                    // silently never make it to disk.
                    let mut cache = self.last_bytes.lock().unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_bytes (commit)",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    });
                    cache.insert(dest.clone(), new_bytes.clone());
                    outcome.pages_written.push(r.page.clone());
                }

                // Only after a successful write do we record the new
                // output path. If the write above failed we abort the
                // tick and the previous mapping is preserved, so the
                // stale file remains prunable on the next rebuild.
                self.last_output_path
                    .lock()
                    .unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_output_path (commit)",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    })
                    .insert(r.page.clone(), dest.clone());
            }

            // Deferred prune: remove candidates that are no longer live.
            // Skip any path that appears in live_dests — another page in
            // this same tick now owns it, so deleting it would remove
            // that sibling's freshly-written artifact (the #727 bug).
            //
            // Also prune any globally-vanished route output paths returned
            // by reload_renderer (routes that disappeared from the route
            // table after this tick's rebuild — e.g. a content file was
            // deleted or a dynamic route's paths() output shrank). Apply
            // the same live_dests guard: if route A lost /x while route B
            // simultaneously gained /x, /x must NOT be deleted.
            for prev in prune_candidates.into_iter().chain(route_vanished) {
                if live_dests.contains(&prev) {
                    continue; // another page now owns this path — skip
                }
                let _ = std::fs::remove_file(&prev);
                self.last_bytes
                    .lock()
                    .unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_bytes (prune)",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    })
                    .remove(&prev);
                // Also evict the stale path from the last_output_path
                // cache so the pipeline doesn't attempt to prune it again
                // on the next tick.
                {
                    let mut last_out = self.last_output_path.lock().unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_output_path (route-prune)",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    });
                    last_out.retain(|_, v| v != &prev);
                }
                outcome.pages_pruned.push(prev);
            }
        }

        // 1b. Prune paths from the plan (issue #804 P2): paths explicitly
        // supplied by the orchestrator from a discovery refresh that already
        // set renderer_fresh (so reload_renderer was skipped and these paths
        // could not be returned through the normal vanished-routes channel).
        // Only runs when the render loop above did not — pages empty, or the
        // renderer refresh was skipped (issue #956; defensive: a discovery
        // refresh sets renderer_fresh, so a Skipped reload never coincides
        // with plan prune paths in practice). If the render loop ran,
        // prune_paths were already included in route_vanished and processed
        // there.
        if (pages.is_empty() || renderer_refresh_skipped) && !plan.prune_paths.is_empty() {
            for prev in &plan.prune_paths {
                let _ = std::fs::remove_file(prev);
                self.last_bytes
                    .lock()
                    .unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_bytes (plan-prune)",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    })
                    .remove(prev);
                {
                    let mut last_out = self.last_output_path.lock().unwrap_or_else(|p| {
                        tracing::warn!(
                            site = "DevAssetPipeline.last_output_path (plan-prune)",
                            "mutex poisoned, recovering"
                        );
                        p.into_inner()
                    });
                    last_out.retain(|_, v| v != prev);
                }
                outcome.pages_pruned.push(prev.clone());
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
    use crate::pipeline::{RelDistPath, RenderedPage};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
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
            render_pages: Arc::new(
                move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                    invocations.fetch_add(1, Ordering::SeqCst);
                    Ok(rendered.clone())
                },
            ),
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
                output_path: RelDistPath::new("a/index.html").unwrap(),
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
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
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
            output_path: RelDistPath::new("a.html").unwrap(),
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
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
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
            render_pages: Arc::new(|_, _| Ok(vec![])),
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
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
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
            output_path: RelDistPath::new("sitemap.xml").unwrap(),
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
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let first = pipeline.apply(&plan, &ctx_a).unwrap();
        assert_eq!(first.pages_written.len(), 1);
        assert!(
            first.pages_pruned.is_empty(),
            "first build has nothing to prune"
        );
        assert!(dir.path().join("sitemap.xml").exists());

        // Second build: same page, but output_path flipped to
        // sitemap.rss. The pipeline must delete the stale .xml.
        let second_render = vec![RenderedPage {
            page: pid("/p/sitemap.xml.tsx"),
            output_path: RelDistPath::new("sitemap.rss").unwrap(),
            html: "<rss/>".into(),
            content_type: Some("application/rss+xml".into()),
        }];
        let calls_b = Arc::new(AtomicUsize::new(0));
        let ctx_b = ctx_with_renderer(dir.path().to_path_buf(), second_render, calls_b);
        let second = pipeline.apply(&plan, &ctx_b).unwrap();

        assert_eq!(second.pages_written.len(), 1, "new artifact written");
        assert_eq!(
            second.pages_pruned.len(),
            1,
            "old artifact reported as pruned"
        );
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
            output_path: RelDistPath::new("a/index.html").unwrap(),
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
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let first = pipeline.apply(&plan, &ctx).unwrap();
        let second = pipeline.apply(&plan, &ctx).unwrap();
        assert!(first.pages_pruned.is_empty());
        assert!(second.pages_pruned.is_empty());
    }

    #[test]
    fn stale_output_prune_skipped_when_sibling_claims_path() {
        // Regression test for #727: when two pages swap output paths in
        // a single tick the deferred prune must not delete the path that
        // a sibling just claimed.
        //
        // Topology:
        //   Tick 1: A → shared.html, B → b.html
        //   Tick 2: A → a.html,      B → shared.html  (B claims A's old path)
        //
        // Expected after tick 2:
        //   - shared.html still exists with B's content (not deleted)
        //   - a.html exists with A's content
        //   - b.html is gone (legitimately pruned — nothing claims it)
        //   - pages_pruned does NOT contain shared.html
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        // -- Tick 1 ----------------------------------------------------------
        let tick1 = vec![
            RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("shared.html").unwrap(),
                html: "<p>A tick1</p>".into(),
                content_type: None,
            },
            RenderedPage {
                page: pid("/p/b.tsx"),
                output_path: RelDistPath::new("b.html").unwrap(),
                html: "<p>B tick1</p>".into(),
                content_type: None,
            },
        ];
        let ctx1 = ctx_with_renderer(
            dir.path().to_path_buf(),
            tick1,
            Arc::new(AtomicUsize::new(0)),
        );
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        sel.insert(pid("/p/b.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let first = pipeline.apply(&plan, &ctx1).unwrap();
        assert_eq!(first.pages_written.len(), 2);
        assert!(first.pages_pruned.is_empty(), "tick 1 has nothing to prune");
        assert!(dir.path().join("shared.html").exists());
        assert!(dir.path().join("b.html").exists());

        // -- Tick 2: B claims A's previous path ------------------------------
        let tick2 = vec![
            RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("a.html").unwrap(),
                html: "<p>A tick2</p>".into(),
                content_type: None,
            },
            RenderedPage {
                page: pid("/p/b.tsx"),
                output_path: RelDistPath::new("shared.html").unwrap(),
                html: "<p>B tick2</p>".into(),
                content_type: None,
            },
        ];
        let ctx2 = ctx_with_renderer(
            dir.path().to_path_buf(),
            tick2,
            Arc::new(AtomicUsize::new(0)),
        );
        let second = pipeline.apply(&plan, &ctx2).unwrap();

        // shared.html is claimed by B — must NOT be in pages_pruned.
        assert!(
            !second
                .pages_pruned
                .contains(&dir.path().join("shared.html")),
            "shared.html is a live dest for B — must not be pruned; pages_pruned={:?}",
            second.pages_pruned,
        );
        // shared.html must still exist with B's tick-2 content.
        assert!(
            dir.path().join("shared.html").exists(),
            "shared.html must still exist after tick 2",
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("shared.html")).unwrap(),
            "<p>B tick2</p>",
        );
        // a.html must exist with A's tick-2 content.
        assert!(
            dir.path().join("a.html").exists(),
            "a.html must exist after A moves to it",
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.html")).unwrap(),
            "<p>A tick2</p>",
        );
        // b.html was abandoned by B with nothing claiming it — it should
        // be pruned (this exercises the normal stale-prune path).
        assert!(
            !dir.path().join("b.html").exists(),
            "b.html was abandoned by B and should be pruned",
        );
        assert!(
            second.pages_pruned.contains(&dir.path().join("b.html")),
            "b.html must appear in pages_pruned",
        );
    }

    #[test]
    fn all_must_be_resolved_before_reaching_pipeline() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::All,
            rerun_css: false,
            rerun_islands: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        assert!(pipeline.apply(&plan, &ctx).is_err());
    }

    #[test]
    fn dev_pipeline_does_not_emit_hashed_asset_urls() {
        // Contract: DevAssetPipeline never populates
        // BuildOutcome::hashed_asset_urls. That field is reserved for
        // ProductionAssetPipeline so callers can distinguish "did the
        // production hashing run?" from "no hashing happened".
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: Some(Arc::new(|| Ok(true))),
            run_islands: Some(Arc::new(|| {
                Ok(Some(crate::pipeline::IslandsBundleInfo {
                    changed: true,
                    bundle_url: "/assets/islands-test.js".into(),
                    components: vec![],
                }))
            })),
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: true,
            rerun_islands: true,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(
            outcome.hashed_asset_urls.is_empty(),
            "dev pipeline must leave hashed_asset_urls empty; got {:?}",
            outcome.hashed_asset_urls,
        );
    }

    // ── RefreshOutcome::Skipped — render-fan-out bypass (issue #956) ─────

    /// Context whose reload reports the given outcome on every tick and
    /// counts render invocations.
    fn ctx_with_reload_outcome(
        dist_root: PathBuf,
        rendered: Vec<RenderedPage>,
        render_calls: Arc<AtomicUsize>,
        outcome: RefreshOutcome,
    ) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(
                move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(rendered.clone())
                },
            ),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(move || Ok(outcome.clone()))),
        }
    }

    fn single_page_plan() -> RebuildPlan {
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        }
    }

    /// A `Skipped` reload must bypass the render fan-out entirely: the
    /// render callback is never invoked, no HTML is written, and nothing
    /// already on disk is pruned (gate (c) — an unrendered URL must not
    /// be mistaken for a vanished one).
    #[test]
    fn skipped_reload_bypasses_render_and_prunes_nothing() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let rendered = vec![RenderedPage {
            page: pid("/p/a.tsx"),
            output_path: RelDistPath::new("a/index.html").unwrap(),
            html: "<h1>A</h1>".into(),
            content_type: None,
        }];
        let plan = single_page_plan();

        // Tick 1: a real refresh renders and writes as usual.
        let calls1 = Arc::new(AtomicUsize::new(0));
        let ctx1 = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            rendered.clone(),
            calls1.clone(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
        );
        let first = pipeline.apply(&plan, &ctx1).unwrap();
        assert_eq!(first.pages_rendered, 1);
        assert!(dir.path().join("a/index.html").exists());

        // Tick 2: byte-identical bundle → Skipped. No render, no write,
        // no prune; the existing HTML stays on disk.
        let calls2 = Arc::new(AtomicUsize::new(0));
        let ctx2 = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            rendered,
            calls2.clone(),
            RefreshOutcome::Skipped,
        );
        let second = pipeline.apply(&plan, &ctx2).unwrap();
        assert_eq!(
            calls2.load(Ordering::SeqCst),
            0,
            "Skipped tick must not invoke the render callback"
        );
        assert_eq!(second.pages_rendered, 0, "no pages rendered on skip");
        assert!(second.pages_written.is_empty(), "no HTML written on skip");
        assert!(
            second.pages_pruned.is_empty(),
            "skip must not treat unrendered URLs as vanished; pruned={:?}",
            second.pages_pruned,
        );
        assert!(
            dir.path().join("a/index.html").exists(),
            "existing HTML must survive a skipped tick"
        );
    }

    /// A `Skipped` reload bypasses only the render loop — CSS and islands
    /// requested by the plan must still run.
    #[test]
    fn skipped_reload_still_runs_css_and_islands() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let render_calls = Arc::new(AtomicUsize::new(0));
        let render_calls_cb = render_calls.clone();
        let css_calls = Arc::new(AtomicUsize::new(0));
        let css_calls_cb = css_calls.clone();
        let islands_calls = Arc::new(AtomicUsize::new(0));
        let islands_calls_cb = islands_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(
                move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                    render_calls_cb.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![])
                },
            ),
            run_css: Some(Arc::new(move || {
                css_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })),
            run_islands: Some(Arc::new(move || {
                islands_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })),
            reload_renderer: Some(Arc::new(|| Ok(RefreshOutcome::Skipped))),
        };

        let mut plan = single_page_plan();
        plan.rerun_css = true;
        plan.rerun_islands = true;

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(render_calls.load(Ordering::SeqCst), 0, "render bypassed");
        assert!(outcome.css_rerun, "CSS still reruns on a skipped tick");
        assert_eq!(css_calls.load(Ordering::SeqCst), 1);
        assert!(
            outcome.islands_rerun,
            "islands still rerun on a skipped tick"
        );
        assert_eq!(islands_calls.load(Ordering::SeqCst), 1);
    }

    /// Plan-carried prune paths (issue #804 P2) are still processed when
    /// the reload reports `Skipped`. Defensive: a discovery refresh sets
    /// `renderer_fresh` so the combination cannot occur in production
    /// wiring, but the pipeline contract is "prune paths always honoured".
    #[test]
    fn skipped_reload_still_processes_plan_prune_paths() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let stale = dir.path().join("stale.html");
        std::fs::write(&stale, "<p>stale</p>").unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            vec![],
            calls.clone(),
            RefreshOutcome::Skipped,
        );
        let mut plan = single_page_plan();
        plan.prune_paths = vec![stale.clone()];

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0, "render bypassed");
        assert!(!stale.exists(), "plan prune path must still be deleted");
        assert!(
            outcome.pages_pruned.contains(&stale),
            "pages_pruned must report the plan prune path; got {:?}",
            outcome.pages_pruned,
        );
    }

    /// SSR-only tick (no SSG pages, `ssr_reload_needed`): a `Skipped`
    /// reload leaves the dist tree untouched — nothing vanished because
    /// the route table did not move.
    #[test]
    fn ssr_only_skipped_reload_prunes_nothing() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let existing = dir.path().join("page.html");
        std::fs::write(&existing, "<p>live</p>").unwrap();

        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(|| Ok(RefreshOutcome::Skipped))),
        };
        let mut plan = single_page_plan();
        plan.pages = PageSelection::none();
        plan.ssr_reload_needed = true;

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.pages_pruned.is_empty(), "nothing vanished on skip");
        assert!(existing.exists(), "existing output must survive");
    }

    /// A `Refreshed` reload (even with nothing vanished) renders as
    /// before — the skip bypass only fires on the explicit `Skipped`
    /// state.
    #[test]
    fn refreshed_reload_renders_as_before() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            vec![RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("a/index.html").unwrap(),
                html: "<h1>A</h1>".into(),
                content_type: None,
            }],
            calls.clone(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
        );
        let outcome = pipeline.apply(&single_page_plan(), &ctx).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "render ran");
        assert_eq!(outcome.pages_rendered, 1);
        assert!(dir.path().join("a/index.html").exists());
    }

    // ── Content-narrowing hint forwarding (issue #958) ───────────────────

    /// Context whose render callback records the narrowing hint it
    /// received and whose reloader reports the given outcome.
    fn ctx_recording_narrowing(
        dist_root: PathBuf,
        outcome: RefreshOutcome,
        seen: Arc<Mutex<Vec<Option<crate::ContentNarrowing>>>>,
    ) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(
                move |_pages: &[PageId], narrowing: Option<&crate::ContentNarrowing>| {
                    seen.lock().unwrap().push(narrowing.cloned());
                    Ok(vec![])
                },
            ),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(move || Ok(outcome.clone()))),
        }
    }

    fn narrowed_plan() -> RebuildPlan {
        let mut plan = single_page_plan();
        plan.content_narrowing = Some(crate::ContentNarrowing {
            changed_content: vec![PathBuf::from("/proj/content/post.md")],
        });
        plan
    }

    /// Happy path: a plan-carried narrowing hint reaches the render
    /// callback verbatim when the refresh reports no structural change.
    #[test]
    fn narrowing_hint_reaches_render_callback() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
            seen.clone(),
        );
        pipeline.apply(&narrowed_plan(), &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "render callback invoked once");
        assert_eq!(
            seen[0],
            Some(crate::ContentNarrowing {
                changed_content: vec![PathBuf::from("/proj/content/post.md")],
            }),
            "the hint must be forwarded when the route table did not move"
        );
    }

    /// Fallback G5 (issue #958): a refresh that reports changed source
    /// route sets disables narrowing for the tick — the render callback
    /// must see `None`.
    #[test]
    fn refresh_changed_sources_disable_narrowing() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![pid("/p/blog/[slug].tsx")],
            },
            seen.clone(),
        );
        pipeline.apply(&narrowed_plan(), &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0], None,
            "non-empty changed_sources must force full fan-out (G5)"
        );
    }

    /// Fallback G5, vanished arm: globally-vanished routes also mean the
    /// route structure moved — no narrowing this tick.
    #[test]
    fn refresh_vanished_routes_disable_narrowing() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![dir.path().join("gone/index.html")],
                changed_sources: vec![],
            },
            seen.clone(),
        );
        pipeline.apply(&narrowed_plan(), &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0], None,
            "non-empty vanished must force full fan-out (G5)"
        );
    }

    /// Fallback G6 (defensive): a hint must never survive on a
    /// renderer-fresh tick (discovery regime) — the callback sees `None`.
    #[test]
    fn renderer_fresh_tick_drops_narrowing_hint() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
            seen.clone(),
        );
        let mut plan = narrowed_plan();
        plan.renderer_fresh = true;
        pipeline.apply(&plan, &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0], None,
            "renderer_fresh must drop the narrowing hint (G6)"
        );
    }

    /// Prune-safety on a narrowed tick (issue #958 §7 / #727): when the
    /// render callback returns only a SUBSET of the selected pages (the
    /// narrowed set), the sibling pages' on-disk HTML and pipeline
    /// bookkeeping (`last_bytes` / `last_output_path`) must survive
    /// untouched — an unrendered sibling URL is never treated as
    /// vanished, and a later full tick still byte-dedups it.
    #[test]
    fn narrowed_tick_leaves_sibling_outputs_and_bookkeeping_intact() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        let page_a = RenderedPage {
            page: pid("blog/a/index.html"),
            output_path: RelDistPath::new("blog/a/index.html").unwrap(),
            html: "<p>A v1</p>".into(),
            content_type: None,
        };
        let page_b = RenderedPage {
            page: pid("blog/b/index.html"),
            output_path: RelDistPath::new("blog/b/index.html").unwrap(),
            html: "<p>B v1</p>".into(),
            content_type: None,
        };

        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/blog/[slug].tsx"));
        let mut plan = RebuildPlan::empty();
        plan.pages = PageSelection::Specific(sel);

        // Tick 1: full fan-out renders A and B.
        let full = vec![page_a.clone(), page_b.clone()];
        let ctx1 = ctx_with_renderer(
            dir.path().to_path_buf(),
            full,
            Arc::new(AtomicUsize::new(0)),
        );
        let first = pipeline.apply(&plan, &ctx1).unwrap();
        assert_eq!(first.pages_written.len(), 2);
        assert!(dir.path().join("blog/b/index.html").exists());

        // Tick 2: narrowed — the callback returns ONLY A (new bytes).
        let narrowed = vec![RenderedPage {
            html: "<p>A v2</p>".into(),
            ..page_a.clone()
        }];
        let ctx2 = ctx_with_renderer(
            dir.path().to_path_buf(),
            narrowed,
            Arc::new(AtomicUsize::new(0)),
        );
        let second = pipeline.apply(&plan, &ctx2).unwrap();
        assert_eq!(second.pages_written, vec![page_a.page.clone()]);
        assert!(
            second.pages_pruned.is_empty(),
            "an unrendered sibling must never be treated as vanished; pruned={:?}",
            second.pages_pruned,
        );
        assert!(
            dir.path().join("blog/b/index.html").exists(),
            "sibling HTML must survive a narrowed tick"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("blog/b/index.html")).unwrap(),
            "<p>B v1</p>",
        );

        // Tick 3: full fan-out again with byte-identical content — the
        // sibling's `last_bytes` slot survived the narrowed tick, so B
        // byte-dedups (not re-written); A's v2 bytes also dedup.
        let full_again = vec![
            RenderedPage {
                html: "<p>A v2</p>".into(),
                ..page_a
            },
            page_b,
        ];
        let ctx3 = ctx_with_renderer(
            dir.path().to_path_buf(),
            full_again,
            Arc::new(AtomicUsize::new(0)),
        );
        let third = pipeline.apply(&plan, &ctx3).unwrap();
        assert!(
            third.pages_written.is_empty(),
            "bookkeeping intact: a later full tick byte-skips unchanged \
             siblings; written={:?}",
            third.pages_written,
        );
        assert!(third.pages_pruned.is_empty());
    }
}
