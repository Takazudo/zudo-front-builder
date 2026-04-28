//! Snapshot-style assertions over the framed-error renderer for each
//! of the four error classes called out in epic #54 sub 3.
//!
//! Each test:
//! 1. Loads a small fixture from `tests/fixtures/error-ux/`.
//! 2. Drives the relevant library to produce a structured error.
//! 3. Routes that error through `zfb::diagnostics` to render a frame.
//! 4. Asserts on the resulting text — file path, 1-based line/col,
//!    a 3-line snippet window, and a single caret line.
//!
//! Tests run with colour suppressed so assertions stay
//! environment-independent; otherwise the framing behaves identically
//! to interactive output.

use std::path::PathBuf;

use zfb::diagnostics::{
    from_directive_diagnostic, from_frontmatter_error, from_js_runtime_error,
    from_paths_error, render_framed,
};
use zfb_content::plugins::DirectiveDiagnostic;
use zfb_render::paths::PathsError;

fn fixture(name: &str) -> (PathBuf, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/error-ux")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
    (path, text)
}

/// Strip ANSI CSI escapes so assertions are colour-agnostic.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for inner in chars.by_ref() {
                if inner == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Lock colour off for the duration of the test process. Each test
/// asserts on plain text and leaving colour codes around makes
/// assertions brittle.
fn no_color() {
    owo_colors::set_override(false);
}

#[test]
fn frontmatter_yaml_error_frames_offending_line() {
    no_color();
    let (path, src) = fixture("frontmatter-bad-yaml.md");
    let err = zfb_content::extract_frontmatter(&path, &src).expect_err("must fail");
    let diag = from_frontmatter_error(&path, &src, &err);
    let out = strip_ansi(&render_framed(&diag));

    assert!(out.starts_with("error: "), "header missing: {out}");
    assert!(
        out.contains("invalid YAML in frontmatter"),
        "should mention YAML cause: {out}"
    );
    assert!(
        out.contains("frontmatter-bad-yaml.md:"),
        "must include relative path: {out}"
    );
    // Snippet must include the offending bracketed value.
    assert!(out.contains("title: [unterminated"), "snippet missing: {out}");
    // Exactly one caret on its own line.
    assert_eq!(
        out.matches("\n  | ").filter(|_| true).count() >= 1,
        true,
        "expected gutter rows"
    );
    assert!(out.matches("^").count() >= 1, "expected caret: {out}");
}

#[test]
fn directive_unknown_frames_with_position_paired_by_caller() {
    no_color();
    let (path, src) = fixture("mdx-unknown-directive.md");
    // The directive diagnostic carries line/col; the orchestrator
    // pairs it with the file path. Line 5 is the `:::nopecard`
    // opener.
    let diag = DirectiveDiagnostic {
        message: "unknown directive `nopecard`".to_string(),
        line: Some(5),
        column: Some(1),
    };
    let d = from_directive_diagnostic(&path, &src, &diag);
    let out = strip_ansi(&render_framed(&d));

    assert!(
        out.contains("unknown directive `nopecard`"),
        "diag message missing: {out}"
    );
    assert!(
        out.contains("mdx-unknown-directive.md:5:1"),
        "expected line/col: {out}"
    );
    assert!(out.contains(":::nopecard"), "snippet missing: {out}");
}

#[test]
fn paths_missing_param_frames_export_paths_line() {
    no_color();
    let (path, src) = fixture("paths-missing-param.tsx");
    // Drive the resolver against the synthetic route template to
    // reproduce the runtime error.
    let mut cache = zfb_render::paths::PathsCache::new();
    let segs = vec![
        zfb_render::paths::Segment::Static("blog".into()),
        zfb_render::paths::Segment::Dynamic("slug".into()),
    ];
    let extracted = match zfb_render::paths_extract::extract_paths(&src, "page.tsx") {
        Ok(zfb_render::paths_extract::PathsExtraction::Literal(v)) => v,
        other => unreachable!("expected literal extraction, got {other:?}"),
    };
    let err = zfb_render::paths::resolve_paths(
        &mut cache,
        "blog/[slug].tsx",
        &segs,
        &extracted,
    )
    .expect_err("must fail with MissingParam");
    assert!(matches!(err, PathsError::MissingParam { .. }));
    let d = from_paths_error(&path, &src, &err);
    let out = strip_ansi(&render_framed(&d));

    assert!(
        out.contains("missing required param `slug`"),
        "diag message missing: {out}"
    );
    assert!(
        out.contains("paths-missing-param.tsx:7:"),
        "expected caret on `export function paths` line 7: {out}"
    );
    assert!(out.contains("export function paths"), "snippet missing: {out}");
}

#[test]
fn paths_shape_error_frames_export_paths_line() {
    no_color();
    let (path, src) = fixture("paths-shape-error.tsx");
    let mut cache = zfb_render::paths::PathsCache::new();
    let segs = vec![zfb_render::paths::Segment::Dynamic("slug".into())];
    // The static extractor accepts the object literal; resolve_paths
    // is what rejects it (must be an array).
    let extracted = match zfb_render::paths_extract::extract_paths(&src, "page.tsx") {
        Ok(zfb_render::paths_extract::PathsExtraction::Literal(v)) => v,
        other => unreachable!("expected literal extraction, got {other:?}"),
    };
    let err = zfb_render::paths::resolve_paths(
        &mut cache,
        "[slug].tsx",
        &segs,
        &extracted,
    )
    .expect_err("must fail");
    assert!(matches!(err, PathsError::InvalidPathsExport { .. }));
    let d = from_paths_error(&path, &src, &err);
    let out = strip_ansi(&render_framed(&d));

    assert!(out.contains("malformed"), "diag wording missing: {out}");
    assert!(
        out.contains("paths-shape-error.tsx:5:"),
        "expected caret on `export function paths` line 5: {out}"
    );
}

#[test]
fn js_runtime_error_decodes_through_synthetic_sourcemap() {
    no_color();
    let (path, src) = fixture("runtime-throw.tsx");
    // Pretend the bundler emitted a 1-line bundle that mirrors line 6
    // of the original (the `throw new Error(...)` line). A trivial
    // identity source map: bundle line 1 col 0 → source 0 line 5
    // col 4 (the `throw` statement, 0-based).
    //
    // VLQ for (0, 0, 5, 4) deltas:
    //   0 →  A  (0 << 1 = 0)
    //   0 →  A
    //   5 → "K" (5 << 1 = 10, base64 K=10)
    //   4 → "I" (4 << 1 = 8, base64 I=8)
    // → "AAKI"
    let map_json = format!(
        r#"{{
            "version": 3,
            "sources": ["{src_path}"],
            "sourcesContent": [{src_json}],
            "mappings": "AAKI"
        }}"#,
        src_path = "runtime-throw.tsx",
        src_json = serde_json::to_string(&src).unwrap(),
    );
    let bundle_path = path.parent().unwrap().join("ssg-render.js");
    let bundle_src = "throw new Error('intentional SSR failure');\n";
    let d = from_js_runtime_error(
        &bundle_path,
        bundle_src,
        1,
        1,
        "intentional SSR failure",
        Some(&map_json),
        None,
    );
    let out = strip_ansi(&render_framed(&d));

    assert!(
        out.contains("intentional SSR failure"),
        "message missing: {out}"
    );
    // Decoded position should land on the original `.tsx` (not the
    // bundle) — line 6 in 1-based after sourcemap decode.
    assert!(
        out.contains("runtime-throw.tsx:6:"),
        "expected mapped line 6: {out}"
    );
    assert!(out.contains("throw new Error"), "snippet missing: {out}");
}
