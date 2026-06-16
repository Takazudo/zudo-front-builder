//! Integration tests for the `image_dimensions` feature.
//!
//! Tests verify that `ImageDimensionsPlugin`:
//!  - injects `width`/`height` on local images resolved against `public_dir`
//!    or the markdown source file's directory.
//!  - leaves `<img>` elements with explicit `width`/`height` unchanged.
//!  - skips remote (`http://`, `https://`) and `data:` URLs silently.
//!  - emits a warning diagnostic when a referenced file is missing or
//!    cannot be probed.
//!  - hits the cache on a second reference to the same file.
//!
//! Each test creates a temporary directory, copies the small binary fixtures
//! from `tests/fixtures/image_dimensions/`, and feeds the pipeline a
//! `BuildContext` with appropriate paths.

use std::path::PathBuf;

use zfb_md_ast::diagnostics::{CollectingSink, DiagnosticSeverity, MarkdownDiagnostic};
use zfb_md_ast::{BuildContext, HastNode, HastVisitor, ImageDimensionsConfig};
use zfb_md_extras::image_dimensions::ImageDimensionsPlugin;

const FIXTURES_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/image_dimensions"
);

// ── helpers ───────────────────────────────────────────────────────────────────

fn fixtures_path() -> PathBuf {
    PathBuf::from(FIXTURES_DIR)
}

fn img_node(src: &str) -> HastNode {
    HastNode::Element {
        tag: "img".to_string(),
        attrs: vec![
            ("src".to_string(), src.to_string()),
            ("alt".to_string(), "test".to_string()),
        ],
        children: vec![],
        void: true,
    }
}

fn root_with_img(src: &str) -> HastNode {
    HastNode::Root {
        children: vec![HastNode::Element {
            tag: "p".to_string(),
            attrs: vec![],
            children: vec![img_node(src)],
            void: false,
        }],
    }
}

fn get_attr<'a>(node: &'a HastNode, name: &str) -> Option<&'a str> {
    let HastNode::Element { attrs, .. } = node else {
        return None;
    };
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Extract the human-readable message from a generic diagnostic (the only
/// variant the image-dimensions plugin emits). Panics on any other variant so
/// a test that gets an unexpected diagnostic shape fails loudly.
fn diag_message(d: &MarkdownDiagnostic) -> &str {
    match d {
        MarkdownDiagnostic::Generic { message, .. } => message,
        other => panic!("expected a Generic diagnostic, got {other:?}"),
    }
}

fn first_img(root: &HastNode) -> &HastNode {
    let HastNode::Root { children } = root else {
        panic!("expected root")
    };
    let HastNode::Element {
        children: p_children,
        ..
    } = &children[0]
    else {
        panic!("expected p");
    };
    &p_children[0]
}

// ── acceptance: local PNG via public_dir ──────────────────────────────────────

/// `<img src="/sample-100x50.png">` with fixtures as public_dir → `width="100" height="50"`.
#[test]
fn injects_dimensions_png_via_public_dir() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-100x50.png");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), Some("100"), "PNG width must be 100");
    assert_eq!(get_attr(img, "height"), Some("50"), "PNG height must be 50");
}

/// Second format: JPEG (60x40).
#[test]
fn injects_dimensions_jpg_via_public_dir() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-60x40.jpg");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), Some("60"), "JPEG width must be 60");
    assert_eq!(
        get_attr(img, "height"),
        Some("40"),
        "JPEG height must be 40"
    );
}

/// Third format: GIF (80x30).
#[test]
fn injects_dimensions_gif_via_public_dir() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-80x30.gif");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), Some("80"), "GIF width must be 80");
    assert_eq!(get_attr(img, "height"), Some("30"), "GIF height must be 30");
}

/// Fourth format: WebP (120x90).
#[test]
fn injects_dimensions_webp_via_public_dir() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-120x90.webp");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        Some("120"),
        "WebP width must be 120"
    );
    assert_eq!(
        get_attr(img, "height"),
        Some("90"),
        "WebP height must be 90"
    );
}

