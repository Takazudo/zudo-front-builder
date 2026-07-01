//! Orchestrator/pipeline-layer tick-class matrix for the lazy dev-render
//! staleness model (issue #1025).
//!
//! These tests drive `BuildOrchestrator::tick_with_kinds` with a
//! `FakeLazySession` — an in-test stand-in for the zfb crate's dev render
//! session running with the lazy switch ON. The fake implements the same
//! contract `lazy_render_tick` pins at the unit level
//! (`crates/zfb/src/commands/dev.rs`, `mod stale_model`):
//!
//! - render callback: with a content-narrowing hint, the edited entry's
//!   own routes render eagerly and the remainder of the selected fan-out
//!   is marked stale (slug-less routes — the S1 statics / S2 aggregates —
//!   are never eager); without a hint (`.tsx`-style ticks) EVERYTHING
//!   selected is marked stale. The stale-state commit happens strictly
//!   before the callback returns.
//! - reloader: a full refresh swaps the route tables, bumps the tick
//!   generation, and evicts vanished routes from the stale set (the P4
//!   hook); a Phase-B `Skipped` outcome touches nothing.
//! - stale probe: `DevAssetPipeline::with_stale_probe` drains the tick's
//!   stale buffer into `BuildOutcome::pages_stale`.
//!
//! What is REAL here: the orchestrator's plan fold + hint production, the
//! dev pipeline's reload/render/write/prune sequencing, and the
//! `pages_stale` plumbing. What is faked: the V8 renderer and the route
//! tables.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tempfile::tempdir;
use zfb_build::{
    BuildContext, BuildOrchestrator, DevAssetPipeline, OrchestratorConfig, RefreshOutcome,
    RelDistPath, RenderedPage,
};
use zfb_graph::{DepKind, DependencyGraph, PageDeps, PageId};
use zfb_watcher::ChangeKind;

fn pid(p: PathBuf) -> PageId {
    PageId::new(p)
}

/// One fake route: relative output path + optional slug provenance.
/// `slug: None` models the S1 statics / S2 aggregates (tag indexes,
/// paginated lists) whose `paths()` params are not slug-shaped — they
/// can never be eager.
#[derive(Clone)]
struct FakeRoute {
    output: PathBuf,
    slug: Option<String>,
}

fn route(output: &str, slug: Option<&str>) -> FakeRoute {
    FakeRoute {
        output: PathBuf::from(output),
        slug: slug.map(str::to_string),
    }
}

#[derive(Default)]
struct FakeStale {
    /// Monotonic tick generation (bumped at the fake table swap).
    generation: u64,
    /// Stale output path → generation that staled it.
    entries: HashMap<PathBuf, u64>,
    /// Routes staled by the current tick (drained by the probe).
    tick: Vec<PathBuf>,
}

/// See the module docs — the dev session's staleness contract, faked.
struct FakeLazySession {
    routes: Mutex<HashMap<PathBuf, Vec<FakeRoute>>>,
    stale: Mutex<FakeStale>,
    events: Mutex<Vec<&'static str>>,
}

impl FakeLazySession {
    fn new(routes: HashMap<PathBuf, Vec<FakeRoute>>) -> Arc<Self> {
        Arc::new(Self {
            routes: Mutex::new(routes),
            stale: Mutex::new(FakeStale::default()),
            events: Mutex::new(Vec::new()),
        })
    }

    fn mark_stale(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let mut stale = self.stale.lock().unwrap();
        let generation = stale.generation;
        for path in paths {
            stale.entries.insert(path.clone(), generation);
            stale.tick.push(path);
        }
    }

    /// The P4 hook: table swap bumps the generation and evicts vanished
    /// routes from the stale set + tick buffer.
    fn note_table_swap(&self, vanished_rel: &[PathBuf]) {
        let mut stale = self.stale.lock().unwrap();
        stale.generation += 1;
        for v in vanished_rel {
            stale.entries.remove(v);
        }
        stale.tick.retain(|p| !vanished_rel.contains(p));
    }

    fn take_tick_stale(&self) -> Vec<PathBuf> {
        let mut drained = std::mem::take(&mut self.stale.lock().unwrap().tick);
        drained.sort();
        drained.dedup();
        drained
    }

    fn is_stale(&self, path: &str) -> bool {
        self.stale
            .lock()
            .unwrap()
            .entries
            .contains_key(Path::new(path))
    }

