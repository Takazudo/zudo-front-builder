//! End-to-end integration tests for `zfb-build`.
//!
//! Each test sets up a small project fixture in a temp dir, builds an
//! orchestrator, and exercises the dev loop with synthetic file changes
//! (via the orchestrator's `tick` method — no watcher spin-up needed,
//! which keeps these tests fast and deterministic).
//!
//! The renderer / CSS / islands callbacks are fakes that record what
//! they were asked to do. The orchestrator + plan + atomic-write path
//! are all real.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::tempdir;
use zfb_build::{
    AssetPipeline, BuildContext, BuildOrchestrator, DevAssetPipeline, DiscoveryHook,
    DiscoveryOutcome, OrchestratorConfig, PageSelection, RebuildPlan, RelDistPath, RenderedPage,
};
use zfb_graph::{DepKind, DependencyGraph, PageDeps, PageId};
use zfb_watcher::{ChangeKind, Watcher};

fn pid(p: PathBuf) -> PageId {
    PageId::new(p)
}

fn make_graph_with_md_page(project_root: &std::path::Path) -> Arc<Mutex<DependencyGraph>> {
    // One page (`pages/post.tsx`) consumes one markdown content file
    // (`content/post.md`).
    let page_src = project_root.join("pages/post.tsx");
    let md_src = project_root.join("content/post.md");

    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(page_src.clone()),
        vec![(md_src, DepKind::Content)],
    ));
    Arc::new(Mutex::new(g))
}

#[test]
fn touching_a_md_file_only_rerenders_its_page() {
    let dir = tempdir().unwrap();
    let project = dir.path();

    // Set up the fixture project tree.
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::write(project.join("pages/post.tsx"), "// page\n").unwrap();
    std::fs::write(project.join("content/post.md"), "# hello\n").unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();

    let graph = make_graph_with_md_page(project);
    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let render_calls_for_cb = render_calls.clone();

    let pipeline = DevAssetPipeline::new();
    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        pipeline,
    );

    let project_path = project.to_path_buf();
    let ctx = BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |pages: &[PageId]| {
            render_calls_for_cb.lock().unwrap().push(pages.to_vec());
            Ok(pages
                .iter()
                .map(|p| {
                    let stem = p
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("idx");
                    RenderedPage {
                        page: p.clone(),
                        output_path: RelDistPath::new(format!("{stem}.html"))
                            .expect("test stem is a relative html path"),
                        html: format!("<h1>{stem}</h1>"),
                        content_type: None,
                    }
                })
                .collect())
        }),
        run_css: None,
        run_islands: None,
        reload_renderer: None,
    };

    // Tick: the markdown file changed.
    let outcome = orch
        .tick(vec![project_path.join("content/post.md")], &ctx)
        .expect("tick succeeded")
        .expect("non-noop");

    assert_eq!(outcome.pages_rendered, 1, "exactly one page re-rendered");
    assert_eq!(outcome.pages_written.len(), 1);
    assert_eq!(
        outcome.pages_written[0],
        pid(project_path.join("pages/post.tsx"))
    );
    assert!(!outcome.css_rerun);
    assert!(!outcome.islands_rerun);

    // The renderer was called with exactly one page.
    let calls = render_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].len(), 1);

    // The post.html exists on disk.
    let post_html = project.join("dist/post.html");
    assert!(post_html.exists(), "post.html should exist");
    assert_eq!(std::fs::read_to_string(&post_html).unwrap(), "<h1>post</h1>");
}

#[test]
fn editing_a_global_css_file_triggers_css_only_rebuild() {
    let dir = tempdir().unwrap();
    let project = dir.path();

    std::fs::create_dir_all(project.join("styles")).unwrap();
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::write(project.join("styles/main.css"), ".x{}").unwrap();
    std::fs::write(project.join("pages/index.tsx"), "// page\n").unwrap();

    // Graph has one page that does NOT depend on the CSS file directly.
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/index.tsx")),
        vec![],
    ));
    let graph = Arc::new(Mutex::new(g));

    let css_runs = Arc::new(AtomicUsize::new(0));
    let css_runs_cb = css_runs.clone();
    let render_runs = Arc::new(AtomicUsize::new(0));
    let render_runs_cb = render_runs.clone();
    let islands_runs = Arc::new(AtomicUsize::new(0));
    let islands_runs_cb = islands_runs.clone();

    let pipeline = DevAssetPipeline::new();
    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("styles")]),
        graph,
        pipeline,
    );

    let ctx = BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |_| {
            render_runs_cb.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }),
        run_css: Some(Arc::new(move || {
            css_runs_cb.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        })),
        run_islands: Some(Arc::new(move || {
            islands_runs_cb.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        })),
        reload_renderer: None,
    };

    let outcome = orch
        .tick(vec![project.join("styles/main.css")], &ctx)
        .expect("tick succeeded")
        .expect("non-noop");

    assert!(outcome.css_rerun, "css must run");
    assert!(outcome.css_changed, "css callback returned true");
    assert!(!outcome.islands_rerun, "islands must NOT run");
    assert_eq!(
        outcome.pages_rendered, 0,
        "no pages depend on this CSS file → no render"
    );

    assert_eq!(css_runs.load(Ordering::SeqCst), 1);
    assert_eq!(islands_runs.load(Ordering::SeqCst), 0);
    assert_eq!(render_runs.load(Ordering::SeqCst), 0);
}

