// TODO(parked): This integration test is parked.
//
// It depends on the orchestrator API surface (`render_route`,
// `RenderInput`, `RenderOutput`) and the `zfb-router` workspace crate,
// none of which exist yet. Until those land, the test cannot compile,
// and a `cargo clippy --workspace --all-features --all-targets` (or
// `cargo check --all-features`) run otherwise breaks the workspace
// build.
//
// To work around that, this file is unconditionally excluded from
// compilation via `#![cfg(any())]` (the canonical "always-false" cfg)
// and the matching `integration_routing_rendering = []` feature has
// been removed from `crates/zfb-render/Cargo.toml`. The test source,
// fixtures, layouts, components, and harness are intentionally kept on
// disk so they remain version-controlled.
//
// When the orchestrator API and `zfb-router` crate land, restore the
// original `#![cfg(feature = "integration_routing_rendering")]`
// attribute below and re-add the feature line in `Cargo.toml`.

//! End-to-end integration tests for the Epic 3 routing + rendering
//! pipeline.
//!
//! For each fixture page under
//! `tests/fixtures/routing-rendering/pages/`, this harness:
//!   1. Scans the fixture project via `zfb-router` (Sub 2).
//!   2. Compiles the page's TSX module via `zfb-render` (Sub 3).
//!   3. Calls `paths()` for dynamic routes (Sub 5) → enumerates URLs.
//!   4. Resolves `meta` / `meta.layout` (Sub 6) and wraps the page in
//!      its layout.
//!   5. Renders to HTML through the configured framework adapter
//!      (Sub 4) — once with `framework: "preact"`, once with
//!      `framework: "react"`.
//!   6. Compares the resulting HTML against the snapshot under
//!      `tests/fixtures/routing-rendering/snapshots/{preact,react}/`.
//!   7. Asserts the Preact and React outputs are byte-identical for
//!      portable components (ADR-002 contract).
//!
//! ## Snapshot bootstrap
//!
//! Snapshots are populated on first run when `INSANE_UPDATE_SNAPSHOTS=1`
//! is set OR the snapshot file is missing. After Subs 1–7 land, the
//! manager (or whoever runs the merged base) runs:
//!
//! ```sh
//! INSANE_UPDATE_SNAPSHOTS=1 cargo test -p zfb-render \
//!     --test integration_routing_rendering
//! ```
//!
//! Once captured, re-runs without the env var perform the comparison.
//!
//! ## Why this file may not compile in isolation
//!
//! `zfb-render` and `zfb-router` are produced by Subs 2 and 3 and do
//! not exist yet on the sub08 worktree. Building this crate alone will
//! fail; the harness compiles only after the manager merges Subs 2–7
//! into the epic base branch.

// Gated behind the `integration_routing_rendering` feature because the
// harness depends on API surface (`render_route`, `RenderInput`,
// `RenderOutput`, `zfb-router` workspace dep) that has not yet been
// wired up at the zfb-render crate root. The TSX fixtures, layouts,
// components, and harness code are preserved as-is so the integration
// can be turned on with a single Cargo.toml change once the orchestrator
// exposes the expected entry points and zfb-render adds zfb-router as
// a dep. Tracked as the natural follow-up to Epic 3 (Subs 1–7).
#![cfg(any())]
#![allow(dead_code)]

use std::path::{Path, PathBuf};

// NOTE: These imports describe the *intended* API surface from sibling
// crates. Sub 2 (zfb-router), Sub 3 (zfb-render), and Sub 4 (adapters)
// will land the actual symbols. If the names drift during merge, fix
// them here at review time.
use zfb_render::{render_route, Framework, RenderInput, RenderOutput};
use zfb_router::{scan_pages, RouteKind, ScannedRoute};

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("routing-rendering")
}

fn pages_dir() -> PathBuf {
    fixtures_root().join("pages")
}

