//! T1 coverage for `JsxNestedImageDimensions` (zfb#2248): markdown images
//! (`![…]`) nested inside MDX JSX now get `width`/`height` stamped at the
//! mdast phase, mirroring the hast-phase `ImageDimensionsPlugin`.
//!
//! Level 1/3 (logic + compiled-module output), same pattern as
//! `link_validation_jsx_descent_counts.rs` (exact-count/ordering pins over
//! `Pipeline::run_with_context`) and `zfb-content`'s
//! `link_validation_jsx_descent_mdx_path.rs` (the real MDX-compile path via
//! `mdx_to_jsx_module_with_pipeline`) — `zfb-md-extras` dev-deps on
//! `zfb-content`, so both patterns live here in one file.

use std::collections::HashMap;
use std::path::PathBuf;

use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_md_ast::{
    diagnostics::MarkdownDiagnostic, DirectiveSpec, ImageDimensionsConfig, LinkValidationConfig,
    MarkdownFeaturesConfig,
};

const FIXTURES_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/image_dimensions"
);

// ── Helpers ────────────────────────────────────────────────────────────────

fn fixtures_path() -> PathBuf {
    PathBuf::from(FIXTURES_DIR)
}

/// Pipeline with `imageDimensions` enabled, context roots armed onto the
/// fixtures dir (both `project_root` and `public_dir`, matching the
/// existing `image_dimensions.rs` integration suite's convention).
fn make_pipeline() -> Pipeline {
    let features = MarkdownFeaturesConfig {
        image_dimensions: Some(ImageDimensionsConfig::default()),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let fixtures = fixtures_path();
    p.set_build_context_roots(fixtures.clone(), fixtures);
    p
}

/// Same as [`make_pipeline`] plus a registered `"note"` container
/// directive (`:::note` → `<Note>`), for the directive-expansion test.
fn make_pipeline_with_note_directive() -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        image_dimensions: Some(ImageDimensionsConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let fixtures = fixtures_path();
    p.set_build_context_roots(fixtures.clone(), fixtures);
    p
}

/// Same as [`make_pipeline`] plus `linkValidation` and the `"note"`
/// directive — for the ordering pin (test 7).
fn make_pipeline_with_link_validation_and_note_directive() -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        image_dimensions: Some(ImageDimensionsConfig::default()),
        link_validation: Some(LinkValidationConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let fixtures = fixtures_path();
    p.set_build_context_roots(fixtures.clone(), fixtures);
    p
}

/// Full-config pipeline (GFM footnotes ON) with `imageDimensions` and the
/// `"note"` directive — the footnote fixtures need footnote syntax live,
/// which `with_defaults_and_features` does not enable (mirrors
/// `link_validation_jsx_descent_counts.rs`'s identical helper).
fn make_full_config_pipeline_with_note_directive() -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        image_dimensions: Some(ImageDimensionsConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::ALL_ON,
        None,
        false,
        false,
        Some(&features),
    )
    .expect("full-config pipeline must build");
    let fixtures = fixtures_path();
    p.set_build_context_roots(fixtures.clone(), fixtures);
    p
}

fn compile(pipeline: &mut Pipeline, md: &str) -> String {
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path(fixtures_path().join("fake-doc.mdx"));
    mdx_to_jsx_module_with_pipeline(md, opts, pipeline).expect("compile must succeed")
}

// ── 1. Markdown image inside `<Note>` gets stamped, correct attr order ─────

#[test]
fn markdown_image_inside_mdx_jsx_note_gets_dimensions_stamped_with_correct_attr_order() {
    let mut p = make_pipeline();

    // No title: src, alt, width, height.
    let jsx = compile(
        &mut p,
        "<Note>\n\n![alt text](/sample-100x50.png)\n\n</Note>\n",
    );
    assert!(
        jsx.contains(
            r#"<_components.img src="/sample-100x50.png" alt="alt text" width="100" height="50" />"#
        ),
        "expected a stamped nested img element with src/alt/width/height in order: {jsx}"
    );

    // With title: src, alt, title, width, height.
    let mut p2 = make_pipeline();
    let jsx2 = compile(
        &mut p2,
        "<Note>\n\n![alt text](/sample-100x50.png \"a title\")\n\n</Note>\n",
    );
    assert!(
        jsx2.contains(
            r#"<_components.img src="/sample-100x50.png" alt="alt text" title="a title" width="100" height="50" />"#
        ),
        "expected title inserted between alt and width: {jsx2}"
    );
}