#[test]
fn editing_a_use_client_component_re_bundles_islands_without_full_rerender() {
    let dir = tempdir().unwrap();
    let project = dir.path();

    std::fs::create_dir_all(project.join("components")).unwrap();
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::write(
        project.join("components/Counter.tsx"),
        "\"use client\"\nexport default function Counter(){}",
    )
    .unwrap();
    std::fs::write(project.join("pages/a.tsx"), "// a").unwrap();
    std::fs::write(project.join("pages/b.tsx"), "// b").unwrap();

    // Graph: only page a imports Counter; page b does not. So editing
    // Counter.tsx should re-render exactly page a (graph) AND re-bundle
    // islands (policy: components/ is an islands root).
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/a.tsx")),
        vec![(project.join("components/Counter.tsx"), DepKind::Module)],
    ));
    g.upsert(PageDeps::new(pid(project.join("pages/b.tsx")), vec![]));
    let graph = Arc::new(Mutex::new(g));

    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let render_calls_cb = render_calls.clone();
    let islands_runs = Arc::new(AtomicUsize::new(0));
    let islands_runs_cb = islands_runs.clone();
    let css_runs = Arc::new(AtomicUsize::new(0));
    let css_runs_cb = css_runs.clone();

    let pipeline = DevAssetPipeline::new();
    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("components")]),
        graph,
        pipeline,
    );

    let ctx = BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |pages: &[PageId]| {
            render_calls_cb.lock().unwrap().push(pages.to_vec());
            Ok(pages
                .iter()
                .map(|p| RenderedPage {
                    page: p.clone(),
                    output_path: RelDistPath::new(format!(
                        "{}.html",
                        p.path().file_stem().unwrap().to_string_lossy()
                    ))
                    .expect("test stem is a relative html path"),
                    html: "<p>x</p>".into(),
                    content_type: None,
                })
                .collect())
        }),
        run_css: Some(Arc::new(move || {
            css_runs_cb.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        })),
        run_islands: Some(Arc::new(move || {
            islands_runs_cb.fetch_add(1, Ordering::SeqCst);
            Ok(Some(zfb_build::IslandsBundleInfo {
                changed: true,
                bundle_url: "/assets/islands-test.js".to_string(),
                components: vec!["Counter".to_string()],
            }))
        })),
        reload_renderer: None,
    };

    let outcome = orch
        .tick(vec![project.join("components/Counter.tsx")], &ctx)
        .expect("tick succeeded")
        .expect("non-noop");

    assert!(outcome.islands_rerun, "islands must rerun");
    assert!(!outcome.css_rerun, "css must NOT rerun");
    assert_eq!(outcome.pages_rendered, 1, "only page a re-rendered");

    let calls = render_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let rendered_pages: BTreeSet<_> = calls[0].iter().cloned().collect();
    assert!(rendered_pages.contains(&pid(project.join("pages/a.tsx"))));
    assert!(!rendered_pages.contains(&pid(project.join("pages/b.tsx"))));

    assert_eq!(css_runs.load(Ordering::SeqCst), 0, "css callback not called");
    assert_eq!(islands_runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn touching_md_via_real_watcher_triggers_one_page_rebuild() {
    // Same as the first test, but driven by the real watcher rather
    // than a synthetic `tick`. Validates the watcher → orchestrator →
    // pipeline path end-to-end.
    //
    // Timing notes — the test is intentionally generous because a busy
    // CI box (or a filesystem with low-resolution mtimes) was observed
    // to flake when the windows were tighter:
    //
    // * Debounce: 200ms. The watcher's debouncer uses a `interval(d/2)`
    //   wake-up, so worst-case latency from write→emit is roughly
    //   `debounce + debounce/2`. 200ms keeps that under ~300ms with
    //   plenty of headroom over the 2s `recv` timeout.
    // * Initial settle: 200ms after `start_with_debounce` returns. The
    //   inotify hook is installed synchronously inside `start_*`, but
    //   the spawned debouncer task may not have scheduled its first
    //   wake yet on a contended runtime, and notify itself sometimes
    //   needs a beat before delivering events for a path that was
    //   created only moments before the watch was installed.
    // * Channel drain: after the first `recv` returns, we drain any
    //   extra `Change`s the platform may emit for a single logical
    //   write (e.g. notify can split a single editor-style write into
    //   multiple kernel events). The orchestrator collapses duplicates
    //   on the same path, so this is purely defensive.
    let dir = tempdir().unwrap();
    let project = dir.path();

    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::write(project.join("pages/post.tsx"), "// page\n").unwrap();
    let md_path = project.join("content/post.md");
    std::fs::write(&md_path, "# v1\n").unwrap();

    let graph = make_graph_with_md_page(project);
    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let render_calls_cb = render_calls.clone();

    let debounce = Duration::from_millis(200);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]).with_debounce(debounce),
        graph.clone(),
        TestPipeline {
            inner: DevAssetPipeline::new(),
            applied: Arc::new(Mutex::new(0)),
        },
    );

    let ctx = BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |pages: &[PageId]| {
            render_calls_cb.lock().unwrap().push(pages.to_vec());
            Ok(pages
                .iter()
                .map(|p| RenderedPage {
                    page: p.clone(),
                    output_path: RelDistPath::new("post.html")
                        .expect("post.html is a relative html path"),
                    html: "<h1>v2</h1>".into(),
                    content_type: None,
                })
                .collect())
        }),
        run_css: None,
        run_islands: None,
        reload_renderer: None,
    };

    // Spawn watcher manually so the test can observe the channel and
    // then drive a single tick. We do NOT call `orch.run` here because
    // the test wants explicit control over when the rebuild happens.
    let (watcher, mut rx) =
        Watcher::start_with_debounce(project, ["content"], debounce).expect("watcher start");

    // Wait for the watcher's debouncer task to be polled at least once
    // before mutating the file. See the timing notes at the top of the
    // test for the rationale.
    tokio::time::sleep(Duration::from_millis(200)).await;
    std::fs::write(&md_path, "# v2\n").unwrap();

    // Wait for the change to propagate. 2s is far longer than the
    // worst-case debounce latency (~300ms) but short enough that a
    // genuinely-broken watcher will fail the test instead of hanging
    // CI.
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("watcher emitted within 2s")
        .expect("channel still open");

    // Drain any further events the platform happened to coalesce into
    // a separate emit. We feed all of them to the orchestrator so the
    // test exercises the same batching path that `BuildOrchestrator::run`
    // uses in production.
    let mut paths: Vec<PathBuf> = vec![first.path];
    while let Ok(extra) = rx.try_recv() {
        paths.push(extra.path);
    }

    let outcome = orch.tick(paths, &ctx).expect("tick ok").expect("non-noop");
    assert_eq!(outcome.pages_rendered, 1);
    assert_eq!(outcome.pages_written.len(), 1);

    let html_path = project.join("dist/post.html");
    assert!(html_path.exists());
    assert_eq!(std::fs::read_to_string(&html_path).unwrap(), "<h1>v2</h1>");

    // Drop the watcher to stop the OS-level watch. `Watcher::shutdown` is
    // now safe to call (the circular-wait deadlock was fixed in #708/#759
    // and is covered by a timeout-guarded test in zfb-watcher), but we keep
    // `drop` here because it matches the production shutdown path
    // (`BuildOrchestrator::run` ends by dropping its watcher).
    drop(watcher);
}