/// Relative src resolved against the source file's directory.
#[test]
fn injects_dimensions_relative_src_via_source_dir() {
    let fixtures = fixtures_path();
    // The source file sits in the fixtures dir, so ./sample-100x50.png → fixtures/sample-100x50.png.
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("./sample-100x50.png");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.join(".."),
        fixtures.join("../public"),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), Some("100"), "relative PNG width");
    assert_eq!(get_attr(img, "height"), Some("50"), "relative PNG height");
}

/// Second PNG variant for wider coverage.
#[test]
fn injects_dimensions_wide_png() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/wide-200x150.png");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        Some("200"),
        "wide PNG width must be 200"
    );
    assert_eq!(
        get_attr(img, "height"),
        Some("150"),
        "wide PNG height must be 150"
    );
}

/// Tiny 1x1 PNG.
#[test]
fn injects_dimensions_tiny_png() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/tiny-1x1.png");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        Some("1"),
        "tiny PNG width must be 1"
    );
    assert_eq!(
        get_attr(img, "height"),
        Some("1"),
        "tiny PNG height must be 1"
    );
}

// ── acceptance: skip remote and data URLs ────────────────────────────────────

/// `<img src="https://example.com/foo.png">` must be left unchanged.
#[test]
fn skips_https_url() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("https://example.com/foo.png");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        None,
        "remote img must not get width"
    );
    assert_eq!(
        get_attr(img, "height"),
        None,
        "remote img must not get height"
    );
}

/// `<img src="http://example.com/foo.png">` must be left unchanged.
#[test]
fn skips_http_url() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("http://example.com/foo.png");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), None, "http img must not get width");
}

/// `<img src="data:image/png;base64,...">` must be left unchanged.
#[test]
fn skips_data_url() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("data:image/png;base64,iVBORw0KGgo=");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), None, "data: img must not get width");
}

// ── acceptance: explicit attrs left unchanged ────────────────────────────────

/// `<img width="...">` must not get a second `width` attribute.
#[test]
fn leaves_explicit_width_unchanged() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = HastNode::Root {
        children: vec![HastNode::Element {
            tag: "img".to_string(),
            attrs: vec![
                ("src".to_string(), "/sample-100x50.png".to_string()),
                ("width".to_string(), "999".to_string()),
            ],
            children: vec![],
            void: true,
        }],
    };
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let HastNode::Root { children } = &tree else {
        panic!()
    };
    let img = &children[0];
    let width_count = match img {
        HastNode::Element { attrs, .. } => attrs.iter().filter(|(k, _)| k == "width").count(),
        _ => panic!("expected element"),
    };
    assert_eq!(width_count, 1, "must not duplicate width attr");
    assert_eq!(
        get_attr(img, "width"),
        Some("999"),
        "original width must be preserved"
    );
    assert_eq!(
        get_attr(img, "height"),
        None,
        "height must not be injected when width present"
    );
}

/// `<img height="...">` must not get width or additional height attribute.
#[test]
fn leaves_explicit_height_unchanged() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = HastNode::Root {
        children: vec![HastNode::Element {
            tag: "img".to_string(),
            attrs: vec![
                ("src".to_string(), "/sample-100x50.png".to_string()),
                ("height".to_string(), "999".to_string()),
            ],
            children: vec![],
            void: true,
        }],
    };
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let HastNode::Root { children } = &tree else {
        panic!()
    };
    let img = &children[0];
    assert_eq!(
        get_attr(img, "width"),
        None,
        "width must not be injected when height present"
    );
    assert_eq!(
        get_attr(img, "height"),
        Some("999"),
        "original height must be preserved"
    );
}

// ── acceptance: diagnostic on missing file ────────────────────────────────────

/// A missing file must not panic — it must emit a Warning diagnostic and leave
/// the `<img>` unchanged.
#[test]
fn emits_warning_for_missing_file() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/nonexistent-file.png");
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(fixtures.join("fake-doc.mdx")),
        project_root: fixtures.clone(),
        public_dir: fixtures.clone(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert_eq!(diags.len(), 1, "must emit exactly one diagnostic");
    assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        None,
        "width must not be added for missing file"
    );
}