fn snapshot_path(framework: Framework, name: &str) -> PathBuf {
    let bucket = match framework {
        Framework::Preact => "preact",
        Framework::React => "react",
    };
    fixtures_root()
        .join("snapshots")
        .join(bucket)
        .join(name)
}

// ---------------------------------------------------------------------------
// Snapshot equality with first-run capture (mirrors zfb-content's harness)
// ---------------------------------------------------------------------------

fn assert_snapshot_eq(actual: &str, snapshot_path: &Path) {
    let update = std::env::var("INSANE_UPDATE_SNAPSHOTS").is_ok();
    if update || !snapshot_path.exists() {
        if let Some(parent) = snapshot_path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create snapshot dir {parent:?}: {e}"));
        }
        std::fs::write(snapshot_path, actual)
            .unwrap_or_else(|e| panic!("write snapshot {snapshot_path:?}: {e}"));
        return;
    }
    let expected = std::fs::read_to_string(snapshot_path)
        .unwrap_or_else(|e| panic!("read snapshot {snapshot_path:?}: {e}"));
    if actual == expected {
        return;
    }
    let line = expected
        .lines()
        .zip(actual.lines())
        .enumerate()
        .find(|(_, (e, a))| e != a)
        .map(|(i, (e, a))| {
            format!(
                "first divergence at line {i}:\n  expected: {e:?}\n  actual:   {a:?}"
            )
        })
        .unwrap_or_else(|| {
            format!(
                "trailing-content mismatch: expected {} lines, actual {} lines",
                expected.lines().count(),
                actual.lines().count(),
            )
        });
    // Snapshot mismatch is a legitimate test-failure mechanism. `panic!`
    // is the assertion primitive here because the bespoke diff output is
    // more readable than `assert_eq!` of two large strings.
    panic!(
        "snapshot mismatch at {snapshot_path:?}\n{line}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
    );
}

// ---------------------------------------------------------------------------
// Render a single route key under one framework, return HTML
// ---------------------------------------------------------------------------

/// Render the route identified by `route_template` (e.g. `/about`,
/// `/blog/[slug]`) under `framework`. For dynamic routes, `params`
/// pins a specific concrete URL produced by `paths()`.
///
/// Internally:
///   1. `zfb_router::scan_pages` builds the route tree from the
///      fixture project.
///   2. `zfb_render::render_route` compiles the matched page module,
///      runs `paths()` if needed, picks the matching params, calls
///      `default()`, wraps with `meta.layout`, and serializes via the
///      adapter for `framework`.
fn render(
    framework: Framework,
    route_template: &str,
    params: Option<&[(&str, &str)]>,
) -> String {
    let routes = scan_pages(&pages_dir())
        .unwrap_or_else(|e| panic!("scan_pages failed: {e}"));
    let route = routes
        .iter()
        .find(|r| r.template == route_template)
        .unwrap_or_else(|| {
            panic!(
                "route {route_template} not found in scanned routes: {:?}",
                routes
                    .iter()
                    .map(|r: &ScannedRoute| r.template.as_str())
                    .collect::<Vec<_>>()
            )
        });

    let input = RenderInput {
        project_root: fixtures_root(),
        framework,
        route: route.clone(),
        // `params` are the concrete bindings the harness wants to
        // render. For static pages this is `None` and the renderer
        // calls `default()` once. For dynamic pages the renderer
        // matches params against the entries returned by `paths()`.
        params: params.map(|kv| {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>()
        }),
    };

    let RenderOutput { html, .. } = render_route(input)
        .unwrap_or_else(|e| panic!("render_route failed for {route_template}: {e}"));
    html
}

/// Render the same route under both frameworks, snapshot each, and
/// assert the two outputs are byte-identical (ADR-002 portable-
/// component contract).
fn check_route_under_both(
    route_template: &str,
    snapshot_basename: &str,
    params: Option<&[(&str, &str)]>,
) {
    let preact = render(Framework::Preact, route_template, params);
    let react = render(Framework::React, route_template, params);

    assert_snapshot_eq(&preact, &snapshot_path(Framework::Preact, snapshot_basename));
    assert_snapshot_eq(&react, &snapshot_path(Framework::React, snapshot_basename));

    assert_eq!(
        preact, react,
        "Preact and React HTML must be byte-identical for portable \
         components (route {route_template}). See ADR-002.",
    );
}