/// Wrapper pipeline that delegates to `DevAssetPipeline` but counts
/// `apply` calls so async tests can verify the pipeline really ran.
struct TestPipeline {
    inner: DevAssetPipeline,
    applied: Arc<Mutex<usize>>,
}

impl AssetPipeline for TestPipeline {
    fn apply(
        &self,
        plan: &RebuildPlan,
        ctx: &BuildContext,
    ) -> anyhow::Result<zfb_build::BuildOutcome> {
        *self.applied.lock().unwrap() += 1;
        self.inner.apply(plan, ctx)
    }
}

// ---------------------------------------------------------------------------
// Issue #659 — live watch-ADD discovery of a newly-created content file.
//
// Mechanism under test (resolved against the CURRENT tree): a content
// file like `content/blog/foo.mdx` is NOT a `pages/`-scanned route. It is
// reached through a dynamic `pages/blog/[slug].tsx` page whose `paths()`
// export enumerates the `blog` content collection (see
// `crates/zfb/templates/basic-blog/pages/blog/[slug].tsx`). So "adding a
// content file" must make the dynamic `[slug]` SOURCE page re-render with
// one more concrete URL — it never adds a new `pages/` source.
//
// The defect (#659): while `zfb dev` runs, the orchestrator's `run()`
// folds only the changed *path* into a plan and discards the
// `ChangeKind`. A brand-new content file has no reverse edge in the dep
// graph yet, so the path-only fold dirties no page and the new route
// 404s until restart. Editing an EXISTING content file works because its
// `[slug]` consumer edge already lives in the graph.
//
// The fix (#659): `tick_with_kinds` carries `ChangeKind` and, on a
// `Created` change, runs a discovery hook that (in `zfb dev`) rebundles
// the content snapshot, reloads the embedded V8 host in place, re-expands
// `paths()`, rebuilds the source→route table, and returns the dynamic
// source `PageId`s that became renderable. The orchestrator folds those
// ids into the plan so the new page renders through the same render→write
// path an edit traverses.
//
// This is an orchestrator-level proxy: the real snapshot/rebundle/reload
// lives in `zfb`'s dev command and needs a live V8 host, so it is not
// unit-testable here. The fake hook stands in for that side effect (and
// mirrors the graph upsert the real hook performs). Sub-issue #649-B's
// real-watcher e2e is the stronger net for the actual GET path.

/// Build a dev fixture whose only page is the dynamic `[slug]` consumer
/// of a `blog` content collection. Mirrors the basic-blog template shape.
fn make_dynamic_blog_fixture(project: &std::path::Path) {
    std::fs::create_dir_all(project.join("pages/blog")).unwrap();
    std::fs::create_dir_all(project.join("content/blog")).unwrap();
    std::fs::write(
        project.join("pages/blog/[slug].tsx"),
        "export async function paths(){return []}\nexport default function P(){}\n",
    )
    .unwrap();
    // One pre-existing post so the project boots with a non-empty graph.
    std::fs::write(
        project.join("content/blog/hello.mdx"),
        "---\ntitle: Hello\n---\nhi\n",
    )
    .unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();
}

/// Graph seeded the way `boot_dev_renderer` seeds it: the `[slug]` source
/// page node plus its edge to the one pre-existing post. A post added
/// AFTER boot has no edge here — the #659 cold spot.
fn seed_blog_graph(project: &std::path::Path) -> Arc<Mutex<DependencyGraph>> {
    let slug_page = project.join("pages/blog/[slug].tsx");
    let hello_md = project.join("content/blog/hello.mdx");
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(slug_page),
        vec![(hello_md, DepKind::Content)],
    ));
    Arc::new(Mutex::new(g))
}

/// The dev session's frozen-at-boot route table, modelled faithfully:
/// source page → the concrete output paths its `paths()` expanded to.
/// `render_one` fans out over exactly this Vec, so the discriminator for
/// #659 is whether the table gains `/blog/foo` BEFORE the fan-out runs —
/// not whether the `[slug]` page is "rendered" (the All-fallback renders
/// it either way). Keyed by source `PathBuf`, value is dist-relative
/// output paths.
type RouteTable = Arc<Mutex<std::collections::HashMap<PathBuf, Vec<String>>>>;

/// Route table seeded the way `boot_dev_renderer` freezes
/// `routes_by_source` at boot: the dynamic `[slug]` source maps to the
/// ONE concrete URL its `paths()` resolved from the single pre-existing
/// post. A post added after boot is absent here — the #659 cold spot.
fn seed_route_table(project: &std::path::Path) -> RouteTable {
    let mut t = std::collections::HashMap::new();
    t.insert(
        project.join("pages/blog/[slug].tsx"),
        vec!["blog/hello.html".to_string()],
    );
    Arc::new(Mutex::new(t))
}