/// A non-image file (text) must emit a Warning diagnostic.
#[test]
fn emits_warning_for_non_image_file() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/not-an-image.txt");
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(fixtures.join("fake-doc.mdx")),
        project_root: fixtures.clone(),
        public_dir: fixtures.clone(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert_eq!(diags.len(), 1, "must emit exactly one diagnostic");
    assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
}

// ── acceptance: cache hit ──────────────────────────────────────────────────────

/// Two `<img>` elements referencing the same file should only read the file
/// once from disk — the second hit must come from cache.
#[test]
fn cache_hit_on_second_reference() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());

    // Build a tree with two <img> elements pointing at the same file.
    let mut tree = HastNode::Root {
        children: vec![
            img_node("/sample-100x50.png"),
            img_node("/sample-100x50.png"),
        ],
    };
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    assert_eq!(
        plugin.read_count(),
        1,
        "second reference to the same file must be a cache hit (read_count must be 1)"
    );

    // Both images must have dimensions injected.
    let HastNode::Root { children } = &tree else {
        panic!()
    };
    for (i, child) in children.iter().enumerate() {
        assert_eq!(
            get_attr(child, "width"),
            Some("100"),
            "img[{i}] width must be 100"
        );
        assert_eq!(
            get_attr(child, "height"),
            Some("50"),
            "img[{i}] height must be 50"
        );
    }
}

// ── acceptance: SVG dimensions (#1083) ────────────────────────────────────────

/// `<img src="/sample-svg-wh.svg">` with explicit `width`/`height` → injected.
#[test]
fn injects_dimensions_svg_explicit_wh() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-svg-wh.svg");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(get_attr(img, "width"), Some("120"), "SVG width must be 120");
    assert_eq!(get_attr(img, "height"), Some("80"), "SVG height must be 80");
}

/// An SVG with only a `viewBox` → dimensions taken from the viewBox.
#[test]
fn injects_dimensions_svg_viewbox_only() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-svg-viewbox.svg");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        Some("200"),
        "viewBox width must be 200"
    );
    assert_eq!(
        get_attr(img, "height"),
        Some("100"),
        "viewBox height must be 100"
    );
}

/// KiCad/Inkscape-style export: physical `mm` width/height alongside a
/// user-unit `viewBox` → falls back to the viewBox (aspect ratio preserved).
#[test]
fn injects_dimensions_svg_units_fall_back_to_viewbox() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-svg-units.svg");
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        Some("210"),
        "viewBox width must be 210"
    );
    assert_eq!(
        get_attr(img, "height"),
        Some("297"),
        "viewBox height must be 297"
    );
}

/// The core of #1083: an SVG with no determinable intrinsic dimensions must be
/// left unchanged and emit **zero** diagnostics — no more per-SVG warning noise.
#[test]
fn svg_without_dimensions_emits_no_warning() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/sample-svg-nodims.svg");
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(fixtures.join("fake-doc.mdx")),
        project_root: fixtures.clone(),
        public_dir: fixtures.clone(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    plugin.visit_with_context(&mut tree, &mut ctx);

    assert!(
        sink.take().is_empty(),
        "an undimensionable SVG must emit no diagnostics (#1083)"
    );
    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        None,
        "no width for dimensionless SVG"
    );
    assert_eq!(
        get_attr(img, "height"),
        None,
        "no height for dimensionless SVG"
    );
}

/// Two `<img>` referencing the same SVG must read it from disk only once — the
/// SVG path participates in the mtime cache and `read_count` instrumentation.
#[test]
fn svg_cache_hit_on_second_reference() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = HastNode::Root {
        children: vec![
            img_node("/sample-svg-wh.svg"),
            img_node("/sample-svg-wh.svg"),
        ],
    };
    let mut ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    plugin.visit_with_context(&mut tree, &mut ctx);

    assert_eq!(
        plugin.read_count(),
        1,
        "second SVG reference must hit the cache (read_count must be 1)"
    );
    let HastNode::Root { children } = &tree else {
        panic!()
    };
    for (i, child) in children.iter().enumerate() {
        assert_eq!(get_attr(child, "width"), Some("120"), "img[{i}] width");
        assert_eq!(get_attr(child, "height"), Some("80"), "img[{i}] height");
    }
}

