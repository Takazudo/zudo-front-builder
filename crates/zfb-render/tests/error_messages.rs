//! Cross-crate "error-quality" acceptance tests.
//!
//! These tests pin the user-facing error messages produced by the failure
//! modes catalogued in Epic 6 / Sub 4. The bar each error must clear:
//!
//! - **file path** — points at the source file the user authored
//! - **line/col** when available
//! - **what went wrong** — concrete signal, not "something failed"
//! - **what was expected** — actionable hint
//!
//! Each test asserts only the *load-bearing* substrings, never the full
//! message string, so cosmetic phrasing tweaks don't break the suite.

use zfb_render::paths::{resolve_paths, PathsCache, PathsError, Segment};

// ---------------------------------------------------------------------------
// Failure mode 3 — `paths()` returning the wrong shape.
// ---------------------------------------------------------------------------

/// `paths()` returning a non-array must name the route file and say what
/// shape was expected.
#[test]
fn paths_export_wrong_top_level_shape_names_route_file() {
    let mut cache = PathsCache::new();
    let segs = vec![
        Segment::Static("blog".to_string()),
        Segment::Dynamic("slug".to_string()),
    ];
    let export = serde_json::json!({ "not": "an array" });

    let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export)
        .expect_err("non-array export should fail");
    let msg = err.to_string();

    assert!(
        msg.contains("blog/[slug].tsx"),
        "expected route file in error, got: {msg}",
    );
    assert!(
        msg.contains("expected"),
        "expected an `expected …` clause in error, got: {msg}",
    );
    match err {
        PathsError::InvalidPathsExport {
            route,
            field,
            expected,
            ..
        } => {
            assert_eq!(route, "blog/[slug].tsx");
            assert!(
                expected.contains("array"),
                "expected `array` in expected clause, got: {expected}",
            );
            assert_eq!(field.as_deref(), Some("paths()"));
        }
        other => unreachable!("expected InvalidPathsExport, got {other:?}"),
    }
}

/// `paths()` returning an entry with the wrong field shape (missing
/// `params`) must name the route file and the bad field path.
#[test]
fn paths_entry_missing_params_names_field_and_route_file() {
    let mut cache = PathsCache::new();
    let segs = vec![
        Segment::Static("blog".to_string()),
        Segment::Dynamic("slug".to_string()),
    ];
    let export = serde_json::json!([{ "props": { "title": "no params here" } }]);

    let err = resolve_paths(&mut cache, "pages/blog/[slug].tsx", &segs, &export)
        .expect_err("missing `params` should fail");
    let msg = err.to_string();

    assert!(
        msg.contains("pages/blog/[slug].tsx"),
        "expected route file in error, got: {msg}",
    );
    assert!(
        msg.contains("entry[0].params"),
        "expected the bad field path in error, got: {msg}",
    );
    assert!(
        msg.contains("expected"),
        "expected an `expected …` clause in error, got: {msg}",
    );
}

/// Canonical Sub-6 case: a `paths()` returning `[{ params: { wrongName: "x" } }]`
/// for `[slug].tsx` must surface BOTH the source file path AND a clear
/// "expected `slug`, got `wrongName`" param-name diagnostic.
#[test]
fn paths_missing_param_surfaces_expected_and_got_with_route_file() {
    let mut cache = PathsCache::new();
    let segs = vec![Segment::Dynamic("slug".to_string())];
    let export = serde_json::json!([{ "params": { "wrongName": "x" } }]);

    let err = resolve_paths(&mut cache, "pages/[slug].tsx", &segs, &export)
        .expect_err("missing required param should fail");
    let msg = err.to_string();

    // (a) Source file path is present.
    assert!(
        msg.contains("pages/[slug].tsx"),
        "expected route file path in error, got: {msg}",
    );
    // (b) Expected param name is present.
    assert!(
        msg.contains("`slug`"),
        "expected required param name in error, got: {msg}",
    );
    // (c) The actually-provided key (the typo) is present so the user can
    //     spot the mismatch immediately.
    assert!(
        msg.contains("`wrongName`"),
        "expected provided-keys list to mention `wrongName`, got: {msg}",
    );
    // (d) The structured form carries both fields so consumers (CLI
    //     diagnostics) can format the same diagnostic without re-parsing.
    match err {
        PathsError::MissingParam {
            name,
            route,
            provided,
        } => {
            assert_eq!(name, "slug");
            assert_eq!(route, "pages/[slug].tsx");
            assert_eq!(provided, vec!["wrongName".to_string()]);
        }
        other => panic!("expected MissingParam, got {other:?}"),
    }
}