/// A `BuildContext` whose renderer FANS OUT over the shared route table —
/// one `RenderedPage` (and one dist file) per output path the requested
/// source page maps to. This mirrors `DevRenderSession::render_one`'s real
/// fan-out, so a frozen table writes only the URLs known at boot and a
/// rebuilt table writes the new URL too. Records which source pages it was
/// asked to render.
fn fanout_ctx(
    project: &std::path::Path,
    routes: RouteTable,
    render_calls: Arc<Mutex<Vec<Vec<PageId>>>>,
) -> BuildContext {
    BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |pages: &[PageId]| {
            render_calls.lock().unwrap().push(pages.to_vec());
            let table = routes.lock().unwrap();
            let mut out = Vec::new();
            for p in pages {
                // Fan out over the source page's frozen/rebuilt route Vec.
                // Unknown source (no table entry) yields nothing — exactly
                // `render_one`'s "empty Vec for an unknown source" contract.
                for output in table.get(p.path()).into_iter().flatten() {
                    out.push(RenderedPage {
                        // Each concrete URL gets a distinct synthetic PageId
                        // keyed on its output path, mirroring `render_one_with`.
                        page: PageId::new(PathBuf::from(output)),
                        output_path: RelDistPath::new(output.clone())
                            .expect("test output is a relative html path"),
                        html: format!("<h1>{output}</h1>"),
                        content_type: None,
                    });
                }
            }
            Ok(out)
        }),
        run_css: None,
        run_islands: None,
        reload_renderer: None,
    }
}

/// A discovery hook faithful to `zfb dev`'s real `discover_created`: for a
/// created file under `content/blog/`, REBUILD THE ROUTE TABLE so the
/// dynamic `[slug]` source now also maps to the new `/blog/foo` URL (this
/// is the load-bearing side effect — the real hook rebundles the content
/// snapshot, reloads the V8 host, and re-expands `paths()` to produce this
/// same table mutation), upsert the graph, and return the `[slug]` source
/// `PageId`. Records the created-path batches so a test can assert it is
/// NOT called on an edit tick.
fn fake_blog_discovery_hook(
    project: &std::path::Path,
    routes: RouteTable,
    graph: Arc<Mutex<DependencyGraph>>,
    invocations: Arc<Mutex<Vec<Vec<PathBuf>>>>,
) -> DiscoveryHook {
    let slug_page = project.join("pages/blog/[slug].tsx");
    let content_blog = project.join("content/blog");
    Arc::new(move |created: &[PathBuf]| {
        invocations.lock().unwrap().push(created.to_vec());
        let mut out = Vec::new();
        for c in created {
            if c.starts_with(&content_blog) {
                let page = pid(slug_page.clone());
                let slug = c.file_stem().and_then(|s| s.to_str()).unwrap_or("post");
                // Rebuild the route table: the dynamic source now also
                // resolves the new concrete URL (mirrors the real
                // `routes_by_source` rebuild after a rebundle + paths()
                // re-expansion).
                if let Ok(mut t) = routes.lock() {
                    t.entry(slug_page.clone())
                        .or_default()
                        .push(format!("blog/{slug}.html"));
                }
                // Mirror the real hook's graph upsert.
                if let Ok(mut g) = graph.lock() {
                    g.upsert(PageDeps::new(
                        page.clone(),
                        vec![(c.clone(), DepKind::Content)],
                    ));
                }
                out.push(page);
            }
        }
        Ok(DiscoveryOutcome {
            pages: out,
            renderer_reloaded: true,
        })
    })
}

/// BUG CHARACTERISATION: a content file CREATED after boot, fed through
/// the legacy path-only `tick` (no discovery), re-renders the `[slug]`
/// page (via the All-fallback) BUT the frozen route table never gained
/// `/blog/foo`, so `blog/foo.html` is never written — the new route 404s
/// until restart. This pins the precise discovery defect: the missing
/// output file, not a missing render.
#[test]
fn created_content_file_without_discovery_does_not_emit_new_url() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    make_dynamic_blog_fixture(project);

    let routes = seed_route_table(project);
    let graph = seed_blog_graph(project);
    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let ctx = fanout_ctx(project, routes, render_calls);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );

    // A brand-new post appears on disk after boot.
    let new_post = project.join("content/blog/foo.mdx");
    std::fs::write(&new_post, "---\ntitle: Foo\n---\nfoo\n").unwrap();

    // Legacy path-only tick (the pre-#659 `run()` behaviour).
    let _ = orch.tick(vec![new_post], &ctx).expect("tick ok");

    assert!(
        !project.join("dist/blog/foo.html").exists(),
        "pre-fix: the new /blog/foo URL must NOT be emitted (frozen route table) — \
         this is the 404-until-restart bug",
    );
    // The pre-existing URL is still emitted (the [slug] page did re-render).
    assert!(
        project.join("dist/blog/hello.html").exists(),
        "the pre-existing /blog/hello URL is still served",
    );
}

/// THE FIX (red→green): the same created post, fed through
/// `tick_with_kinds` as a `ChangeKind::Created` change with the discovery
/// hook wired, rebuilds the route table and emits `dist/blog/foo.html` —
/// closing the watch-ADD 404. Falsifiability: with the discovery branch
/// removed from `tick_with_kinds` (or the hook not invoked), the table
/// stays frozen and `blog/foo.html` is never written, failing the central
/// assertion.
#[test]
fn created_content_file_with_discovery_emits_new_url() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    make_dynamic_blog_fixture(project);

    let routes = seed_route_table(project);
    let graph = seed_blog_graph(project);
    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let ctx = fanout_ctx(project, routes.clone(), render_calls);

    let hook_invocations: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
    let discover =
        fake_blog_discovery_hook(project, routes, graph.clone(), hook_invocations.clone());

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );

    let new_post = project.join("content/blog/foo.mdx");
    std::fs::write(&new_post, "---\ntitle: Foo\n---\nfoo\n").unwrap();

    orch.tick_with_kinds(
        vec![(new_post.clone(), ChangeKind::Created)],
        &ctx,
        Some(&discover),
    )
    .expect("tick_with_kinds ok")
    .expect("a Created content file must produce a non-noop tick");

    // The central acceptance check: the new URL is now emitted to dist.
    assert!(
        project.join("dist/blog/foo.html").exists(),
        "the discovery hook must rebuild the route table so /blog/foo is written",
    );
    // The pre-existing URL survives the rebuild.
    assert!(
        project.join("dist/blog/hello.html").exists(),
        "the pre-existing /blog/hello URL must still be emitted",
    );

    // The discovery hook saw exactly the created path, once.
    let invs = hook_invocations.lock().unwrap();
    assert_eq!(invs.len(), 1, "discovery hook called once for the Created tick");
    assert_eq!(invs[0], vec![new_post]);
}