// ── security: path-traversal containment guard (#1089) ────────────────────────
//
// The containment guard normalizes BOTH the candidate path and the configured
// root before comparing, so a `..` carried by the root no longer wrongly
// rejects a legitimately-contained image — see
// `injects_dimensions_relative_src_via_source_dir` above, whose `project_root`
// is `<fixtures>/..`. These tests assert the OTHER half of the contract: a
// crafted `src` that lexically resolves OUTSIDE the normalized root is still
// rejected with the existing warning and no dimensions injected.

/// A relative `src` escaping the project root via `../../` must be skipped with
/// a containment warning and must NOT inject width/height.
#[test]
fn relative_traversal_outside_root_rejected() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    // source dir = <fixtures>, project_root = <fixtures>. The src joins onto
    // the source dir as <fixtures>/../../etc/hosts, which normalizes to a path
    // well above <fixtures> — outside the (normalized) root, so it is rejected
    // before any disk probe.
    let mut tree = root_with_img("../../etc/hosts");
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(fixtures.join("fake-doc.mdx")),
        project_root: fixtures.clone(),
        public_dir: fixtures.clone(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert_eq!(diags.len(), 1, "traversal src must emit one warning");
    assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
    assert!(
        diag_message(&diags[0]).contains("resolves outside the expected root"),
        "warning must be the containment warning: {:?}",
        diag_message(&diags[0])
    );

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        None,
        "traversal src must not inject width"
    );
    assert_eq!(
        get_attr(img, "height"),
        None,
        "traversal src must not inject height"
    );
}

/// An absolute `src` escaping `public_dir` via `/../../` must be skipped — the
/// candidate normalizes to `/etc/hosts`, which is not under the normalized
/// public_dir.
#[test]
fn absolute_traversal_outside_public_dir_rejected() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    let mut tree = root_with_img("/../../etc/hosts");
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(fixtures.join("fake-doc.mdx")),
        project_root: fixtures.clone(),
        public_dir: fixtures.clone(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert_eq!(diags.len(), 1, "absolute traversal src must emit one warning");
    assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
    assert!(
        diag_message(&diags[0]).contains("resolves outside the expected root"),
        "warning must be the containment warning: {:?}",
        diag_message(&diags[0])
    );

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        None,
        "absolute traversal src must not inject width"
    );
}

/// Regression-pair for #1089: even when the configured root itself carries a
/// `..` (the scenario that wrongly rejected a contained image), a crafted `src`
/// that escapes the *normalized* root is STILL rejected. This proves the fix
/// (normalize both sides) did not weaken the traversal guard.
#[test]
fn traversal_still_rejected_when_root_has_dotdot() {
    let fixtures = fixtures_path();
    let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
    // project_root = <fixtures>/.. (normalizes to the parent of fixtures).
    // The src then climbs further out, escaping even the normalized root.
    let mut tree = root_with_img("../../../../../../etc/hosts");
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(fixtures.join("fake-doc.mdx")),
        project_root: fixtures.join(".."),
        public_dir: fixtures.join("../public"),
        heading_registry: None,
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert_eq!(
        diags.len(),
        1,
        "traversal must still warn even when root carries a .."
    );
    assert!(
        diag_message(&diags[0]).contains("resolves outside the expected root"),
        "warning must be the containment warning: {:?}",
        diag_message(&diags[0])
    );

    let img = first_img(&tree);
    assert_eq!(
        get_attr(img, "width"),
        None,
        "traversal src must not inject width even with a ..-root"
    );
}

// ── feature-disabled smoke test ───────────────────────────────────────────────

/// When `imageDimensions` is absent, plain `<img>` elements are passed through.
#[test]
fn disabled_leaves_img_unchanged() {
    use zfb_content::pipeline::Pipeline;
    use zfb_content::serializer::serialize;
    use zfb_md_extras::MarkdownFeaturesConfig;

    let features = MarkdownFeaturesConfig::default();
    let mut p = Pipeline::with_defaults_and_features(&features);
    let hast = p.run("![alt](./foo.png)\n").expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        !html.contains("width="),
        "disabled imageDimensions must not inject width: {html}"
    );
}
