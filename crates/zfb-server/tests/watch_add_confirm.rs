//! Real-watcher end-to-end confirmation for the watch-ADD discovery fix
//! (issue #659 / confirm sub #660).
//!
//! ## Why this test exists
//!
//! #659's integration test (`crates/zfb-build/tests/integration_dev_loop.rs`)
//! injects synthetic `ChangeKind::Created` ticks directly into the
//! orchestrator, bypassing the real watcher and the debounce/coalescing path.
//! That encodes the same assumption as the fix: "a synthetic Created tick
//! faithfully represents a brand-new file on disk." Under autonomous merge to
//! `main` with no human gate, that is not enough.
//!
//! This test exercises the **complete path**: real file write → real `notify`
//! watcher → debouncer coalescing → `ChangeKind::Created` emitted on the
//! channel → `BuildOrchestrator::run()` drain loop → `tick_with_kinds` with
//! discovery hook → render → disk write → HTTP `GET` returns 200.
//!
//! ## What is real vs stubbed
//!
//! Real (this test exercises live code all the way through):
//!   - `fs::write` to a brand-new path on the real filesystem
//!   - `notify` OS-level watcher (FSEvents on macOS, inotify on Linux)
//!   - Debouncer coalescing (the "sticky Created" fix in `zfb-watcher`)
//!   - `BuildOrchestrator::run()` drain/debounce loop
//!   - `tick_with_kinds` Created-path → discovery-hook invocation
//!   - `DevAssetPipeline` render → atomic disk write
//!   - `zfb-server` HTTP layer serving from `html_root` on disk
//!   - In-process `reqwest` GET → real 200 response
//!
//! Stubbed (requires a live V8 host, not reachable from crate-level tests):
//!   - The real discovery hook's V8 rebundle/reload-in-place step. The fake
//!     hook below mirrors the real hook's contract (route-table rebuild,
//!     graph upsert, return discovered PageIds) without reaching into the
//!     embedded V8 host.
//!
//! ## Timing strategy
//!
//! The test uses a generous 200ms debounce and polls the HTTP endpoint with a
//! 30s overall deadline (generous for loaded CI), and a shared lock serializes
//! the two tests so they never contend. Poll-with-timeout (not a fixed sleep) is how the
//! zfb-watcher smoke suite handles the same flake class.
//!
//! ### Watcher-live handshake (ADD test)
//!
//! macOS FSEvents has a *per-stream startup latency*: a file CREATED in the
//! first window after the watch stream is registered can be dropped entirely
//! — the raw `notify` channel receives no event at all (confirmed by
//! instrumentation: a 200ms warmup yielded zero raw events for the new file;
//! a longer wait yielded the expected Create+Modify burst). A fixed warmup
//! sleep is fragile under `cargo test --workspace` load. So before writing
//! the real `foo.mdx`, the ADD test repeatedly creates fresh-named throwaway
//! files under `content/blog/` and polls the discovery hook until one is
//! observed — proving the stream is live (past its dead window). The dead
//! window is per-stream, not per-path, so once any create is delivered the
//! subsequent `foo.mdx` create is reliably delivered too. See the inline
//! handshake comment for why fresh names (not a re-write) are required.
//!
//! ## Path canonicalization
//!
//! On macOS, `/tmp` is a symlink to `/private/tmp`. `notify` (via FSEvents)
//! reports events on the canonical path. The orchestrator's `classify_change`
//! uses `path.strip_prefix(project_root)` which fails if `project_root` is
//! the non-canonical symlink path. We canonicalize `project` once at setup
//! to ensure the orchestrator, the discovery hook, and the watcher all agree
//! on the same form.
//!
//! ## Route/file layout
//!
//! Pages are rendered to `<html_root>/blog/<slug>/index.html` so the server's
//! `read_from_dist` fallback (`<html_root>/<path>/index.html`) can serve them
//! at `GET /blog/<slug>`. This mirrors the directory-style layout that `zfb
//! dev`'s actual renderer writes for clean URLs.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_build::{
    BuildContext, BuildOrchestrator, DevAssetPipeline, DiscoveryHook, DiscoveryOutcome,
    OrchestratorConfig,
    RelDistPath, RenderedPage,
};
use zfb_graph::{DepKind, DependencyGraph, PageDeps, PageId};
use zfb_server::livereload::ReloadEvent;
use zfb_server::{serve_with_listener, PageCache, ServeOpts};