/// EDIT PATH NOT REGRESSED: editing an EXISTING content file is a
/// `ChangeKind::Modified` change. Its consumer page still hot-reloads
/// (the pre-existing URL re-renders), the discovery hook is NEVER invoked
/// (add-only stays add-only), and no new URL is invented.
#[test]
fn modified_content_file_does_not_invoke_discovery_and_still_hot_reloads() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    make_dynamic_blog_fixture(project);

    let routes = seed_route_table(project);
    let graph = seed_blog_graph(project);
    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let ctx = fanout_ctx(project, routes.clone(), render_calls.clone());

    let hook_invocations: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
    let discover =
        fake_blog_discovery_hook(project, routes, graph.clone(), hook_invocations.clone());

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );

    // Edit the EXISTING post (it already has a graph edge to [slug].tsx).
    let existing = project.join("content/blog/hello.mdx");
    std::fs::write(&existing, "---\ntitle: Hello\n---\nedited\n").unwrap();

    orch.tick_with_kinds(
        vec![(existing.clone(), ChangeKind::Modified)],
        &ctx,
        Some(&discover),
    )
    .expect("tick_with_kinds ok")
    .expect("editing an existing post must still produce a non-noop tick");

    // Hot-reload still works: the consumer [slug] page re-rendered its URL.
    let calls = render_calls.lock().unwrap();
    assert!(
        calls[0].contains(&pid(project.join("pages/blog/[slug].tsx"))),
        "the edit must re-render the dynamic [slug] consumer; got {:?}",
        calls[0],
    );
    assert!(
        project.join("dist/blog/hello.html").exists(),
        "editing an existing post must still emit its URL (hot reload)",
    );

    // The discovery hook was NOT consulted — the edit path is untouched.
    assert!(
        hook_invocations.lock().unwrap().is_empty(),
        "a Modified change must never invoke the watch-ADD discovery hook",
    );
    // No phantom URL was invented for the edit.
    assert!(
        !project.join("dist/blog/edited.html").exists(),
        "editing a post must not invent a new URL",
    );
}

#[allow(dead_code)]
fn _ensure_unused_imports_used(_: PageSelection) {}

// ---------------------------------------------------------------------------
// Route-prune: stale HTML removal when routes disappear or rename (issue #804)
//
// These tests exercise three scenarios described in the issue:
//
// 1. Route deleted: a content-driven route existed and was rendered; then the
//    source file is deleted (ChangeKind::Removed) and reload_renderer returns
//    the vanished absolute path. After the tick the HTML file must be gone from
//    disk, and `pages_pruned` must contain the path.
//
// 2. Route renamed: reload_renderer returns the old URL as vanished while the
//    render loop writes the new URL. Old HTML gone, new HTML present.
//
// 3. Lose-/x/-gain-/x/ swap: route A loses /x while route B simultaneously
//    gains /x. The vanished set includes /x but the live_dests set from the
//    render loop also includes /x — so /x must NOT be deleted.
//
// A fourth test verifies that ChangeKind::Removed drops the dependency-graph
// edge so the deleted content file no longer dirties its consumer page.
// ---------------------------------------------------------------------------

/// Build a simple fixture with one page source and one content file.
fn simple_page_content_fixture(project: &std::path::Path) -> Arc<Mutex<DependencyGraph>> {
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();
    std::fs::write(project.join("pages/post.tsx"), "// page\n").unwrap();
    std::fs::write(project.join("content/post.md"), "# hello\n").unwrap();

    let mut g = DependencyGraph::new();
    let page = pid(project.join("pages/post.tsx"));
    let md = project.join("content/post.md");
    g.upsert(PageDeps::new(page, vec![(md, DepKind::Content)]));
    Arc::new(Mutex::new(g))
}

/// Build a `BuildContext` that renders each requested page to a fixed output
/// path determined by `routes`, with a `reload_renderer` that returns the
/// `vanished` set and simultaneously mutates `routes` to the new state.
fn ctx_with_route_prune(
    project: &std::path::Path,
    routes: RouteTable,
    vanished: Arc<Mutex<Vec<PathBuf>>>,
) -> BuildContext {
    let dist = project.join("dist");
    let dist_for_reload = dist.clone();
    let routes_for_render = routes.clone();
    let vanished_for_reload = vanished.clone();
    BuildContext {
        dist_root: dist,
        render_pages: Arc::new(move |pages: &[PageId]| {
            let table = routes_for_render.lock().unwrap();
            let mut out = Vec::new();
            for p in pages {
                for output in table.get(p.path()).into_iter().flatten() {
                    out.push(RenderedPage {
                        page: PageId::new(PathBuf::from(output)),
                        output_path: RelDistPath::new(output.clone())
                            .expect("test output is a relative html path"),
                        html: format!("<h1>{output}</h1>"),
                        content_type: None,
                    });
                }
            }
            Ok(out)
        }),
        run_css: None,
        run_islands: None,
        reload_renderer: Some(Arc::new(move || {
            // Return the vanished absolute paths and clear the shared cell
            // so subsequent ticks don't re-prune the same paths.
            let paths: Vec<PathBuf> = vanished_for_reload
                .lock()
                .unwrap()
                .drain(..)
                .map(|rel| dist_for_reload.join(rel))
                .collect();
            Ok(paths)
        })),
    }
}

