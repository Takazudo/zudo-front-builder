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
    AssetPipeline, BuildContext, BuildOrchestrator, DevAssetPipeline, OrchestratorConfig,
    PageSelection, RebuildPlan, RenderedPage,
};
use zfb_graph::{DepKind, DependencyGraph, PageDeps, PageId};
use zfb_watcher::Watcher;

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
                        output_path: PathBuf::from(format!("{stem}.html")),
                        html: format!("<h1>{stem}</h1>"),
                        content_type: None,
                    }
                })
                .collect())
        }),
        run_css: None,
        run_islands: None,
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
            Ok(false)
        })),
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
                    output_path: PathBuf::from(format!(
                        "{}.html",
                        p.path().file_stem().unwrap().to_string_lossy()
                    )),
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
            Ok(true)
        })),
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
    // pipeline path end-to-end. Uses a generous (200ms) debounce so
    // the test is robust against jittery filesystems on CI.
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

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project, vec![PathBuf::from("content")])
            .with_debounce(Duration::from_millis(50)),
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
                    output_path: PathBuf::from("post.html"),
                    html: "<h1>v2</h1>".into(),
                    content_type: None,
                })
                .collect())
        }),
        run_css: None,
        run_islands: None,
    };

    // Spawn watcher manually so the test can observe the channel and
    // then drive a single tick. We do NOT call `orch.run` here because
    // the test wants explicit control over when the rebuild happens.
    let (handle, mut rx) = Watcher::start_with_debounce(
        project,
        ["content"],
        Duration::from_millis(50),
    )
    .expect("watcher start");

    // Wait briefly for the watcher to install its inotify on the dir,
    // then change the file.
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::write(&md_path, "# v2\n").unwrap();

    // Wait for the change to propagate.
    let change = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("watcher emitted within 2s")
        .expect("channel still open");

    let outcome = orch
        .tick(vec![change.path], &ctx)
        .expect("tick ok")
        .expect("non-noop");
    assert_eq!(outcome.pages_rendered, 1);
    assert_eq!(outcome.pages_written.len(), 1);

    let html_path = project.join("dist/post.html");
    assert!(html_path.exists());
    assert_eq!(std::fs::read_to_string(&html_path).unwrap(), "<h1>v2</h1>");

    drop(handle);
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

#[allow(dead_code)]
fn _ensure_unused_imports_used(_: PageSelection) {}