// ---------------------------------------------------------------------------
// Test cases — one per fixture page
// ---------------------------------------------------------------------------

#[test]
fn renders_about_under_preact() {
    let html = render(Framework::Preact, "/about", None);
    assert_snapshot_eq(&html, &snapshot_path(Framework::Preact, "about.html"));
}

#[test]
fn renders_about_under_react() {
    let html = render(Framework::React, "/about", None);
    assert_snapshot_eq(&html, &snapshot_path(Framework::React, "about.html"));
}

#[test]
fn renders_about_identically_across_frameworks() {
    check_route_under_both("/about", "about.html", None);
}

#[test]
fn renders_index_under_both() {
    check_route_under_both("/", "index.html", None);
}

#[test]
fn renders_blog_index_under_both() {
    check_route_under_both("/blog", "blog-index.html", None);
}

#[test]
fn renders_blog_slug_under_both_frameworks_identically() {
    // One concrete URL per stub post — see content/posts.ts.
    for slug in ["hello", "second", "third", "fourth"] {
        check_route_under_both(
            "/blog/[slug]",
            &format!("blog-{slug}.html"),
            Some(&[("slug", slug)]),
        );
    }
}

#[test]
fn renders_blog_pagination_under_both() {
    // 4 posts, pageSize 2 → 2 pages.
    for page in ["1", "2"] {
        check_route_under_both(
            "/blog/page/[page]",
            &format!("blog-page-{page}.html"),
            Some(&[("page", page)]),
        );
    }
}

#[test]
fn renders_docs_catchall_under_both() {
    // Three stub docs in pages/docs/[...slug].tsx. The catchall
    // produces multi-segment URLs; the params binding for a catchall
    // is the joined slug.
    for (joined, snapshot_name) in [
        ("intro", "docs-intro.html"),
        ("guides/install", "docs-guides-install.html"),
        ("guides/config/framework", "docs-guides-config-framework.html"),
    ] {
        check_route_under_both(
            "/docs/[...slug]",
            snapshot_name,
            Some(&[("slug", joined)]),
        );
    }
}

#[test]
fn renders_nested_dynamic_under_both() {
    // 2 langs × 2 slugs = 4 concrete URLs.
    for lang in ["en", "ja"] {
        for slug in ["welcome", "goodbye"] {
            check_route_under_both(
                "/[lang]/[slug]",
                &format!("{lang}-{slug}.html"),
                Some(&[("lang", lang), ("slug", slug)]),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Routing-only assertions: `zfb-router` must produce the right URLs.
// These don't depend on the JS runtime, so they're a useful smoke test
// even before the renderer is fully wired.
// ---------------------------------------------------------------------------

#[test]
fn router_discovers_all_required_routes() {
    let routes = scan_pages(&pages_dir())
        .unwrap_or_else(|e| panic!("scan_pages failed: {e}"));
    let templates: Vec<&str> = routes.iter().map(|r| r.template.as_str()).collect();
    for required in [
        "/",
        "/about",
        "/blog",
        "/blog/[slug]",
        "/blog/page/[page]",
        "/docs/[...slug]",
        "/[lang]/[slug]",
    ] {
        assert!(
            templates.contains(&required),
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
            .find(|r| r.template == template)
            .unwrap_or_else(|| panic!("no route for {template}"))
            .kind
            .clone()
    };

    assert_eq!(kind_of("/about"), RouteKind::Static);
    assert_eq!(kind_of("/blog/[slug]"), RouteKind::Dynamic);
    assert_eq!(kind_of("/docs/[...slug]"), RouteKind::Catchall);
    assert_eq!(kind_of("/[lang]/[slug]"), RouteKind::Dynamic);
}