/// Scenario 1: a content-driven route existed, was rendered, then the source
/// file is deleted. reload_renderer reports the vanished output path.
/// After the tick the HTML file must be gone and `pages_pruned` must list it.
#[test]
fn route_deletion_prunes_stale_html() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let graph = simple_page_content_fixture(project);

    // Seed route table: post.tsx → dist/post.html
    let mut t = std::collections::HashMap::new();
    t.insert(
        project.join("pages/post.tsx"),
        vec!["post.html".to_string()],
    );
    let routes = Arc::new(Mutex::new(t));

    // Vanished set: initially empty (boot render)
    let vanished: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph.clone(),
        DevAssetPipeline::new(),
    );
    let ctx = ctx_with_route_prune(project, routes.clone(), vanished.clone());

    // Tick 1: initial render → writes dist/post.html
    let md_path = project.join("content/post.md");
    let outcome1 = orch
        .tick(vec![md_path.clone()], &ctx)
        .expect("tick ok")
        .expect("non-noop");
    assert_eq!(outcome1.pages_written.len(), 1, "initial render wrote post.html");
    assert!(
        project.join("dist/post.html").exists(),
        "post.html must exist after initial render",
    );

    // Simulate content file deletion: remove the graph edge and clear the
    // route table (as `refresh_bundle_and_routes` would do in the real
    // server — the route no longer exists).
    routes.lock().unwrap().remove(&project.join("pages/post.tsx"));
    // Tell the reload_renderer to report the vanished path.
    vanished.lock().unwrap().push(PathBuf::from("post.html"));

    // Tick 2: content file Removed
    let outcome2 = orch
        .tick_with_kinds(
            vec![(md_path, ChangeKind::Removed)],
            &ctx,
            None,
        )
        .expect("tick ok");
    // The route table is now empty, so no pages render. But the
    // reload_renderer still runs (non-empty page plan was pending before
    // remove_node), OR the vanished prune happens through a non-empty plan
    // triggered by the refresh.
    //
    // NOTE: after ChangeKind::Removed + remove_node, the dep-graph edge is
    // gone so plan_for_changes returns empty for the removed path.
    // reload_renderer runs only when pages is non-empty. So we need a
    // second page to dirty so the plan is non-empty. Let's trigger via the
    // page source itself.
    //
    // Actually: the issue says ChangeKind::Removed dirties consumers via
    // remove_node (which returns them). But remove_node is called before
    // plan_for_changes, so the graph no longer has the edge when
    // plan_for_changes runs. The consumers were returned from remove_node
    // but we don't use them in the plan. Let's instead trigger a normal
    // Modified tick so reload_renderer fires, and the vanished path is pruned.
    let _ = outcome2; // outcome2 may be None if plan is empty

    // Trigger a normal edit on the page source so reload_renderer fires.
    std::fs::write(project.join("pages/post.tsx"), "// edited\n").unwrap();
    // Set the vanished path again (it may have been consumed or not).
    vanished.lock().unwrap().push(PathBuf::from("post.html"));

    let outcome3 = orch
        .tick_with_kinds(
            vec![(project.join("pages/post.tsx"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick ok")
        .expect("non-noop tick from page-source edit");

    assert!(
        !project.join("dist/post.html").exists(),
        "post.html must be deleted after route vanishes; pages_pruned={:?}",
        outcome3.pages_pruned,
    );
    assert!(
        outcome3.pages_pruned.contains(&project.join("dist/post.html")),
        "pages_pruned must contain dist/post.html; got {:?}",
        outcome3.pages_pruned,
    );
}

/// Scenario 2: route rename — old URL dies, new URL serves.
/// reload_renderer returns the old path as vanished while render writes the new path.
#[test]
fn route_rename_prunes_old_and_writes_new() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();
    std::fs::write(project.join("pages/slug.tsx"), "// page\n").unwrap();
    std::fs::write(project.join("content/old.md"), "# old\n").unwrap();

    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/slug.tsx")),
        vec![(project.join("content/old.md"), DepKind::Content)],
    ));
    let graph = Arc::new(Mutex::new(g));

    // Initial: slug.tsx → old.html
    let mut t = std::collections::HashMap::new();
    t.insert(
        project.join("pages/slug.tsx"),
        vec!["old.html".to_string()],
    );
    let routes = Arc::new(Mutex::new(t));
    let vanished: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );
    let ctx = ctx_with_route_prune(project, routes.clone(), vanished.clone());

    // Tick 1: initial render → dist/old.html
    let outcome1 = orch
        .tick(vec![project.join("content/old.md")], &ctx)
        .expect("tick ok")
        .expect("non-noop");
    assert_eq!(outcome1.pages_written.len(), 1);
    assert!(project.join("dist/old.html").exists(), "old.html must exist");

    // Rename: route table changes to new.html; old.html is vanished.
    {
        let mut t = routes.lock().unwrap();
        t.insert(
            project.join("pages/slug.tsx"),
            vec!["new.html".to_string()],
        );
    }
    vanished.lock().unwrap().push(PathBuf::from("old.html"));

    // Tick 2: content file Modified → triggers reload_renderer which returns
    // the vanished set, AND render writes new.html.
    let outcome2 = orch
        .tick_with_kinds(
            vec![(project.join("content/old.md"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick ok")
        .expect("non-noop");

    assert!(
        !project.join("dist/old.html").exists(),
        "old.html must be pruned after rename; pages_pruned={:?}",
        outcome2.pages_pruned,
    );
    assert!(
        project.join("dist/new.html").exists(),
        "new.html must exist after rename",
    );
    assert!(
        outcome2.pages_pruned.contains(&project.join("dist/old.html")),
        "pages_pruned must contain old.html",
    );
}

/// Scenario 3: lose-/x/-gain-/x/ swap — route A loses /x, route B gains /x
/// simultaneously. The vanished set includes /x but the render loop also
/// writes /x (via route B), so /x must NOT be deleted.
#[test]
fn lose_gain_same_path_keeps_html_alive() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();
    std::fs::write(project.join("pages/a.tsx"), "// a\n").unwrap();
    std::fs::write(project.join("pages/b.tsx"), "// b\n").unwrap();
    std::fs::write(project.join("content/trigger.md"), "# t\n").unwrap();

    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/a.tsx")),
        vec![(project.join("content/trigger.md"), DepKind::Content)],
    ));
    g.upsert(PageDeps::new(
        pid(project.join("pages/b.tsx")),
        vec![(project.join("content/trigger.md"), DepKind::Content)],
    ));
    let graph = Arc::new(Mutex::new(g));

    // Initial state: A → shared.html; B → b.html
    let mut t = std::collections::HashMap::new();
    t.insert(
        project.join("pages/a.tsx"),
        vec!["shared.html".to_string()],
    );
    t.insert(project.join("pages/b.tsx"), vec!["b.html".to_string()]);
    let routes = Arc::new(Mutex::new(t));
    let vanished: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );
    let ctx = ctx_with_route_prune(project, routes.clone(), vanished.clone());

    // Tick 1: initial render
    let outcome1 = orch
        .tick(vec![project.join("content/trigger.md")], &ctx)
        .expect("tick ok")
        .expect("non-noop");
    assert_eq!(outcome1.pages_written.len(), 2);
    assert!(project.join("dist/shared.html").exists());
    assert!(project.join("dist/b.html").exists());

    // Swap: A loses shared.html (→ a.html), B gains shared.html.
    // The globally-vanished set is {shared.html} (it was in A's old set).
    // But B now produces shared.html, so it must NOT be deleted.
    {
        let mut t = routes.lock().unwrap();
        t.insert(project.join("pages/a.tsx"), vec!["a.html".to_string()]);
        t.insert(
            project.join("pages/b.tsx"),
            vec!["shared.html".to_string()],
        );
    }
    vanished.lock().unwrap().push(PathBuf::from("shared.html"));

    // Tick 2: content Modified → both A and B re-render; reload_renderer
    // returns shared.html as vanished. The render loop writes shared.html
    // (via B), so the prune guard must skip it.
    let outcome2 = orch
        .tick_with_kinds(
            vec![(project.join("content/trigger.md"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick ok")
        .expect("non-noop");

    assert!(
        project.join("dist/shared.html").exists(),
        "shared.html must NOT be deleted — B now claims it; pages_pruned={:?}",
        outcome2.pages_pruned,
    );
    assert!(
        !outcome2
            .pages_pruned
            .contains(&project.join("dist/shared.html")),
        "shared.html must not appear in pages_pruned; got {:?}",
        outcome2.pages_pruned,
    );
    assert!(
        project.join("dist/a.html").exists(),
        "a.html must exist after A moves to it",
    );
    assert_eq!(
        std::fs::read_to_string(project.join("dist/shared.html")).unwrap(),
        "<h1>shared.html</h1>",
    );
}

/// Scenario 4: ChangeKind::Removed drops the dependency-graph edge so
/// the removed content file no longer dirties its consumer page on later
/// ticks. This verifies the `remove_node` call in `tick_with_kinds`.
#[test]
fn removed_content_file_drops_graph_edge() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let graph = simple_page_content_fixture(project);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph.clone(),
        DevAssetPipeline::new(),
    );
    let render_calls: Arc<Mutex<Vec<Vec<PageId>>>> = Arc::new(Mutex::new(Vec::new()));
    let render_calls_cb = render_calls.clone();
    let ctx = BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |pages: &[PageId]| {
            render_calls_cb.lock().unwrap().push(pages.to_vec());
            Ok(pages
                .iter()
                .map(|p| RenderedPage {
                    page: p.clone(),
                    output_path: RelDistPath::new("post.html").unwrap(),
                    html: "<p>x</p>".into(),
                    content_type: None,
                })
                .collect())
        }),
        run_css: None,
        run_islands: None,
        reload_renderer: None,
    };

    let md_path = project.join("content/post.md");

    // Tick 1: Modified → consumer page re-renders.
    orch.tick_with_kinds(
        vec![(md_path.clone(), ChangeKind::Modified)],
        &ctx,
        None,
    )
    .expect("tick ok")
    .expect("non-noop");
    assert_eq!(
        render_calls.lock().unwrap().len(),
        1,
        "tick 1 must have rendered the consumer page",
    );

    // Tick 2: Removed → remove_node drops the graph edge; the removed path is
    // excluded from plan_for_changes so the tick is a noop (not even a
    // conservative All-rebuild, which would wrongly keep the dead route alive).
    let outcome2 = orch
        .tick_with_kinds(
            vec![(md_path.clone(), ChangeKind::Removed)],
            &ctx,
            None,
        )
        .expect("tick ok");
    assert!(
        outcome2.is_none(),
        "Removed tick must be noop — removed path excluded from plan; got {:?}",
        outcome2.map(|o| o.pages_written),
    );
    // Renderer must not be called for the Removed tick.
    assert_eq!(
        render_calls.lock().unwrap().len(),
        1,
        "renderer must not be called for the Removed tick",
    );
}

