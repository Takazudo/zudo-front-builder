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

use zfb_md_ast::diagnostics::{CollectingSink, DiagnosticSeverity};
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