// ── 2. Same inside `:::note` (directive-expanded) ───────────────────────────

#[test]
fn markdown_image_inside_directive_expanded_note_gets_dimensions_stamped() {
    let mut p = make_pipeline_with_note_directive();
    let jsx = compile(
        &mut p,
        ":::note\n\n![alt text](/sample-100x50.png)\n\n:::\n",
    );
    assert!(
        jsx.contains(
            r#"<_components.img src="/sample-100x50.png" alt="alt text" width="100" height="50" />"#
        ),
        "directive-expanded nested image must also be stamped — proves directives share the fix: {jsx}"
    );
}

// ── 3. Untreated cases stay byte-identical to today ─────────────────────────

#[test]
fn untreated_jsx_nested_cases_stay_byte_identical() {
    // Remote src (default skip_remote): no width/height, no diagnostic.
    let mut p = make_pipeline();
    let jsx = compile(
        &mut p,
        "<Note>\n\n![alt](https://example.com/foo.png)\n\n</Note>\n",
    );
    assert!(
        !jsx.contains("width="),
        "remote nested image must not get width: {jsx}"
    );
    assert!(
        p.take_markdown_diagnostics().is_empty(),
        "remote src must not warn"
    );

    // data: URL: no width/height, no diagnostic.
    let mut p = make_pipeline();
    let jsx = compile(
        &mut p,
        "<Note>\n\n![alt](data:image/png;base64,iVBORw0KGgo=)\n\n</Note>\n",
    );
    assert!(
        !jsx.contains("width="),
        "data: nested image must not get width: {jsx}"
    );
    assert!(
        p.take_markdown_diagnostics().is_empty(),
        "data: src must not warn"
    );

    // Missing file: no width/height, exactly one warning diagnostic.
    let mut p = make_pipeline();
    let jsx = compile(&mut p, "<Note>\n\n![alt](/does-not-exist.png)\n\n</Note>\n");
    assert!(
        !jsx.contains("width="),
        "missing file must not get width: {jsx}"
    );
    let diags = p.take_markdown_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "missing file must warn exactly once: {diags:?}"
    );

    // SVG without determinable intrinsic dims: no width/height, silent (#1083).
    let mut p = make_pipeline();
    let jsx = compile(
        &mut p,
        "<Note>\n\n![alt](/sample-svg-nodims.svg)\n\n</Note>\n",
    );
    assert!(
        !jsx.contains("width="),
        "undimensionable SVG must not get width: {jsx}"
    );
    assert!(
        p.take_markdown_diagnostics().is_empty(),
        "undimensionable SVG must stay silent (#1083)"
    );
}

// ── 4. Partition pins ────────────────────────────────────────────────────────

/// A top-level (non-JSX-nested) markdown image is unaffected by the new
/// mdast-phase pass — it is treated exactly once, by the existing
/// hast-phase `ImageDimensionsPlugin` only.
#[test]
fn top_level_image_output_unaffected_by_jsx_nested_pass() {
    let mut p = make_pipeline();
    let jsx = compile(&mut p, "![alt text](/sample-100x50.png)\n");
    assert_eq!(
        jsx.matches("width=\"100\"").count(),
        1,
        "top-level image must get width exactly once: {jsx}"
    );
    assert_eq!(
        jsx.matches("height=\"50\"").count(),
        1,
        "top-level image must get height exactly once: {jsx}"
    );
}