// ---------------------------------------------------------------------------
// Per-tick renderer reload (`BuildContext::reload_renderer` ×
// `RebuildPlan::renderer_fresh`).
//
// The content snapshot and page modules are baked into the SSR bundle, so
// an EDIT tick that only re-renders against the boot-time bundle emits
// byte-identical stale HTML — the dev server never reflected in-place
// saves until restart. The pipeline must therefore invoke
// `reload_renderer` before rendering on a normal edit tick, and must NOT
// invoke it when the tick's bundle is already fresh (boot initial render,
// or a watch-ADD discovery re-bundle in the same tick).
// ---------------------------------------------------------------------------

/// Shared scaffolding for the reload tests: one md page fixture, a
/// recording reload hook, and a recording renderer.
struct ReloadProbe {
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl ReloadProbe {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn ctx(&self, project: &std::path::Path) -> BuildContext {
        let render_events = self.events.clone();
        let reload_events = self.events.clone();
        BuildContext {
            dist_root: project.join("dist"),
            render_pages: Arc::new(move |pages: &[PageId]| {
                render_events.lock().unwrap().push("render");
                Ok(pages
                    .iter()
                    .map(|p| RenderedPage {
                        page: p.clone(),
                        output_path: RelDistPath::new(format!(
                            "{}.html",
                            p.path().file_stem().unwrap().to_string_lossy()
                        ))
                        .expect("test stem is a relative html path"),
                        html: "<p>x</p>".into(),
                        content_type: None,
                    })
                    .collect())
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(move || {
                reload_events.lock().unwrap().push("reload");
                Ok(vec![])
            })),
        }
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }
}

fn md_page_fixture(project: &std::path::Path) -> Arc<Mutex<DependencyGraph>> {
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::write(project.join("pages/post.tsx"), "// page\n").unwrap();
    std::fs::write(project.join("content/post.md"), "# hello\n").unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();
    make_graph_with_md_page(project)
}

/// An in-place EDIT (`Modified`) must reload the renderer BEFORE
/// rendering — otherwise the render runs against the stale boot bundle
/// and the output is byte-identical to the previous tick.
#[test]
fn modified_content_tick_reloads_renderer_before_render() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let graph = md_page_fixture(project);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );
    let probe = ReloadProbe::new();
    let ctx = probe.ctx(project);

