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

use std::path::PathBuf;

use zfb_render::loader::ModuleLoader;
use zfb_render::paths::{resolve_paths, PathsCache, PathsError, Segment};
use zfb_render::swc_pipeline::JsxRuntime;
use zfb_render::RenderError;

// ---------------------------------------------------------------------------
// Failure mode 1 — broken JS import in a TSX page.
// ---------------------------------------------------------------------------

/// The resolver should name the importer **file** (not just its dir), the
/// missing specifier, and at least one candidate path it probed.
#[test]
fn broken_relative_import_points_at_importer_file_and_lists_candidates() {
    // Construct a project tree on disk so the resolver actually walks the
    // filesystem and reports the absolute paths it tried.
    let tmp = mk_tmp("broken-import");
    let pages = tmp.path.join("pages");
    std::fs::create_dir_all(&pages).unwrap();
    let importer = pages.join("index.tsx");
    std::fs::write(
        &importer,
        "import { foo } from \"./does-not-exist\";\nexport default function P(){ return null; }\n",
    )
    .unwrap();

    let loader = ModuleLoader::new(JsxRuntime::Preact);
    // Pretend the parser surfaced the import on line 1, column 1.
    let err = loader
        .resolve_relative_from_file(&importer, "./does-not-exist", Some(1), Some(1))
        .expect_err("resolution should fail");

    let msg = err.to_string();
    let importer_str = importer.display().to_string();

    assert!(
        msg.contains(&importer_str),
        "expected importer file path `{importer_str}` in error, got: {msg}",
    );
    assert!(
        msg.contains("does-not-exist"),
        "expected missing specifier in error, got: {msg}",
    );
    assert!(
        msg.contains(":1:1"),
        "expected `:line:col` location marker, got: {msg}",
    );
    // The probe list should mention at least one extension we tried.
    assert!(
        msg.contains(".tsx"),
        "expected `.tsx` candidate in `tried` list, got: {msg}",
    );

    // And the error variant is the structured one (not a stringified Other).
    match err {
        RenderError::Resolve {
            specifier,
            importer: ie,
            line,
            col,
            tried,
        } => {
            assert_eq!(specifier, "./does-not-exist");
            assert_eq!(ie, importer_str);
            assert_eq!(line, Some(1));
            assert_eq!(col, Some(1));
            assert!(
                !tried.is_empty(),
                "expected the resolver to record probed candidates, got tried={tried:?}",
            );
        }
        other => panic!("expected RenderError::Resolve, got {other:?}"),
    }
}

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
        other => panic!("expected InvalidPathsExport, got {other:?}"),
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
// Failure mode 5 — invalid `zfb.config.ts`: SKIPPED.
//
// No crate currently parses `zfb.config.ts` into a typed Rust struct (the
// adapter / SWC pipeline references it only in doc-comments, and the
// islands esbuild crate touches it as a marker). When the loader lands,
// this test should produce a config file with a missing required field /
// wrong type and assert the error includes:
//   - the absolute path to `zfb.config.ts`
//   - the bad field name
//   - what was expected (e.g. `framework: "preact" | "react"`)
// Tracked in the Sub 4 log as a follow-up.
// ---------------------------------------------------------------------------
#[test]
#[ignore = "no zfb.config.ts loader exists yet — see Sub 4 log follow-up"]
fn invalid_zfb_config_ts_points_at_field_and_file() {
    // Once a config loader exists in zfb (or zfb-render), build a tmp
    // config with a missing `framework` field and assert the error
    // mentions the path, the field name, and the expected union type.
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

struct TmpDir {
    path: PathBuf,
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn mk_tmp(label: &str) -> TmpDir {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "zfb-render-error-quality-{label}-{nanos}-{n}-{pid}",
        pid = std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    TmpDir { path: dir }
}