    fn generation(&self) -> u64 {
        self.stale.lock().unwrap().generation
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }
}

/// The canonical three-source fixture:
///
/// - `pages/blog.tsx` — slug-shaped dynamic source (own routes
///   `/blog/a`, `/blog/b`),
/// - `pages/tags.tsx` — S2-style aggregate (tag index; never eager),
/// - `pages/index.tsx` — S1-style static (never eager),
///
/// all consuming `content/a.md`.
fn matrix_fixture(project: &Path) -> (Arc<Mutex<DependencyGraph>>, Arc<FakeLazySession>) {
    std::fs::create_dir_all(project.join("pages")).unwrap();
    std::fs::create_dir_all(project.join("content")).unwrap();
    std::fs::create_dir_all(project.join("dist")).unwrap();
    std::fs::write(project.join("pages/blog.tsx"), "// blog\n").unwrap();
    std::fs::write(project.join("pages/tags.tsx"), "// tags\n").unwrap();
    std::fs::write(project.join("pages/index.tsx"), "// index\n").unwrap();
    std::fs::write(project.join("content/a.md"), "---\ntitle: A\n---\n\nv1\n").unwrap();

    let md = project.join("content/a.md");
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/blog.tsx")),
        vec![(md.clone(), DepKind::Content)],
    ));
    g.upsert(PageDeps::new(
        pid(project.join("pages/tags.tsx")),
        vec![(md.clone(), DepKind::Content)],
    ));
    g.upsert(PageDeps::new(
        pid(project.join("pages/index.tsx")),
        vec![(md, DepKind::Content)],
    ));

    let mut routes = HashMap::new();
    routes.insert(
        project.join("pages/blog.tsx"),
        vec![
            route("blog/a/index.html", Some("a")),
            route("blog/b/index.html", Some("b")),
        ],
    );
    routes.insert(
        project.join("pages/tags.tsx"),
        vec![route("tags/rust/index.html", None)],
    );
    routes.insert(
        project.join("pages/index.tsx"),
        vec![route("index.html", None)],
    );

    (Arc::new(Mutex::new(g)), FakeLazySession::new(routes))
}

fn orchestrator_for(
    project: &Path,
    graph: Arc<Mutex<DependencyGraph>>,
    session: &Arc<FakeLazySession>,
) -> BuildOrchestrator<DevAssetPipeline> {
    let probe_session = session.clone();
    BuildOrchestrator::new(
        OrchestratorConfig::new(
            project,
            vec![PathBuf::from("pages"), PathBuf::from("content")],
        ),
        graph,
        DevAssetPipeline::with_stale_probe(Arc::new(move || probe_session.take_tick_stale())),
    )
}