/// An ORDINARY (non-JSX-wrapped) image directly inside a footnote
/// definition body — even when the definition itself is authored inside
/// `<Note>` — is left untouched by this mdast pass (the `FootnoteDefinition`
/// boundary resets the JSX-nested driver) and is instead treated exactly
/// once by the rendered footnote section's hast-phase plugin. Mirrors
/// `link_validation_jsx_descent_counts.rs`'s
/// `footnote_definition_inside_jsx_with_ordinary_link_still_reports_exactly_one`.
#[test]
fn ordinary_image_in_footnote_definition_under_note_is_treated_exactly_once_by_rendered_section() {
    let mut p = make_full_config_pipeline_with_note_directive();
    let jsx = compile(
        &mut p,
        "ref[^fnord]\n\n<Note>\n\n[^fnord]: ![alt](/sample-100x50.png)\n\n</Note>\n",
    );
    assert_eq!(
        jsx.matches("width=\"100\"").count(),
        1,
        "an ordinary image in a footnote def body must be treated exactly once, \
         by the rendered footnote section's hast plugin: {jsx}"
    );
    assert_eq!(jsx.matches("height=\"50\"").count(), 1, "{jsx}");
}

// ── 5. Shared cache: read_count() == 1 across top-level + nested ───────────

/// The hast-phase `ImageDimensionsPlugin` and the mdast-phase
/// `JsxNestedImageDimensions` share one probe cache (`new_shared`,
/// zfb#2247): the SAME image referenced once top-level (hast) and once
/// JSX-nested (mdast) costs exactly one disk read. Driven directly against
/// both plugins (Level-1 logic) rather than through a full pipeline compile
/// — `read_count()` is plugin-internal instrumentation, not observable from
/// compiled JSX text alone.
#[test]
fn shared_cache_hits_once_for_top_level_and_nested_reference_to_same_file() {
    use markdown::mdast::{Image, MdxJsxFlowElement, Node as MdastNode, Paragraph, Root};
    use zfb_md_ast::{BuildContext, HastNode, HastVisitor, MdastVisitor};
    use zfb_md_extras::image_dimensions::{ImageDimensionsPlugin, JsxNestedImageDimensions};

    let fixtures = fixtures_path();
    let (mut hast_plugin, shared) =
        ImageDimensionsPlugin::new_shared(ImageDimensionsConfig::default());
    let mut mdast_plugin =
        JsxNestedImageDimensions::new(ImageDimensionsConfig::default(), shared, None);

    // Top-level hast <img> referencing the fixture.
    let mut hast_tree = HastNode::Root {
        children: vec![HastNode::Element {
            tag: "img".to_string(),
            attrs: vec![("src".to_string(), "/sample-100x50.png".to_string())],
            children: vec![],
            void: true,
        }],
    };
    let mut hast_ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    hast_plugin.visit_with_context(&mut hast_tree, &mut hast_ctx);

    // JSX-nested mdast Image referencing the SAME fixture.
    let mut mdast_tree = MdastNode::Root(Root {
        children: vec![MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
            children: vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Image(Image {
                    position: None,
                    alt: "alt".to_string(),
                    url: "/sample-100x50.png".to_string(),
                    title: None,
                })],
                position: None,
            })],
            position: None,
            name: Some("Note".to_string()),
            attributes: vec![],
        })],
        position: None,
    });
    let mut mdast_ctx = BuildContext::for_paths(
        fixtures.join("fake-doc.mdx"),
        fixtures.clone(),
        fixtures.clone(),
    );
    mdast_plugin.visit_with_context(&mut mdast_tree, &mut mdast_ctx);

    assert_eq!(
        hast_plugin.read_count(),
        1,
        "the same file referenced top-level AND JSX-nested must cost exactly one disk read"
    );

    // Both were still treated (sanity — the shared cache proof above is
    // meaningless if either side actually failed to inject).
    let HastNode::Root { children } = &hast_tree else {
        panic!("expected hast root")
    };
    let HastNode::Element { attrs, .. } = &children[0] else {
        panic!("expected element")
    };
    assert!(attrs.iter().any(|(k, v)| k == "width" && v == "100"));

    let MdastNode::Root(root) = &mdast_tree else {
        panic!("expected mdast root")
    };
    let MdastNode::MdxJsxFlowElement(note) = &root.children[0] else {
        panic!("expected Note element")
    };
    let MdastNode::Paragraph(p) = &note.children[0] else {
        panic!("expected paragraph")
    };
    assert!(
        matches!(&p.children[0], MdastNode::MdxJsxTextElement(_)),
        "the JSX-nested image must have been replaced with a synthesized element"
    );
}