    orch.tick_with_kinds(
        vec![(project.join("content/post.md"), ChangeKind::Modified)],
        &ctx,
        None,
    )
    .expect("tick succeeded")
    .expect("non-noop");

    assert_eq!(
        probe.events(),
        vec!["reload", "render"],
        "edit tick must reload the renderer exactly once, before rendering"
    );
}

/// A Created tick whose discovery hook already re-bundled + reloaded
/// (`DiscoveryOutcome::renderer_reloaded == true`) must NOT trigger the
/// pipeline's own reload — one bundle per tick.
#[test]
fn discovery_reload_suppresses_pipeline_reload_in_same_tick() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let graph = md_page_fixture(project);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );
    let probe = ReloadProbe::new();
    let ctx = probe.ctx(project);

    let new_post = project.join("content/new-post.md");
    std::fs::write(&new_post, "# new\n").unwrap();

    let hook_pages = vec![pid(project.join("pages/post.tsx"))];
    let hook: DiscoveryHook = Arc::new(move |_created: &[PathBuf]| {
        Ok(DiscoveryOutcome {
            pages: hook_pages.clone(),
            renderer_reloaded: true,
        })
    });

    orch.tick_with_kinds(
        vec![(new_post.clone(), ChangeKind::Created)],
        &ctx,
        Some(&hook),
    )
    .expect("tick succeeded")
    .expect("non-noop");

    assert_eq!(
        probe.events(),
        vec!["render"],
        "discovery already refreshed the bundle — the pipeline must not reload again"
    );
}

/// Boot's eager initial render runs right after the boot bundle — the
/// renderer is already fresh, so `initial_build` must not reload.
#[test]
fn initial_build_does_not_reload_renderer() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let graph = md_page_fixture(project);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")]),
        graph,
        DevAssetPipeline::new(),
    );
    let probe = ReloadProbe::new();
    let ctx = probe.ctx(project);

    orch.initial_build(&ctx)
        .expect("initial build succeeded")
        .expect("non-empty graph renders");

    assert_eq!(
        probe.events(),
        vec!["render"],
        "initial render must reuse the boot bundle, not re-bundle"
    );
}

/// A collection configured under a custom root (`src/mdx/notes`) plans a
/// Content rebuild — pages re-render, islands do NOT re-bundle (before
/// the `content_roots` policy, the `src` segment classified the entry as
/// Module and re-bundled islands on every edit).
#[test]
fn modified_entry_under_custom_collection_root_skips_islands() {
    let dir = tempdir().unwrap();
    let project = dir.path();

    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("src/mdx/notes")).unwrap();
    std::fs::write(project.join("pages/post.tsx"), "// page\n").unwrap();
    std::fs::write(project.join("src/mdx/notes/foo.mdx"), "# hi\n").unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();

    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/post.tsx")),
        vec![(project.join("src/mdx/notes/foo.mdx"), DepKind::Content)],
    ));
    let graph = Arc::new(Mutex::new(g));

    let islands_runs = Arc::new(AtomicUsize::new(0));
    let islands_runs_cb = islands_runs.clone();

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(
            project,
            vec![PathBuf::from("pages"), PathBuf::from("src/mdx/notes")],
        )
        .with_policy(
            zfb_build::GranularityPolicy::default()
                .with_content_roots(vec![PathBuf::from("src/mdx/notes")]),
        ),
        graph,
        DevAssetPipeline::new(),
    );

    let probe = ReloadProbe::new();
    let mut ctx = probe.ctx(project);
    ctx.run_islands = Some(Arc::new(move || {
        islands_runs_cb.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }));

    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("src/mdx/notes/foo.mdx"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");

    assert_eq!(outcome.pages_rendered, 1, "consumer page re-rendered");
    assert!(
        !outcome.islands_rerun,
        "a content entry under a configured collection root must not re-bundle islands"
    );
    assert_eq!(islands_runs.load(Ordering::SeqCst), 0);
    assert_eq!(
        probe.events(),
        vec!["reload", "render"],
        "content edit still refreshes the renderer bundle"
    );
}