/// Build a tick context around the fake session: the lazy render
/// callback (eager own routes + stale remainder), a reloader produced
/// by `reload`, and `html` as the rendered body (vary it across ticks
/// to defeat the byte-dedup cache where a re-write is expected).
fn lazy_ctx(
    project: &Path,
    session: &Arc<FakeLazySession>,
    html: &str,
    reload: impl Fn(&FakeLazySession) -> RefreshOutcome + Send + Sync + 'static,
) -> BuildContext {
    let render_session = session.clone();
    let html = html.to_string();
    let reload_session = session.clone();
    BuildContext {
        dist_root: project.join("dist"),
        render_pages: Arc::new(move |pages: &[PageId], narrowing| {
            // Eager = the edited entries' own routes, identified by the
            // changed content files' stems (the fake's stand-in for the
            // real slug-candidate machinery). No hint → nothing eager.
            let eager_slugs: BTreeSet<String> = narrowing
                .map(|n| {
                    n.changed_content
                        .iter()
                        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            let mut out = Vec::new();
            let mut stale: Vec<PathBuf> = Vec::new();
            {
                let routes = render_session.routes.lock().unwrap();
                for page in pages {
                    let Some(entries) = routes.get(page.path()) else {
                        continue;
                    };
                    for r in entries {
                        let eager = r.slug.as_ref().is_some_and(|s| eager_slugs.contains(s));
                        if eager {
                            out.push(RenderedPage {
                                page: PageId::new(r.output.clone()),
                                output_path: RelDistPath::new(r.output.clone())
                                    .expect("fixture paths are relative"),
                                html: format!("<p>{html}</p>"),
                                content_type: None,
                            });
                        } else {
                            stale.push(r.output.clone());
                        }
                    }
                }
            }
            // Stale-state commit strictly BEFORE returning — mirrors
            // `lazy_render_tick`'s ordering contract.
            render_session.mark_stale(stale);
            render_session
                .events
                .lock()
                .unwrap()
                .push("render:stale-marked");
            Ok(out)
        }),
        run_css: None,
        run_islands: None,
        run_client_scripts: None,
        reload_renderer: Some(Arc::new(move || Ok(reload(&reload_session)))),
    }
}

/// A full refresh with nothing vanished: swap tables (generation bump),
/// report `Refreshed`.
fn plain_refresh(session: &FakeLazySession) -> RefreshOutcome {
    session.note_table_swap(&[]);
    session.events.lock().unwrap().push("reload:tables-swapped");
    RefreshOutcome::Refreshed {
        vanished: vec![],
        changed_sources: vec![],
    }
}

/// Body edit tick class: the edited entry's own route renders eagerly;
/// the sibling route, the S2 aggregate (tag index), and the S1 static
/// are marked stale and NOT rendered. `pages_stale` reports exactly the
/// stale remainder.
#[test]
fn body_edit_renders_own_route_and_stales_aggregates() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let (graph, session) = matrix_fixture(project);
    let orch = orchestrator_for(project, graph, &session);
    let ctx = lazy_ctx(project, &session, "v1", plain_refresh);

    std::fs::write(
        project.join("content/a.md"),
        "---\ntitle: A\n---\n\nv2 body\n",
    )
    .unwrap();
    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("content/a.md"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");

    // Eager own route: rendered + written to dist.
    assert_eq!(outcome.pages_rendered, 1, "exactly the own route renders");
    assert_eq!(
        outcome.pages_written,
        vec![pid(PathBuf::from("blog/a/index.html"))]
    );
    assert_eq!(
        std::fs::read_to_string(project.join("dist/blog/a/index.html")).unwrap(),
        "<p>v1</p>"
    );

    // Stale remainder: marked, NOT rendered — the S2 aggregate must not
    // reach disk.
    assert_eq!(
        outcome.pages_stale,
        vec![
            PathBuf::from("blog/b/index.html"),
            PathBuf::from("index.html"),
            PathBuf::from("tags/rust/index.html"),
        ],
        "the stale remainder (sibling + S1 static + S2 aggregate) is announced"
    );
    assert!(
        !project.join("dist/tags/rust/index.html").exists(),
        "the S2 aggregate route must be stale-NOT-rendered"
    );
    assert!(!project.join("dist/blog/b/index.html").exists());
    assert!(!project.join("dist/index.html").exists());
    assert!(session.is_stale("tags/rust/index.html"));
    assert!(session.is_stale("blog/b/index.html"));
    assert!(session.is_stale("index.html"));
    assert!(
        !session.is_stale("blog/a/index.html"),
        "the eagerly-rendered own route is fresh"
    );

    // Nothing pruned — stale routes are not vanished routes.
    assert!(outcome.pages_pruned.is_empty());
}

/// Frontmatter edit tick class: same split as a body edit — the own
/// route still renders eagerly, the remainder goes stale. (The G4
/// frontmatter gate of the eager-narrowing path does NOT demote the
/// own route in lazy mode; the cross-page fallout is covered by the
/// stale remainder.)
#[test]
fn frontmatter_edit_renders_own_route_and_stales_aggregates() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let (graph, session) = matrix_fixture(project);
    let orch = orchestrator_for(project, graph, &session);

    // Tick 1 — body edit (seeds dist + the stale set).
    let ctx1 = lazy_ctx(project, &session, "v1", plain_refresh);
    std::fs::write(project.join("content/a.md"), "---\ntitle: A\n---\n\nv2\n").unwrap();
    orch.tick_with_kinds(
        vec![(project.join("content/a.md"), ChangeKind::Modified)],
        &ctx1,
        None,
    )
    .expect("tick succeeded")
    .expect("non-noop");

    // Tick 2 — frontmatter edit of the same entry.
    let ctx2 = lazy_ctx(project, &session, "v2", plain_refresh);
    std::fs::write(
        project.join("content/a.md"),
        "---\ntitle: A (renamed)\n---\n\nv2\n",
    )
    .unwrap();
    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("content/a.md"), ChangeKind::Modified)],
            &ctx2,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");

    assert_eq!(outcome.pages_rendered, 1, "own route still renders eagerly");
    assert_eq!(
        std::fs::read_to_string(project.join("dist/blog/a/index.html")).unwrap(),
        "<p>v2</p>",
        "the own route reflects the frontmatter tick's render"
    );
    assert_eq!(
        outcome.pages_stale,
        vec![
            PathBuf::from("blog/b/index.html"),
            PathBuf::from("index.html"),
            PathBuf::from("tags/rust/index.html"),
        ],
    );
    assert!(
        !project.join("dist/tags/rust/index.html").exists(),
        "the aggregate stays stale-NOT-rendered on a frontmatter tick too"
    );
}

