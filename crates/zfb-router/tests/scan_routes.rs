//! Integration tests for the file-based router.
//!
//! Fixtures live under `tests/fixtures/pages/` (a "happy path" tree) and
//! `tests/fixtures/pages_ambiguous/` (a tree that should be rejected).

use std::path::{Path, PathBuf};

use zfb_router::{Route, RouteKind, Router, RouterError, Segment};

fn fixture(name: &str) -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest).join("tests/fixtures").join(name)
}

fn templates(routes: &[Route]) -> Vec<String> {
    routes.iter().map(Route::template).collect()
}

#[test]
fn scans_canonical_pages_dir() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let templates = templates(router.routes());

    // Every expected route must appear (independent of order).
    for expected in [
        "/",
        "/about",
        "/blog",
        "/blog/:slug",
        "/blog/page/:page",
        "/docs/:slug*",
        "/:lang/:slug",
    ] {
        assert!(
            templates.iter().any(|t| t == expected),
            "missing {expected:?} in {templates:?}",
        );
    }
}

#[test]
fn skips_underscore_files() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    for route in router.routes() {
        let name = route
            .source_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        assert!(
            !name.starts_with('_'),
            "underscore file {name:?} should be ignored",
        );
    }
    let templates = templates(router.routes());
    assert!(!templates.iter().any(|t| t == "/_app"));
    assert!(!templates.iter().any(|t| t == "/_document"));
}

#[test]
fn skips_non_tsx_files() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    for route in router.routes() {
        let ext = route
            .source_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        assert_eq!(ext, "tsx");
    }
    // README.md should not have produced any route.
    assert!(!templates(router.routes()).iter().any(|t| t == "/README"));
}

#[test]
fn classifies_static_dynamic_catchall() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let by_template: std::collections::HashMap<String, RouteKind> = router
        .routes()
        .iter()
        .map(|r| (r.template(), r.kind))
        .collect();

    assert_eq!(by_template["/"], RouteKind::Static);
    assert_eq!(by_template["/about"], RouteKind::Static);
    assert_eq!(by_template["/blog"], RouteKind::Static);
    assert_eq!(by_template["/blog/:slug"], RouteKind::Dynamic);
    assert_eq!(by_template["/blog/page/:page"], RouteKind::Dynamic);
    assert_eq!(by_template["/:lang/:slug"], RouteKind::Dynamic);
    assert_eq!(by_template["/docs/:slug*"], RouteKind::Catchall);
}

#[test]
fn dynamic_param_name_preserved() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let blog_slug = router
        .routes()
        .iter()
        .find(|r| r.template() == "/blog/:slug")
        .expect("/blog/:slug");
    assert_eq!(
        blog_slug.segments,
        vec![Segment::Static("blog".into()), Segment::Dynamic("slug".into())],
    );

    let pagination = router
        .routes()
        .iter()
        .find(|r| r.template() == "/blog/page/:page")
        .expect("/blog/page/:page");
    assert_eq!(
        pagination.segments,
        vec![
            Segment::Static("blog".into()),
            Segment::Static("page".into()),
            Segment::Dynamic("page".into()),
        ],
    );

    let docs = router
        .routes()
        .iter()
        .find(|r| r.template() == "/docs/:slug*")
        .expect("/docs/:slug*");
    assert_eq!(
        docs.segments,
        vec![Segment::Static("docs".into()), Segment::Catchall("slug".into())],
    );
}

#[test]
fn sort_order_static_then_dynamic_then_catchall() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let kinds: Vec<RouteKind> = router.routes().iter().map(|r| r.kind).collect();
    let mut last_rank: u8 = 0;
    for kind in kinds {
        let rank = kind_rank(kind);
        assert!(last_rank <= rank, "routes not sorted by kind");
        last_rank = rank;
    }
}

fn kind_rank(k: RouteKind) -> u8 {
    match k {
        RouteKind::Static => 0,
        RouteKind::Dynamic => 1,
        RouteKind::Catchall => 2,
    }
}

#[test]
fn longer_paths_sort_before_shorter_within_kind() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let templates = templates(router.routes());

    // Among static routes, /about and /blog (1 segment) should appear before /
    // (0 segments).
    let pos_root = templates.iter().position(|t| t == "/").expect("/");
    let pos_about = templates.iter().position(|t| t == "/about").expect("/about");
    let pos_blog = templates.iter().position(|t| t == "/blog").expect("/blog");
    assert!(pos_about < pos_root, "/about should sort before /");
    assert!(pos_blog < pos_root, "/blog should sort before /");

    // Catchall /docs/:slug* must come last (after everything dynamic).
    let pos_docs = templates
        .iter()
        .position(|t| t == "/docs/:slug*")
        .expect("/docs/:slug*");
    let pos_blog_slug = templates
        .iter()
        .position(|t| t == "/blog/:slug")
        .expect("/blog/:slug");
    assert!(
        pos_blog_slug < pos_docs,
        "dynamic should beat catchall in sort order",
    );
}

#[test]
fn ambiguous_routes_are_rejected() {
    let err = Router::scan(&fixture("pages_ambiguous")).unwrap_err();
    match err {
        RouterError::AmbiguousRoute { template, .. } => {
            assert_eq!(template, "/blog");
        }
        other => panic!("expected AmbiguousRoute, got {other:?}"),
    }
}

#[test]
fn missing_pages_dir_is_an_error() {
    let err = Router::scan(Path::new(
        "/this/path/should/not/exist/zfb-router/test",
    ))
    .unwrap_err();
    assert!(matches!(err, RouterError::PagesDirMissing(_)));
}

#[test]
fn index_route_has_zero_segments() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let index = router
        .routes()
        .iter()
        .find(|r| r.template() == "/")
        .expect("root");
    assert!(index.segments.is_empty());
}

#[test]
fn nested_index_collapses_to_directory_route() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let blog = router
        .routes()
        .iter()
        .find(|r| r.template() == "/blog")
        .expect("/blog");
    assert_eq!(blog.segments, vec![Segment::Static("blog".into())]);
    assert!(blog.source_path.ends_with("blog/index.tsx"));
}
