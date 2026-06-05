//! Routing-side integration test for the routing+rendering fixtures.
//!
//! This file used to be parked behind `#![cfg(any())]` because it was
//! authored against a planned but never-built unified
//! `render_route(RenderInput) -> RenderOutput` orchestrator on
//! `zfb-render`. The actual API surface that landed is:
//!
//! - `zfb_render::Renderer<H: RenderHost>` — a single-page,
//!   single-source orchestrator that delegates JS execution to a
//!   pluggable host (`render_smoke.rs` shows the in-process test host
//!   pattern). It does not walk a project root, resolve layouts, or run
//!   `paths()`.
//! - `zfb_build::renderer::render_all` — the production end-to-end
//!   pipeline, which drives the embedded V8 host and serves a
//!   pre-bundled Worker entry. That orchestrator is exercised by
//!   `zfb-build`'s own test suite, not from inside `zfb-render`.
//!
//! Neither matches the in-process, fixture-walking, dual-framework
//! harness the parked test was sketching. Building that harness is a
//! sizeable feature in its own right, so for now this file only
//! exercises the routing half of the contract: scan the fixture pages
//! and assert the route templates and kinds line up with what the
//! rendering harness will eventually consume. The TSX pages, layouts,
//! components, and content under `tests/fixtures/routing-rendering/`
//! are kept on disk for the full end-to-end test to plug into later.
//!
//! Tracked follow-up: the full harness (TSX compile + JS execution +
//! layout wrapping + dual-framework byte-equality + snapshotting)
//! belongs at the `zfb-build` orchestrator layer. Tracked at
//! https://github.com/Takazudo/zudo-front-builder/issues/63 .
//!
//! Route template syntax: `zfb-router` renders dynamic and catchall
//! segments using `:name` / `:name{.+}` (Hono path-pattern style), not
//! the `[name]` / `[...name]` filename style. The filename brackets
//! are the input convention; `:name` is the canonical template form.

use std::path::PathBuf;

use zfb_router::{scan_pages, RouteKind};

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("routing-rendering")
}

fn pages_dir() -> PathBuf {
    fixtures_root().join("pages")
}

#[test]
fn router_discovers_all_required_routes() {
    let routes = scan_pages(&pages_dir())
        .unwrap_or_else(|e| panic!("scan_pages failed: {e}"));
    let templates: Vec<String> = routes.iter().map(|r| r.template()).collect();
    for required in [
        "/",
        "/about",
        "/blog",
        "/blog/:slug",
        "/blog/page/:page",
        "/docs/:slug{.+}",
        "/manual/:slug{.+}?",
        "/:lang/:slug",
    ] {
        assert!(
            templates.iter().any(|t| t == required),
            "router missing required route {required}; got {templates:?}",
        );
    }
}

#[test]
fn router_classifies_route_kinds_correctly() {
    let routes = scan_pages(&pages_dir())
        .unwrap_or_else(|e| panic!("scan_pages failed: {e}"));

    let kind_of = |template: &str| -> RouteKind {
        routes
            .iter()
            .find(|r| r.template() == template)
            .unwrap_or_else(|| panic!("no route for {template}"))
            .kind
    };

    assert_eq!(kind_of("/"), RouteKind::Static);
    assert_eq!(kind_of("/about"), RouteKind::Static);
    assert_eq!(kind_of("/blog"), RouteKind::Static);
    assert_eq!(kind_of("/blog/:slug"), RouteKind::Dynamic);
    assert_eq!(kind_of("/blog/page/:page"), RouteKind::Dynamic);
    assert_eq!(kind_of("/docs/:slug{.+}"), RouteKind::Catchall);
    // Optional catchall (#812) shares RouteKind::Catchall.
    assert_eq!(kind_of("/manual/:slug{.+}?"), RouteKind::Catchall);
    assert_eq!(kind_of("/:lang/:slug"), RouteKind::Dynamic);
}