/// `.tsx`-style (non-Content) tick class: no narrowing hint reaches the
/// callback, so NOTHING renders eagerly — every selected route is
/// marked stale.
#[test]
fn tsx_tick_marks_all_selected_routes_stale() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let (graph, session) = matrix_fixture(project);
    let orch = orchestrator_for(project, graph, &session);
    let ctx = lazy_ctx(project, &session, "v1", plain_refresh);

    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("pages/blog.tsx"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");

    assert_eq!(
        outcome.pages_rendered, 0,
        "an all-lazy tick renders nothing"
    );
    assert!(outcome.pages_written.is_empty());
    assert_eq!(
        outcome.pages_stale,
        vec![
            PathBuf::from("blog/a/index.html"),
            PathBuf::from("blog/b/index.html"),
        ],
        "ALL of the edited source's routes go stale — own routes included"
    );
    assert!(session.is_stale("blog/a/index.html"));
    assert!(session.is_stale("blog/b/index.html"));
    assert!(!project.join("dist/blog/a/index.html").exists());
}

/// Prune integration (#804): a route that vanishes in the tick's table
/// rebuild is EVICTED from the stale set — on an all-lazy tick with
/// zero eager renders — and its on-disk artifact is pruned. The
/// vanished route is never announced via `pages_stale`.
#[test]
fn vanished_route_is_evicted_from_stale_set_and_pruned() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let (graph, session) = matrix_fixture(project);
    let orch = orchestrator_for(project, graph, &session);

    // Tick 1 (.tsx — all-lazy): blog/a + blog/b go stale.
    let ctx1 = lazy_ctx(project, &session, "v1", plain_refresh);
    orch.tick_with_kinds(
        vec![(project.join("pages/blog.tsx"), ChangeKind::Modified)],
        &ctx1,
        None,
    )
    .expect("tick succeeded")
    .expect("non-noop");
    assert!(session.is_stale("blog/b/index.html"));

    // A stale artifact from an earlier life sits on disk.
    std::fs::create_dir_all(project.join("dist/blog/b")).unwrap();
    std::fs::write(project.join("dist/blog/b/index.html"), "<p>old</p>").unwrap();

    // Tick 2 (.tsx, all-lazy again): the refresh drops /blog/b from the
    // route table — vanish + evict at the fake P4, prune via the
    // reloader's vanished contract.
    let vanished_abs = project.join("dist/blog/b/index.html");
    let blog_source = project.join("pages/blog.tsx");
    let ctx2 = lazy_ctx(project, &session, "v1", move |sess| {
        if let Some(entries) = sess.routes.lock().unwrap().get_mut(&blog_source) {
            entries.retain(|r| r.output != Path::new("blog/b/index.html"));
        }
        sess.note_table_swap(&[PathBuf::from("blog/b/index.html")]);
        sess.events.lock().unwrap().push("reload:tables-swapped");
        RefreshOutcome::Refreshed {
            vanished: vec![vanished_abs.clone()],
            changed_sources: vec![],
        }
    });
    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("pages/blog.tsx"), ChangeKind::Modified)],
            &ctx2,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");

    assert!(
        !session.is_stale("blog/b/index.html"),
        "the vanished route must be evicted from the stale set"
    );
    assert!(
        !project.join("dist/blog/b/index.html").exists(),
        "the vanished route's artifact must be pruned from disk"
    );
    assert!(outcome
        .pages_pruned
        .contains(&project.join("dist/blog/b/index.html")));
    assert_eq!(
        outcome.pages_stale,
        vec![PathBuf::from("blog/a/index.html")],
        "only the surviving route is (re-)announced as stale"
    );
    assert_eq!(outcome.pages_rendered, 0, "zero eager renders on this tick");
}