/// `ExtraParam` should likewise carry the route file path AND a clear
/// "expected one of […], got `…`" diagnostic listing the route's declared
/// param names.
#[test]
fn paths_extra_param_surfaces_expected_and_got_with_route_file() {
    let mut cache = PathsCache::new();
    let segs = vec![Segment::Dynamic("slug".to_string())];
    let export = serde_json::json!([
        { "params": { "slug": "ok", "stray": "x" } }
    ]);

    let err = resolve_paths(&mut cache, "pages/[slug].tsx", &segs, &export)
        .expect_err("extra param should fail");
    let msg = err.to_string();

    assert!(
        msg.contains("pages/[slug].tsx"),
        "expected route file path in error, got: {msg}",
    );
    assert!(
        msg.contains("`stray`"),
        "expected offending param name in error, got: {msg}",
    );
    assert!(
        msg.contains("`slug`"),
        "expected the declared param name list to mention `slug`, got: {msg}",
    );
    match err {
        PathsError::ExtraParam {
            name,
            route,
            expected,
        } => {
            assert_eq!(name, "stray");
            assert_eq!(route, "pages/[slug].tsx");
            assert_eq!(expected, vec!["slug".to_string()]);
        }
        other => panic!("expected ExtraParam, got {other:?}"),
    }
}

/// A `params` value with the wrong type names both the param, the route
/// file, and (via the shared `route` field) the file context.
#[test]
fn paths_param_with_wrong_type_names_param_and_route() {
    let mut cache = PathsCache::new();
    let segs = vec![
        Segment::Static("blog".to_string()),
        Segment::Dynamic("slug".to_string()),
    ];
    // `slug` should be a string; pass a number.
    let export = serde_json::json!([{ "params": { "slug": 42 } }]);

    let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export)
        .expect_err("number slug should fail");
    let msg = err.to_string();

    assert!(
        msg.contains("blog/[slug].tsx"),
        "expected route file in error, got: {msg}",
    );
    assert!(
        msg.contains("slug"),
        "expected param name in error, got: {msg}",
    );
}

// ---------------------------------------------------------------------------
// Failure mode 5 — invalid `zfb.config.ts`: relocated to `crates/zfb`.
//
// The original plan for this test (issue #1353) was "wire zfb-render to call
// the in-process TS config evaluator". That premise is stale: the V8
// evaluator *core* (`ThreadedConfigEvaluator::eval_bundle`) already lives
// inside zfb-render (`src/config_eval/mod.rs`, covered by
// `tests/config_eval.rs`), but the esbuild-bundling orchestration around it
// was hoisted into `zfb-config-loader` (issue #1037), which normally-depends
// on zfb-render — so "zfb-render calls the full TS-config pipeline" is a
// dependency cycle in every direction. The path/field/expected error quality
// this test wants to assert is also owned entirely by the `zfb` bin crate
// (`crates/zfb/src/config.rs`), not by zfb-render.
//
// The real-pipeline test now lives in `crates/zfb/src/config.rs` mod tests
// as `invalid_zfb_config_ts_points_at_field_and_file` (wrong `framework`
// value) and `invalid_zfb_config_ts_collection_missing_path_field` (missing
// `collections[].path`), issue #1359.
// ---------------------------------------------------------------------------