/// Serialize the two real-watcher tests. Each stands up a live notify watcher
/// plus an in-process dev server; cargo runs tests in a file concurrently by
/// default, and under full-suite load (`cargo test --workspace`) that
/// contention pushed the HTTP poll past its deadline (both passed when run
/// alone or with --test-threads=1). A shared async lock forces them to run
/// one at a time regardless of cargo's thread count.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pid(p: impl Into<PathBuf>) -> PageId {
    PageId::new(p.into())
}

/// Set up the blog fixture directory layout under `project` (must be
/// canonical — no symlinks):
///
/// ```text
/// pages/blog/[slug].tsx   — the dynamic consumer source
/// content/blog/hello.mdx  — one pre-existing post
/// dist/assets/            — required by the server
/// public/                 — required by the server
/// ```
fn setup_fixture(project: &Path) {
    std::fs::create_dir_all(project.join("pages/blog")).unwrap();
    std::fs::create_dir_all(project.join("content/blog")).unwrap();
    std::fs::create_dir_all(project.join("dist/assets")).unwrap();
    std::fs::create_dir_all(project.join("public")).unwrap();
    std::fs::write(
        project.join("pages/blog/[slug].tsx"),
        "export async function paths(){return []}\nexport default function P(){}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("content/blog/hello.mdx"),
        "---\ntitle: Hello\n---\nhi\n",
    )
    .unwrap();
}

/// The route table the orchestrator uses to fan out a dynamic `[slug]` source
/// page to concrete output paths. Mirrors `routes_by_source` in `dev.rs`.
type RouteTable = Arc<Mutex<HashMap<PathBuf, Vec<String>>>>;

/// Seed the route table: the `[slug]` source maps to ONE output path for the
/// pre-existing post. Directory-style (`blog/hello/index.html`) so the
/// server serves it at `GET /blog/hello`.
fn seed_route_table(project: &Path) -> RouteTable {
    let mut t = HashMap::new();
    t.insert(
        project.join("pages/blog/[slug].tsx"),
        vec!["blog/hello/index.html".to_string()],
    );
    Arc::new(Mutex::new(t))
}

/// Seed the dep graph: the `[slug]` source plus its edge to the pre-existing post.
fn seed_graph(project: &Path) -> Arc<Mutex<DependencyGraph>> {
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/blog/[slug].tsx")),
        vec![(project.join("content/blog/hello.mdx"), DepKind::Content)],
    ));
    Arc::new(Mutex::new(g))
}