/// Skipped tick class (#956 + #1301): a Phase-B `Skipped` reload still
/// bypasses the V8 host boot, the table swap, the generation bump, and
/// disk pruning — but the render callback now runs for the tick's
/// SELECTED pages so their staleness is not silently dropped (#1301). An
/// all-lazy `.tsx` tick renders nothing eagerly yet re-announces its
/// selected routes as stale.
#[test]
fn skipped_tick_stales_selection_without_table_swap_or_prune() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let (graph, session) = matrix_fixture(project);
    let orch = orchestrator_for(project, graph, &session);

    // Tick 1: stale blog/a + blog/b (all-lazy .tsx tick), drain probe.
    let ctx1 = lazy_ctx(project, &session, "v1", plain_refresh);
    orch.tick_with_kinds(
        vec![(project.join("pages/blog.tsx"), ChangeKind::Modified)],
        &ctx1,
        None,
    )
    .expect("tick succeeded")
    .expect("non-noop");
    let generation_before = session.generation();
    let events_before = session.events().len();

    // Tick 2: byte-identical bundle → Skipped.
    let ctx2 = lazy_ctx(project, &session, "v1", |sess| {
        sess.events.lock().unwrap().push("reload:skipped");
        RefreshOutcome::Skipped
    });
    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("pages/blog.tsx"), ChangeKind::Modified)],
            &ctx2,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");

    // #1301: the skipped tick renders nothing eagerly (all-lazy .tsx) but
    // still re-stales its SELECTED routes rather than dropping them.
    assert_eq!(outcome.pages_rendered, 0);
    assert_eq!(
        outcome.pages_stale,
        vec![
            PathBuf::from("blog/a/index.html"),
            PathBuf::from("blog/b/index.html"),
        ],
        "a skipped tick re-announces its selected routes as stale (#1301)"
    );
    assert!(outcome.pages_pruned.is_empty());
    assert_eq!(
        session.generation(),
        generation_before,
        "no table swap → no generation bump"
    );
    assert!(
        session.is_stale("blog/a/index.html") && session.is_stale("blog/b/index.html"),
        "the selected routes remain stale after a skipped tick"
    );
    assert_eq!(
        &session.events()[events_before..],
        &["reload:skipped", "render:stale-marked"],
        "the render callback now runs on a skipped tick (#1301), after the skip"
    );
}

/// ORDERING (issue #1025): stale-state + route-table updates complete
/// strictly BEFORE the tick outcome is reported. `tick_with_kinds`'s
/// return value is exactly what `BuildOrchestrator::run` hands to
/// `on_outcome` (and thence the SSE layer), so asserting the event
/// order at return time pins that the eventual SSE Page event can never
/// beat the state swap — a reloading browser always finds the stale
/// route resolvable.
#[test]
fn outcome_is_delivered_only_after_table_swap_and_stale_marking() {
    let dir = tempdir().unwrap();
    let project = dir.path();
    let (graph, session) = matrix_fixture(project);
    let orch = orchestrator_for(project, graph, &session);
    let ctx = lazy_ctx(project, &session, "v1", plain_refresh);

    std::fs::write(project.join("content/a.md"), "---\ntitle: A\n---\n\nv2\n").unwrap();
    let outcome = orch
        .tick_with_kinds(
            vec![(project.join("content/a.md"), ChangeKind::Modified)],
            &ctx,
            None,
        )
        .expect("tick succeeded")
        .expect("non-noop");
    session.events.lock().unwrap().push("outcome:delivered");

    assert_eq!(
        session.events(),
        vec![
            "reload:tables-swapped",
            "render:stale-marked",
            "outcome:delivered",
        ],
        "table swap → stale marking → only then the outcome"
    );
    // At outcome-delivery time the stale state already describes the
    // new world: the routes the outcome announces are resolvable as
    // stale entries.
    for stale_route in &outcome.pages_stale {
        assert!(
            session.is_stale(stale_route.to_str().unwrap()),
            "{stale_route:?} must already be claimable when the outcome lands"
        );
    }
    assert!(
        !outcome.pages_stale.is_empty(),
        "sanity: the tick staled routes"
    );
}
