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
use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome};
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

        if !pages.is_empty() {
            // Reload the SSR renderer (embedded V8 host) before rendering
            // pages whenever the dirty set is non-empty. Errors abort the
            // tick — the watcher stays alive and the previous renderer
            // state is preserved by the orchestrator.
            if let Some(reload) = &ctx.reload_renderer {
                reload()?;
            }

            let rendered = (ctx.render_pages)(&pages)?;
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
            for prev in prune_candidates {
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
                outcome.pages_pruned.push(prev);
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
            triggers: vec![],
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
            triggers: vec![],
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
            triggers: vec![],
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
    fn dev_pipeline_does_not_emit_hashed_asset_urls() {
        // Contract: DevAssetPipeline never populates
        // BuildOutcome::hashed_asset_urls. That field is reserved for
        // ProductionAssetPipeline so callers can distinguish "did the
        // production hashing run?" from "no hashing happened".
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
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
            triggers: vec![],
        };
        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(
            outcome.hashed_asset_urls.is_empty(),
            "dev pipeline must leave hashed_asset_urls empty; got {:?}",
            outcome.hashed_asset_urls,
        );
    }
}