// ── 6. JSX-in-JSX ─────────────────────────────────────────────────────────

#[test]
fn jsx_in_jsx_nested_image_is_treated_exactly_once() {
    let mut p = make_pipeline();
    let jsx = compile(
        &mut p,
        "<Note>\n\n<Box>\n\n![alt text](/sample-100x50.png)\n\n</Box>\n\n</Note>\n",
    );
    assert_eq!(
        jsx.matches("width=\"100\"").count(),
        1,
        "JSX-in-JSX nested image must be treated exactly once: {jsx}"
    );
    assert_eq!(jsx.matches("height=\"50\"").count(), 1, "{jsx}");
}

// ── 7. Ordering pin ──────────────────────────────────────────────────────────

/// A broken nested link BESIDE a treated nested image must still produce
/// its link-validation diagnostic — proves `jsx_nested_image_dimensions`
/// ran after `jsx_nested_link_collector` (the #2247 slot guarantee): had
/// the image-dimensions pass mutated the tree before the collector walked
/// it, nothing about the image's OWN candidate would be lost here (they
/// are siblings), but this pins the invariant end-to-end regardless.
#[test]
fn broken_nested_link_beside_treated_nested_image_still_diagnoses() {
    let mut p = make_pipeline_with_link_validation_and_note_directive();
    let jsx = compile(
        &mut p,
        "<Note>\n\n[broken](#missing-ordering-pin)\n\n![alt text](/sample-100x50.png)\n\n</Note>\n",
    );

    let diags = p.take_markdown_diagnostics();
    let broken: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-ordering-pin")
        })
        .collect();
    assert_eq!(
        broken.len(),
        1,
        "the sibling image being treated must not suppress the link's diagnostic \
         — proves jsx_nested_image_dimensions ran after jsx_nested_link_collector: {diags:?}"
    );
    assert!(
        jsx.contains(r#"width="100" height="50""#),
        "the neighboring image must still get stamped: {jsx}"
    );
}

// ── Bonus: heading-text regression guard (codex review finding) ────────────

/// A heading directly containing a JSX-nested image whose dimensions get
/// successfully stamped must NOT lose the image's `alt` text from the
/// heading's collected `text` (and therefore its slug) — codex review
/// flagged that `mdx_jsx_emit::mdast_inline_text` reads `Image.alt` but,
/// before this fix, treated ANY JSX element (including the synthesized
/// `<img>` replacement) as contributing no text, so a heading like `##
/// ![Foo](/existing.png) Bar` silently changed from `"Foo Bar"` to `"Bar"`
/// only when the file happened to exist and be probable.
#[test]
fn heading_with_treated_jsx_nested_image_keeps_alt_text_in_heading_export() {
    let mut p = make_pipeline();
    let jsx = compile(
        &mut p,
        "<Note>\n\n## ![Foo](/sample-100x50.png) Bar\n\n</Note>\n",
    );
    assert!(
        jsx.contains(r#"text: "Foo Bar""#),
        "the heading's collected text must still include the image's alt text \
         after dimension injection replaced the Image node: {jsx}"
    );
    // Sanity: the image itself was actually treated (dimensions present),
    // otherwise the assertion above would trivially pass via the
    // untreated-node's native `Image.alt` contribution instead of the
    // fix under test.
    assert!(
        jsx.contains(r#"width="100" height="50""#),
        "the heading's nested image must have been treated: {jsx}"
    );
}