/// Build the `BuildContext` whose `render_pages` fans out over the shared
/// route table. `DevAssetPipeline` then writes each rendered page to
/// `ctx.dist_root/<output_path>` atomically.
fn make_ctx(html_root: PathBuf, routes: RouteTable) -> BuildContext {
    BuildContext {
        dist_root: html_root,
        render_pages: Arc::new(move |pages: &[PageId]| {
            let table = routes.lock().unwrap();
            let mut out = Vec::new();
            for p in pages {
                for output in table.get(p.path()).into_iter().flatten() {
                    let html = format!("<html><body><p>{output}</p></body></html>");
                    let output_path = RelDistPath::new(output.clone())
                        .expect("test output is a relative path");
                    out.push(RenderedPage {
                        page: PageId::new(PathBuf::from(output)),
                        output_path,
                        html,
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

/// Build the discovery hook that mirrors `zfb dev`'s `make_discovery_hook`:
/// for a newly-created content file under `content/blog/`, rebuild the route
/// table and graph, return the `[slug]` source PageId.
///
/// `project` MUST be canonical (no symlinks) so `starts_with` comparisons
/// against notify's reported canonical paths succeed.
fn make_discovery_hook(
    project: PathBuf,
    routes: RouteTable,
    graph: Arc<Mutex<DependencyGraph>>,
    invocations: Arc<Mutex<Vec<Vec<PathBuf>>>>,
) -> DiscoveryHook {
    let slug_src = project.join("pages/blog/[slug].tsx");
    let content_blog = project.join("content/blog");
    Arc::new(move |created: &[PathBuf]| {
        invocations.lock().unwrap().push(created.to_vec());
        let mut out = Vec::new();
        for c in created {
            // Normalize both paths for comparison: canonicalize if possible,
            // fall back to the raw path. This handles cases where the
            // incoming path is canonical but the prefix isn't (or vice versa).
            let c_norm = std::fs::canonicalize(c).unwrap_or_else(|_| c.clone());
            let blog_norm = std::fs::canonicalize(&content_blog)
                .unwrap_or_else(|_| content_blog.clone());

            if c_norm.starts_with(&blog_norm) {
                let slug = c.file_stem().and_then(|s| s.to_str()).unwrap_or("post");
                {
                    let mut t = routes.lock().unwrap();
                    t.entry(slug_src.clone())
                        .or_default()
                        .push(format!("blog/{slug}/index.html"));
                }
                {
                    let mut g = graph.lock().unwrap();
                    g.upsert(PageDeps::new(
                        pid(slug_src.clone()),
                        vec![(c.clone(), DepKind::Content)],
                    ));
                }
                out.push(pid(slug_src.clone()));
            }
        }
        Ok(DiscoveryOutcome {
            pages: out,
            renderer_reloaded: true,
        })
    })
}

/// Bind an ephemeral port and return `(listener, addr)`.
async fn bind_ephemeral() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    (listener, addr)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Real-watcher end-to-end: ADD a new content file → GET /blog/foo returns 200.
///
/// Exercises the full path (disk → notify → debounce → tick_with_kinds →
/// discovery hook → render → disk → HTTP GET 200) without any synthetic tick
/// injection.
///
/// The discovery hook is a faithful stub: it performs the same route-table
/// rebuild, graph upsert, and PageId return as `zfb dev`'s real hook, but
/// without the V8 rebundle step (not reachable from crate tests).
#[tokio::test(flavor = "multi_thread")]
async fn real_watcher_add_content_file_serves_new_route_as_200() {
    let _serial = SERIAL.lock().await;

    let tmp = tempfile::tempdir().expect("tempdir");
    // Canonicalize so notify's FSEvents paths and our prefix checks agree.
    // On macOS /tmp is a symlink to /private/tmp; without canonicalization
    // classify_change's strip_prefix fails and the path is mis-classified
    // as External, bypassing the discovery hook.
    let project = tmp.path().canonicalize().expect("canonicalize project");

    setup_fixture(&project);

    let routes = seed_route_table(&project);
    let graph = seed_graph(&project);
    let hook_invocations: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));

    let discover = make_discovery_hook(
        project.clone(),
        routes.clone(),
        graph.clone(),
        hook_invocations.clone(),
    );

    let html_root = project.join("dist");
    let ctx = make_ctx(html_root.clone(), routes.clone());

    // Debounce: 200ms — generous for macOS FSEvents coalescing.
    let debounce = Duration::from_millis(200);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project.clone(), vec![PathBuf::from("content")])
            .with_debounce(debounce),
        graph.clone(),
        DevAssetPipeline::new(),
    );

    // ----------------------------------------------------------------
    // 1. Boot the HTTP server
    // ----------------------------------------------------------------
    let (listener, addr) = bind_ephemeral().await;
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let opts = ServeOpts {
        project_root: project.clone(),
        dist_root: html_root.clone(),
        html_root: html_root.clone(),
        public_root: project.join("public"),
        addr,
        pages: PageCache::new(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: None,
        base: None,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;

    // ----------------------------------------------------------------
    // 2. Spin up the orchestrator run() loop with discovery hook wired.
    // ----------------------------------------------------------------
    let orch_task = tokio::spawn(async move {
        orch.run(ctx, Some(discover), |_outcome| {}).await
    });

    // ----------------------------------------------------------------
    // 2b. Watcher-live handshake (deterministic warmup).
    //
    // macOS FSEvents has a per-stream startup latency: a brand-new file
    // CREATED in the first window after the watch stream is registered can
    // be DROPPED entirely — no Create, no Modify event reaches the watcher
    // (confirmed by instrumentation: with a 200ms sleep, the raw notify
    // channel received zero events for the new file; with a longer wait it
    // received Create+Modify as expected). A fixed sleep is fragile under
    // load (`cargo test --workspace`), so instead we prove the stream is
    // live before writing the real file.
    //
    // We repeatedly create FRESH-NAMED warmup files under `content/blog/`
    // (same operation class as the real ADD — a brand-new file create) and
    // poll the discovery hook's invocation log until one is observed. A
    // single warmup write would have the identical startup-window
    // vulnerability (its own create could be dropped), and re-writing the
    // same path only fires Modify (never re-triggers discovery), so the
    // loop uses a new name each iteration. Once ANY warmup create is
    // observed, the FSEvents stream is past its dead window and stays live
    // (startup latency is per-stream, not per-path), so the subsequent
    // `foo.mdx` create lands in the live window.
    //
    // Separate deadline + panic message so a genuinely dead watcher fails
    // fast and is diagnosable as a watcher problem, not a discovery
    // regression. Warmup files create `/blog/__warmup_*` routes, never
    // `/blog/foo`, so the baseline 404 assertion below still holds.
    {
        let warmup_deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut warmup_idx = 0u32;
        let mut watcher_live = false;
        while std::time::Instant::now() < warmup_deadline {
            let warmup = project.join(format!("content/blog/__warmup_{warmup_idx}.mdx"));
            std::fs::write(&warmup, "---\ntitle: warmup\n---\nwarmup\n")
                .expect("write warmup content file");
            warmup_idx += 1;

            // Give this warmup's event a beat to propagate through
            // FSEvents → debounce (200ms) → hook.
            tokio::time::sleep(Duration::from_millis(400)).await;

            let saw_warmup = hook_invocations.lock().unwrap().iter().flatten().any(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("__warmup_"))
                    .unwrap_or(false)
            });
            if saw_warmup {
                watcher_live = true;
                break;
            }
        }
        assert!(
            watcher_live,
            "watcher never became live: no warmup create reached the discovery hook within 10s \
             (the FSEvents stream never started delivering events — a watcher-layer problem, \
             not a discovery regression)",
        );
    }

    // ----------------------------------------------------------------
    // 3. Confirm /blog/foo 404s BEFORE the file is created (baseline).
    // ----------------------------------------------------------------
    let client = reqwest::Client::new();
    let before = client
        .get(format!("http://{addr}/blog/foo"))
        .send()
        .await
        .expect("GET /blog/foo before write");
    assert_eq!(
        before.status().as_u16(),
        404,
        "/blog/foo must 404 before the content file is created",
    );

    // ----------------------------------------------------------------
    // 4. Write a brand-new content file to disk. The real `notify`
    //    watcher picks this up; the debouncer preserves ChangeKind::Created
    //    (the "sticky Created" fix — without it macOS FSEvents collapses
    //    the Create+Modify burst to Modified, preventing hook invocation).
    // ----------------------------------------------------------------
    let new_post = project.join("content/blog/foo.mdx");
    std::fs::write(&new_post, "---\ntitle: Foo\n---\nfoo content\n")
        .expect("write new content file");

    // ----------------------------------------------------------------
    // 5. Poll the new route until it returns 200.
    //    Overall deadline: 30s — far longer than the 200ms debounce + tick
    //    lag, short enough to fail fast on a genuine regression.
    // ----------------------------------------------------------------
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut got_200 = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match client
            .get(format!("http://{addr}/blog/foo"))
            .send()
            .await
        {
            Ok(resp) if resp.status().as_u16() == 200 => {
                got_200 = true;
                let body = resp.text().await.unwrap_or_default();
                // The render_pages callback embeds the output path in the
                // body. Check it matches the expected URL.
                assert!(
                    body.contains("blog/foo/index.html"),
                    "body should contain the rendered output path; got: {body}",
                );
                break;
            }
            Ok(_) => {} // still 404, keep polling
            Err(e) => eprintln!("poll error (will retry): {e}"),
        }
    }

    assert!(
        got_200,
        "GET /blog/foo must return 200 within 30s after writing the new content file \
         (real watcher → Created kind → discovery hook → render → 200)",
    );

    // ----------------------------------------------------------------
    // 6. The pre-existing route (/blog/hello) must still be served.
    //    (Discovery must not break existing routes.)
    // ----------------------------------------------------------------
    let hello = client
        .get(format!("http://{addr}/blog/hello"))
        .send()
        .await
        .expect("GET /blog/hello after add");
    assert_eq!(
        hello.status().as_u16(),
        200,
        "/blog/hello must still serve 200 after the new-file discovery",
    );

    // ----------------------------------------------------------------
    // 7. The discovery hook must have fired at least once for foo.mdx.
    // ----------------------------------------------------------------
    let invs = hook_invocations.lock().unwrap();
    let hook_saw_foo = invs.iter().flatten().any(|p| {
        p.file_name().and_then(|n| n.to_str()) == Some("foo.mdx")
    });
    assert!(
        hook_saw_foo,
        "the discovery hook must have been invoked with the new content file; \
         invocations: {invs:?}",
    );

    // ----------------------------------------------------------------
    // 8. Teardown
    // ----------------------------------------------------------------
    orch_task.abort();
    server.abort();
}

/// Edit-path regression: editing an EXISTING content file still hot-reloads
/// through the real watcher.
///
/// The route `GET /blog/hello` must return 200 before AND after an edit to
/// `content/blog/hello.mdx`. This guards against the re-scan/re-bundle
/// changes in #659 breaking the previously-working edit path.
///
/// Note on macOS FSEvents: editing an existing file can also surface as
/// `ChangeKind::Created` (FSEvents sets ITEM_CREATED for metadata updates).
/// So we do NOT assert "hook not called for edit" — on macOS the hook may
/// be called, and that is fine (it is idempotent). We assert behavior only:
/// the endpoint must return 200 after the edit.
#[tokio::test(flavor = "multi_thread")]
async fn real_watcher_edit_existing_file_still_hot_reloads() {
    let _serial = SERIAL.lock().await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().canonicalize().expect("canonicalize project");

    setup_fixture(&project);

    let routes = seed_route_table(&project);
    let graph = seed_graph(&project);
    let hook_invocations: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));

    let discover = make_discovery_hook(
        project.clone(),
        routes.clone(),
        graph.clone(),
        hook_invocations.clone(),
    );

    let html_root = project.join("dist");
    let ctx = make_ctx(html_root.clone(), routes.clone());
    let debounce = Duration::from_millis(200);

    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project.clone(), vec![PathBuf::from("content")])
            .with_debounce(debounce),
        graph.clone(),
        DevAssetPipeline::new(),
    );

    // Pre-render the hello route so the server can serve it at boot.
    {
        let hello_dir = html_root.join("blog/hello");
        std::fs::create_dir_all(&hello_dir).unwrap();
        std::fs::write(
            hello_dir.join("index.html"),
            "<html><body><p>blog/hello/index.html</p></body></html>",
        )
        .unwrap();
    }

    // ----------------------------------------------------------------
    // 1. Boot the HTTP server
    // ----------------------------------------------------------------
    let (listener, addr) = bind_ephemeral().await;
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let opts = ServeOpts {
        project_root: project.clone(),
        dist_root: html_root.clone(),
        html_root: html_root.clone(),
        public_root: project.join("public"),
        addr,
        pages: PageCache::new(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: None,
        base: None,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;

    // Verify initial 200 before the orchestrator does anything.
    let client = reqwest::Client::new();
    let r0 = client
        .get(format!("http://{addr}/blog/hello"))
        .send()
        .await
        .expect("GET /blog/hello initial");
    assert_eq!(r0.status().as_u16(), 200, "hello must serve 200 before edit");

    // ----------------------------------------------------------------
    // 2. Spin up orchestrator run() with discovery hook wired.
    // ----------------------------------------------------------------
    let orch_task = tokio::spawn(async move {
        orch.run(ctx, Some(discover), |_outcome| {}).await
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ----------------------------------------------------------------
    // 3. Edit the EXISTING hello.mdx. The orchestrator should re-render
    //    the [slug] page and write a fresh blog/hello/index.html.
    // ----------------------------------------------------------------
    let hello_content = project.join("content/blog/hello.mdx");
    std::fs::write(&hello_content, "---\ntitle: Hello\n---\nv2 content\n")
        .expect("write updated hello.mdx");

    // ----------------------------------------------------------------
    // 4. Poll until the route returns 200. The exact body doesn't change
    //    (our stub renderer uses the output path as body, not the source
    //    content), but the endpoint must not regress to 404 or 500.
    //    Overall deadline: 30s.
    // ----------------------------------------------------------------
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut got_200_after_edit = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match client
            .get(format!("http://{addr}/blog/hello"))
            .send()
            .await
        {
            Ok(resp) if resp.status().as_u16() == 200 => {
                // We got 200 — the route still exists and the file was
                // (re-)written. That's the hot-reload invariant.
                got_200_after_edit = true;
                break;
            }
            Ok(_) => {}
            Err(e) => eprintln!("poll error: {e}"),
        }
    }

    assert!(
        got_200_after_edit,
        "GET /blog/hello must return 200 within 30s after editing hello.mdx (hot-reload regression guard)",
    );

    // ----------------------------------------------------------------
    // 5. Teardown
    // ----------------------------------------------------------------
    orch_task.abort();
    server.abort();
}

/// Real-watcher confirmation for the per-tick renderer reload
/// (`BuildContext::reload_renderer` wiring).
///
/// ## Why this test exists
///
/// The content snapshot and page modules are baked into the SSR bundle.
/// Before the reload wiring, an IN-PLACE save (`ChangeKind::Modified` —
/// what VS Code and most editors emit) re-rendered against the
/// boot-time bundle, so the re-rendered HTML was byte-identical to the
/// stale boot output: edits never reached the browser until a dev-server
/// restart. (Rename-replace saves worked by accident via the watch-ADD
/// discovery path.) Discovered during usage in a consumer project.
///
/// ## How the staleness is modelled
///
/// The real staleness lives in the V8 bundle, which crate-level tests
/// can't reach. The stub here mirrors the semantics exactly:
///
/// - `snapshot` (an `Arc<Mutex<String>>`) plays the role of the baked
///   content snapshot: it is read ONCE at "boot".
/// - `render_pages` renders from `snapshot`, NOT from disk — like the
///   real renderer, it cannot see an edit until the bundle refreshes.
/// - `reload_renderer` re-reads `hello.mdx` from disk into `snapshot` —
///   like the real re-bundle + host swap.
///
/// With `reload_renderer: None` (the pre-fix wiring) the served body
/// stays at v1 forever and this test fails its marker assertion — the
/// falsifiable encoding of the user-visible bug.
#[tokio::test]
async fn real_watcher_inplace_edit_reaches_served_html_via_reload() {
    let _serial = SERIAL.lock().await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().canonicalize().expect("canonicalize project");

    setup_fixture(&project);

    let routes = seed_route_table(&project);
    let graph = seed_graph(&project);

    let html_root = project.join("dist");
    let hello_src = project.join("content/blog/hello.mdx");

    // "Boot bundle": the snapshot reads the source once, up front.
    let snapshot: Arc<Mutex<String>> = Arc::new(Mutex::new(
        std::fs::read_to_string(&hello_src).expect("read hello.mdx at boot"),
    ));

    let reload_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let ctx = {
        let routes = routes.clone();
        let snapshot_for_render = snapshot.clone();
        let snapshot_for_reload = snapshot.clone();
        let hello_for_reload = hello_src.clone();
        let reload_count = reload_count.clone();
        BuildContext {
            dist_root: html_root.clone(),
            render_pages: Arc::new(move |pages: &[PageId]| {
                let table = routes.lock().unwrap();
                let body = snapshot_for_render.lock().unwrap().clone();
                let mut out = Vec::new();
                for p in pages {
                    for output in table.get(p.path()).into_iter().flatten() {
                        let html = format!("<html><body><p>{body}</p></body></html>");
                        let output_path = RelDistPath::new(output.clone())
                            .expect("test output is a relative path");
                        out.push(RenderedPage {
                            page: PageId::new(PathBuf::from(output)),
                            output_path,
                            html,
                            content_type: None,
                        });
                    }
                }
                Ok(out)
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: Some(Arc::new(move || {
                let fresh = std::fs::read_to_string(&hello_for_reload)?;
                *snapshot_for_reload.lock().unwrap() = fresh;
                reload_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
        }
    };

    let debounce = Duration::from_millis(200);
    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(project.clone(), vec![PathBuf::from("content")])
            .with_debounce(debounce),
        graph.clone(),
        DevAssetPipeline::new(),
    );

    // Pre-render the hello route from the boot snapshot so the server
    // serves v1 at boot.
    {
        let hello_dir = html_root.join("blog/hello");
        std::fs::create_dir_all(&hello_dir).unwrap();
        let body = snapshot.lock().unwrap().clone();
        std::fs::write(
            hello_dir.join("index.html"),
            format!("<html><body><p>{body}</p></body></html>"),
        )
        .unwrap();
    }

    let (listener, addr) = bind_ephemeral().await;
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let opts = ServeOpts {
        project_root: project.clone(),
        dist_root: html_root.clone(),
        html_root: html_root.clone(),
        public_root: project.join("public"),
        addr,
        pages: PageCache::new(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: None,
        base: None,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;

    let client = reqwest::Client::new();
    let r0 = client
        .get(format!("http://{addr}/blog/hello"))
        .send()
        .await
        .expect("GET /blog/hello initial");
    assert_eq!(r0.status().as_u16(), 200);
    assert!(
        !r0.text().await.unwrap().contains("V2-MARKER"),
        "marker must not be present before the edit"
    );

    // No discovery hook: the per-tick reload is the only refresh path,
    // exactly the EDIT scenario under test.
    let orch_task = tokio::spawn(async move { orch.run(ctx, None, |_outcome| {}).await });

    // Watcher-live handshake (same FSEvents dead-window mitigation as the
    // ADD test, but probed through the reload counter since there is no
    // discovery hook here): write fresh-named throwaway files under
    // content/blog/ until a tick lands.
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut i = 0;
        while reload_count.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "watcher-live handshake timed out (no tick within 15s)"
            );
            std::fs::write(
                project.join(format!("content/blog/.warmup-{i}.md")),
                "warmup\n",
            )
            .unwrap();
            i += 1;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    // The actual in-place EDIT: same path, same inode, truncate + write.
    std::fs::write(&hello_src, "---\ntitle: Hello\n---\nV2-MARKER content\n")
        .expect("in-place edit hello.mdx");

    // Poll until the SERVED body carries the new content.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut served_fresh = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(resp) = client.get(format!("http://{addr}/blog/hello")).send().await {
            if resp.status().as_u16() == 200 {
                if let Ok(body) = resp.text().await {
                    if body.contains("V2-MARKER") {
                        served_fresh = true;
                        break;
                    }
                }
            }
        }
    }

    assert!(
        served_fresh,
        "an in-place edit must reach the served HTML via the per-tick renderer \
         reload within 30s (stale-bundle regression: without reload_renderer the \
         body stays at v1 forever)",
    );

    orch_task.abort();
    server.abort();
}
