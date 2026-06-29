//! Reproduction tests for bug #1284 (epic #1285), Wave-1 diagnosis #1286 —
//! orchestrator `plan_for_changes` level (Level 1, pure fold, no V8).
//!
//! NOTE: build/run these with `--no-default-features` to avoid the V8 first
//! compile — the orchestrator/plan/policy code under test does not need
//! `embed_v8`:
//!
//! ```sh
//! cargo test -p zfb-build --no-default-features --test dev_dep_invalidation_1284
//! ```
//!
//! These pin the page-SELECTION asymmetry that is the orchestrator half of the
//! bug, empirically confirmed during diagnosis:
//!
//! - The `Page|Module|Content|Data` arm (`orchestrator.rs:~452`) falls back to
//!   `PageSelection::All` when `dirty_pages` is empty. So a `components/**`
//!   edit in the (edge-less) dev graph selects ALL pages — imprecise but it
//!   *does* re-render. The #1287 fix replaces this blunt All-fallback with a
//!   precise per-route selection driven by populated Module edges.
//! - The `Style` arm (`orchestrator.rs:~480`) does NOT fall back to All: an
//!   empty `dirty_pages(css)` yields `Specific({})` — zero pages. Combined
//!   with the missing Style edges in dev, a transitively-imported CSS edit
//!   re-renders no page (symptom B at the planner).
//!
//! The "current_bug_*" tests assert TODAY's behaviour (and are not ignored, to
//! lock the regression boundary). The fixed-behaviour tests are
//! `#[ignore = "pending fix: #1284"]`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use zfb_build::{BuildOrchestrator, DevAssetPipeline, OrchestratorConfig, PageSelection};
use zfb_graph::{DependencyGraph, PageDeps, PageId};

fn pid(p: PathBuf) -> PageId {
    PageId::new(p)
}

/// Dev-seeded orchestrator: pages with NO dep edges (mirrors `dev.rs` boot
/// seed), watching `components` + `styles`.
fn dev_orch(project: &std::path::Path) -> BuildOrchestrator<DevAssetPipeline> {
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(pid(project.join("pages/index.tsx")), vec![]));
    g.upsert(PageDeps::new(pid(project.join("pages/about.tsx")), vec![]));
    let graph = Arc::new(Mutex::new(g));
    BuildOrchestrator::new(
        OrchestratorConfig::new(
            project,
            vec![PathBuf::from("components"), PathBuf::from("styles")],
        ),
        graph,
        DevAssetPipeline::new(),
    )
}

/// SYMPTOM A (current, imprecise) — a component edit selects ALL pages via the
/// Page|Module All-fallback. Documents present behaviour; not ignored. The fix
/// narrows this to the specific consumer route.
#[test]
fn current_component_edit_selects_all_pages_imprecisely() {
    let project = PathBuf::from("/proj");
    let orch = dev_orch(&project);
    let plan = orch.plan_for_changes(vec![project.join("components/Header.tsx")]);
    assert!(
        plan.pages.is_all(),
        "today the Page|Module arm falls back to All when the dev graph has no Module edges"
    );
}

/// SYMPTOM A (fixed) — once Module edges exist, the component edit selects only
/// its consumer route, NOT All. Ignored until #1287 lands.
#[test]
#[ignore = "pending fix: #1284"]
fn component_edit_selects_only_consumer_route() {
    let project = PathBuf::from("/proj");
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid(project.join("pages/index.tsx")),
        vec![(
            project.join("components/Header.tsx"),
            zfb_graph::DepKind::Module,
        )],
    ));
    g.upsert(PageDeps::new(pid(project.join("pages/about.tsx")), vec![]));
    let orch = BuildOrchestrator::new(
        OrchestratorConfig::new(&project, vec![PathBuf::from("components")]),
        Arc::new(Mutex::new(g)),
        DevAssetPipeline::new(),
    );

    let plan = orch.plan_for_changes(vec![project.join("components/Header.tsx")]);
    match plan.pages {
        PageSelection::Specific(pages) => {
            assert_eq!(
                pages.into_iter().collect::<Vec<_>>(),
                vec![pid(project.join("pages/index.tsx"))],
                "fix selects exactly the consuming route"
            );
        }
        PageSelection::All => panic!("after #1287 the selection must be precise, not All"),
    }
}

/// SYMPTOM A (fixed) — a component edit must also re-run the CSS content
/// re-scan, because a new utility class authored inside that component
/// (symptom C) only reaches `/assets/styles.css` when the CSS pipeline
/// re-runs. The #1288-owned `mark_css` edit at `orchestrator.rs:~474-478`
/// makes a Module change set `rerun_css`. Ignored until the fix lands.
#[test]
#[ignore = "pending fix: #1284"]
fn component_edit_triggers_css_rescan() {
    let project = PathBuf::from("/proj");
    let orch = dev_orch(&project);
    let plan = orch.plan_for_changes(vec![project.join("components/Header.tsx")]);
    assert!(
        plan.rerun_css,
        "a Module edit must trigger a CSS content re-scan so a new utility class is emitted (symptom C)"
    );
}

/// SYMPTOM B (current) — the Style arm does NOT fall back to All. A
/// transitively-imported CSS file (no Style edge in the dev graph) selects
/// ZERO pages even though `rerun_css` is set. Documents present behaviour;
/// not ignored. This is the asymmetry vs the Page|Module arm.
#[test]
fn current_style_arm_selects_no_pages_without_edges() {
    let project = PathBuf::from("/proj");
    let orch = dev_orch(&project);
    let plan = orch.plan_for_changes(vec![project.join("styles/theme.css")]);
    assert!(plan.rerun_css, "a styles/ edit always re-runs CSS");
    assert!(
        !plan.pages.is_all(),
        "the Style arm does NOT fall back to All (asymmetry vs Page|Module arm)"
    );
    assert!(
        matches!(plan.pages, PageSelection::Specific(ref s) if s.is_empty()),
        "with no Style edge, the Style arm selects zero pages"
    );
}
