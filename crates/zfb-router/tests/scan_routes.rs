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
        "/docs/:slug{.+}",
        "/manual/:slug{.+}?",
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
fn skips_non_page_extension_files() {
    // Accepted page extensions: tsx, md, html. Any other extension must be
    // skipped. The fixture pages/ dir contains only .tsx and a README.md.
    let accepted = &["tsx", "md", "html"];
    let router = Router::scan(&fixture("pages")).expect("scan");
    for route in router.routes() {
        let ext = route
            .source_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        assert!(
            accepted.contains(&ext),
            "unexpected extension {ext:?} for {:?}",
            route.source_path,
        );
    }
    // README.md in the fixture is a .md page source and now produces /README.
    let tmpl = templates(router.routes());
    assert!(
        tmpl.iter().any(|t| t == "/README"),
        "README.md should now produce /README route; got: {tmpl:?}"
    );
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
    assert_eq!(by_template["/docs/:slug{.+}"], RouteKind::Catchall);
    // The optional catchall shares RouteKind::Catchall (it only drives
    // sorting/dispatch; the segment carries the optional-ness).
    assert_eq!(by_template["/manual/:slug{.+}?"], RouteKind::Catchall);
}

#[test]
fn optional_catchall_param_name_preserved() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let manual = router
        .routes()
        .iter()
        .find(|r| r.template() == "/manual/:slug{.+}?")
        .expect("/manual/:slug{.+}?");
    assert_eq!(
        manual.segments,
        vec![
            Segment::Static("manual".into()),
            Segment::OptionalCatchall("slug".into()),
        ],
    );
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
        vec![
            Segment::Static("blog".into()),
            Segment::Dynamic("slug".into())
        ],
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
        .find(|r| r.template() == "/docs/:slug{.+}")
        .expect("/docs/:slug{.+}");
    assert_eq!(
        docs.segments,
        vec![
            Segment::Static("docs".into()),
            Segment::Catchall("slug".into())
        ],
    );
}

#[test]
fn sort_order_is_per_segment_rank_vector() {
    // Routes are sorted by the per-segment rank vector (static `0` <
    // dynamic `1` < catchall `2`), compared lexicographically — the same
    // key `zfb_build::bundler::route_sort_key` uses for Hono registration
    // order (#814). The whole table must be non-decreasing in that key.
    // A coarse per-route kind sort is intentionally NOT the contract: a
    // static-prefixed catchall (`/docs/:slug{.+}` → `[0, 2]`) sorts ahead
    // of a fully-dynamic pair (`/:lang/:slug` → `[1, 1]`).
    let router = Router::scan(&fixture("pages")).expect("scan");
    let keys: Vec<Vec<u8>> = router.routes().iter().map(rank_vector).collect();
    for w in keys.windows(2) {
        assert!(
            w[0] <= w[1],
            "routes not sorted by per-segment rank vector: {:?} before {:?}",
            w[0],
            w[1],
        );
    }
}

fn rank_vector(route: &Route) -> Vec<u8> {
    route
        .segments
        .iter()
        .map(|s| match s {
            Segment::Static(_) => 0u8,
            Segment::Dynamic(_) => 1,
            Segment::Catchall(_) | Segment::OptionalCatchall(_) => 2,
        })
        .collect()
}

#[test]
fn more_specific_routes_sort_before_looser_overlapping_ones() {
    let router = Router::scan(&fixture("pages")).expect("scan");
    let templates = templates(router.routes());
    let pos = |t: &str| {
        templates
            .iter()
            .position(|x| x == t)
            .unwrap_or_else(|| panic!("missing {t} in {templates:?}"))
    };

    // The empty (index) route carries the empty rank vector, which sorts
    // first — matching the bundler (`/ → []` "empty vector sorts first").
    // `/` never overlaps `/about`, so this is a deterministic tiebreak only.
    assert!(pos("/") < pos("/about"), "/ (empty vector) sorts first");

    // Static prefix beats a fully-dynamic pair at the same depth: a URL
    // like `/docs/a` is matched by both `/docs/:slug{.+}` and `/:lang/:slug`
    // and must dispatch to the static-prefixed catchall (#814).
    assert!(
        pos("/docs/:slug{.+}") < pos("/:lang/:slug"),
        "static-prefixed catchall must beat the fully-dynamic pair",
    );

    // A deeper static-prefixed dynamic route still beats a fully-dynamic
    // pair too (`/blog/:slug` before `/:lang/:slug`).
    assert!(
        pos("/blog/:slug") < pos("/:lang/:slug"),
        "static-prefixed dynamic must beat the fully-dynamic pair",
    );
}

#[test]
fn ambiguous_routes_are_rejected() {
    let err = Router::scan(&fixture("pages_ambiguous")).unwrap_err();
    match err {
        RouterError::AmbiguousRoute { template, .. } => {
            assert_eq!(template, "/blog");
        }
        other => unreachable!("expected AmbiguousRoute, got {other:?}"),
    }
}

#[test]
fn missing_pages_dir_is_an_error() {
    let err = Router::scan(Path::new("/this/path/should/not/exist/zfb-router/test")).unwrap_err();
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
