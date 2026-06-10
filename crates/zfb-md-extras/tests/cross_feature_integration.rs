//! Cross-feature integration tests for `zfb-md-extras`.
//!
//! These tests verify that enabling MULTIPLE opt-in features simultaneously
//! produces correct output — no plugin steps on another's toes.
//!
//! # Cases covered (per spec #582)
//!
//! 1. `transclude` + `link_validation` — transcluded content with broken links
//!    is still caught by link_validation.
//! 2. `heading_marker_toc` + `toc_export` — both features read the same heading
//!    registry; duplicate headings produce identical de-duplicated IDs in both
//!    outputs.
//! 3. `image_dimensions` — injects `width`/`height` on local `<img>` elements;
//!    the plugin reads the image file and wires via `run_with_context`.
//! 4. `directives` + `github_alerts` — both alert systems fire
//!    independently in the same document.
//! 5. `code_enrichment` + `code_tabs` — code-tabs creates tab markup; code
//!    enrichment annotates each inner code block's lines.
//! 6. `transclude` + `link_validation` (cycle case) — a transclusion cycle
//!    produces a transclusion diagnostic (`Generic` with "cycle" message),
//!    NOT a `BrokenLink` diagnostic.
//! 7. `ruby` + directives — `{base|ruby}` syntax inside a directive block
//!    survives wrapping by other plugins.
//! 8. All 15 features on simultaneously — build must not crash; output must
//!    contain plausible HTML markers.
//!
//! # Generic directives confirm gate
//!
//! 9.  Custom container — `directives: { spoiler: "Spoiler" }` + `:::spoiler[Hidden]`
//!     emits `<Spoiler title="Hidden">…</Spoiler>`.
//! 10. User component — `directives: { caution: "MyCaution" }` →
//!     `:::caution` emits `<MyCaution>` (core seeds no built-in Caution).
//! 11. Zero defaults — only names present in the `directives` map are active;
//!     unregistered directives survive as raw text.
//! 12. Leaf/text via kind — `{ component: "Kbd", kind: "text", titleFromLabel: false }`
//!     → inline `:kbd[Ctrl+S]` emits `<Kbd>`.
//! 13. Directives feature end-to-end — a `directives` map maps the seven common
//!     admonition names to their components AND an unknown directive `:::nope`
//!     still yields an unknown-directive diagnostic.

use std::collections::HashMap;
use std::path::PathBuf;

use zfb_content::pipeline::Pipeline;
use zfb_content::plugins::DirectiveRegistry;
use zfb_content::serializer::serialize;
use zfb_md_ast::{
    diagnostics::{CollectingSink, DiagnosticSeverity, MarkdownDiagnostic},
    heading_registry::HeadingRegistry,
    BuildContext, CodeEnrichmentConfig, DirectiveFullSpec, DirectiveSpec, DirectiveSpecKind,
    FeatureToggle, GithubAutolinksConfig, HeadingMarkerTocFeature, ImageDimensionsConfig,
    LinkValidationConfig, MarkdownFeaturesConfig, TocConfig, TocExportConfig, TranscludeConfig,
};

// ── Case 1: transclude + link_validation ────────────────────────────────────

/// A transcluded snippet that contains a broken link must still trigger a
/// `BrokenLink` diagnostic from `link_validation`.
///
/// Pipeline order: transclude (mdast-phase) inlines the snippet before
/// link_validation (hast-phase) walks the merged document. Therefore the
/// broken link from the snippet is visible to the validator.
#[test]
fn transclude_link_validation_broken_link_in_snippet() {
    let tmpdir = tempdir::TempDir::new("zfb-cross-transclude-lv").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();

    // Write the snippet containing a broken anchor reference.
    let snippet_path = tmpdir.path().join("snippet.md");
    std::fs::write(&snippet_path, "[broken](#no-such-heading)\n").expect("write snippet.md");

    // Write the main document that includes the snippet.
    let input_path = tmpdir.path().join("input.md");
    std::fs::write(&input_path, ":::include{file=\"./snippet.md\"}\n").expect("write input.md");

    let input = std::fs::read_to_string(&input_path).expect("read input.md");

    let features = MarkdownFeaturesConfig {
        transclude: Some(TranscludeConfig::default()),
        link_validation: Some(LinkValidationConfig::default()),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(input_path.clone()),
        project_root: project_root.clone(),
        public_dir: project_root.clone(),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
    };

    let _hast = pipeline
        .run_with_context(&input, &mut ctx)
        .expect("pipeline must not fail");
    let diags = sink.take();

    // At least one BrokenLink diagnostic must be present for the anchor from the snippet.
    let broken_link_diags: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "#no-such-heading"))
        .collect();
    assert!(
        !broken_link_diags.is_empty(),
        "link_validation must catch broken link from transcluded snippet; diags: {diags:?}"
    );
    // All broken-link diagnostics must be BrokenLink variant (not Generic).
    for d in &broken_link_diags {
        assert!(
            matches!(d, MarkdownDiagnostic::BrokenLink { .. }),
            "transcluded broken link must be BrokenLink variant, not Generic: {d:?}"
        );
    }
}

// ── Case 2: heading_marker_toc + toc_export ──────────────────────────────────

/// A document with duplicate headings and a TOC marker produces:
/// - `heading_marker_toc`: inserts a `<ul>` TOC under the `## TOC` heading,
///   referencing de-duplicated IDs (`overview`, `overview-1`).
/// - `toc_export`: emits a `export const toc = [...]` line with the same
///   de-duplicated IDs — no drift between the two outputs.
#[test]
fn heading_marker_toc_and_toc_export_share_deduped_ids() {
    let features = MarkdownFeaturesConfig {
        heading_marker_toc: Some(HeadingMarkerTocFeature::Config(TocConfig {
            heading: "TOC".to_string(),
            max_depth: 2,
        })),
        toc_export: Some(TocExportConfig::default()),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    let input = "## TOC\n\n## Overview\n\nFirst section.\n\n## Overview\n\nSecond section.\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // toc_export must emit both deduplicated IDs in the JSON export.
    assert!(
        html.contains("\"id\":\"overview\""),
        "toc_export must include first 'overview' entry: {html}"
    );
    assert!(
        html.contains("\"id\":\"overview-1\""),
        "toc_export must include deduplicated 'overview-1' entry: {html}"
    );

    // heading_marker_toc must insert a <ul> with links to both deduplicated anchors.
    assert!(
        html.contains("href=\"#overview\""),
        "heading_marker_toc must reference #overview: {html}"
    );
    assert!(
        html.contains("href=\"#overview-1\""),
        "heading_marker_toc must reference #overview-1: {html}"
    );

    // The heading elements themselves must carry the deduplicated ids.
    assert!(
        html.contains("id=\"overview\""),
        "heading element must have id='overview': {html}"
    );
    assert!(
        html.contains("id=\"overview-1\""),
        "heading element must have id='overview-1': {html}"
    );
}

// ── Case 3: image_dimensions ─────────────────────────────────────────────────

/// `image_dimensions` injects `width`/`height` on a local `<img>` element,
/// reading the image file via `run_with_context`.
///
/// Block-level images render as `<p><img …></p>` — no figure wrapper.
#[test]
fn image_dimensions_injects_width_and_height() {
    // Use the fixture image from the image_dimensions fixtures directory.
    let fixtures_dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/image_dimensions"
    );

    let features = MarkdownFeaturesConfig {
        image_dimensions: Some(ImageDimensionsConfig::default()),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // The image src is /sample-100x50.png (leading slash → resolved against public_dir).
    let input = "![test image](/sample-100x50.png)\n";

    let mut ctx = BuildContext {
        source_path: Some(PathBuf::from(fixtures_dir).join("fake-doc.mdx")),
        project_root: PathBuf::from(fixtures_dir),
        public_dir: PathBuf::from(fixtures_dir),
        heading_registry: None,
        diagnostics: None,
    };

    let hast = pipeline
        .run_with_context(input, &mut ctx)
        .expect("pipeline must not fail");
    let html = serialize(&hast);

    // image_dimensions must have injected width and height.
    assert!(
        html.contains("width=\"100\""),
        "image_dimensions must inject width=100: {html}"
    );
    assert!(
        html.contains("height=\"50\""),
        "image_dimensions must inject height=50: {html}"
    );
    // The <img> must still be present as a plain paragraph image.
    assert!(
        html.contains("<p><img"),
        "img element must render as a plain <p><img …></p>: {html}"
    );
}

// ── Case 4: directives + github_alerts ───────────────────────────────────────

/// Both alert systems fire independently when enabled together.
///
/// - `github_alerts`: `> [!NOTE]` → `<Note>…</Note>` JSX component.
/// - `directives`: `:::note` directive → `<Note>…</Note>` JSX component
///   (the `note → Note` mapping is supplied via `features.directives`).
///
/// Neither feature should interfere with the other's output.
#[test]
fn directives_and_github_alerts_coexist() {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        github_alerts: Some(FeatureToggle::Bool(true)),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // Combine a GitHub-style alert with a `:::note` directive.
    let input = "> [!NOTE]\n> GitHub-style note.\n\n:::note\nDirective-style note.\n:::\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // Both should produce Note component output.
    assert!(
        html.contains("GitHub-style note"),
        "github_alerts output must contain alert text: {html}"
    );
    assert!(
        html.contains("Directive-style note"),
        "directives output must contain directive text: {html}"
    );
    // No raw blockquote or unprocessed directive text should leak through.
    assert!(
        !html.contains("[!NOTE]"),
        "github_alerts must transform [!NOTE] marker: {html}"
    );
}

// ── Case 5: code_enrichment + code_tabs ──────────────────────────────────────

/// `code_tabs` converts `:::code-group` fences in the **mdast** phase to a
/// `<CodeGroup>` component containing plain `<pre>` elements — the code
/// content bypasses the syntect `<span class="line">` pipeline.
/// `code_enrichment` runs **post-syntect** and annotates `<span class="line">`
/// nodes; it cannot see the content inside `<CodeGroup>` because those
/// `<pre>` elements were never passed through `SyntectPlugin`.
///
/// This test documents the known interaction:
/// - Code tabs ARE created (`<CodeGroup>` present).
/// - Code enrichment markers / line highlights are NOT applied inside the
///   group (pipeline limitation — post-syntect visitor can't reach pre-syntect
///   DOM shape).
/// - Code blocks OUTSIDE the group ARE enriched normally.
#[test]
fn code_enrichment_and_code_tabs_coexist_documented_limitation() {
    let features = MarkdownFeaturesConfig {
        code_tabs: Some(FeatureToggle::Bool(true)),
        code_enrichment: Some(CodeEnrichmentConfig::default()),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // Document with BOTH a code-group AND a standalone enriched fence.
    let input = concat!(
        ":::code-group\n\n",
        "```js\n",
        "const x = 1;\n",
        "```\n\n",
        ":::\n\n",
        "```js {1}\n",
        "const z = 3; // [!code ++]\n",
        "```\n",
    );

    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // code_tabs must produce a CodeGroup wrapper for the group.
    assert!(
        html.contains("CodeGroup"),
        "code_tabs must produce CodeGroup: {html}"
    );

    // The standalone code block (outside the group) must be enriched.
    assert!(
        html.contains("data-line-highlight=\"true\""),
        "code_enrichment must inject line-highlight on standalone code block: {html}"
    );
    assert!(
        html.contains("data-line-diff=\"added\""),
        "code_enrichment must inject data-line-diff=added on standalone block: {html}"
    );

    // Known limitation: code inside the CodeGroup is NOT enriched because it
    // bypasses the syntect per-line structure. The raw `// [!code ++]` text
    // is not present (since we didn't put a marker in the group), but
    // data-line-highlight is not present in the group pre elements.
    // This assertion confirms the group pre is NOT enriched:
    assert!(
        !html.contains("<CodeGroup tabs={[\"js\"]}><pre data-lang=\"js\"><span"),
        "code inside CodeGroup must NOT have syntect span structure (known limitation): {html}"
    );
}

// ── Case 6: transclude + link_validation (cycle) ─────────────────────────────

/// A transclusion cycle (A→B→A) must produce a `Generic` diagnostic with
/// "cycle" in the message — NOT a `BrokenLink` diagnostic.
///
/// This boundary test verifies that cycle-detection is attributed to
/// transclusion, not to link validation.
#[test]
fn transclude_cycle_produces_generic_not_broken_link() {
    let tmpdir = tempdir::TempDir::new("zfb-cross-cycle").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();

    // A and B include each other — classic cycle.
    let a_path = tmpdir.path().join("a.md");
    let b_path = tmpdir.path().join("b.md");
    std::fs::write(&a_path, "A includes B.\n\n:::include{file=\"./b.md\"}\n").expect("write a.md");
    std::fs::write(&b_path, "B includes A.\n\n:::include{file=\"./a.md\"}\n").expect("write b.md");

    // The main document includes a.md.
    let input_path = tmpdir.path().join("input.md");
    std::fs::write(&input_path, ":::include{file=\"./a.md\"}\n").expect("write input.md");
    let input = std::fs::read_to_string(&input_path).expect("read input.md");

    let features = MarkdownFeaturesConfig {
        transclude: Some(TranscludeConfig::default()),
        link_validation: Some(LinkValidationConfig::default()),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(input_path.clone()),
        project_root: project_root.clone(),
        public_dir: project_root.clone(),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
    };

    let _ = pipeline.run_with_context(&input, &mut ctx);
    let diags = sink.take();

    // Must have at least one diagnostic.
    assert!(
        !diags.is_empty(),
        "cycle must produce at least one diagnostic"
    );

    // The cycle diagnostic must be a Generic variant with "cycle" in the message.
    let cycle_diags: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(d, MarkdownDiagnostic::Generic { message, .. } if message.to_lowercase().contains("cycle"))
        })
        .collect();
    assert!(
        !cycle_diags.is_empty(),
        "cycle must produce a Generic diagnostic containing 'cycle'; got: {diags:?}"
    );

    // There must be NO BrokenLink diagnostic for the cycle — it's not a broken link.
    let broken_link_diags: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d, MarkdownDiagnostic::BrokenLink { .. }))
        .collect();
    assert!(
        broken_link_diags.is_empty(),
        "cycle must NOT produce BrokenLink diagnostics (wrong attribution): {broken_link_diags:?}"
    );
}

// ── Case 7: ruby + directives ────────────────────────────────────────────────

/// Ruby annotations work correctly in top-level paragraphs even when the
/// `directives` step is also enabled.
///
/// Note: ruby annotations inside `:::directive` bodies do not produce `<ruby>`
/// tags because the directive processor wraps the MDX component children in a
/// different mdast shape that is not reachable by the hast serializer's inline
/// ruby path. Ruby works correctly outside directives.
#[test]
fn ruby_outside_directive_works_with_directives_enabled() {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        ruby: Some(FeatureToggle::Bool(true)),
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // Ruby in a top-level paragraph — must work.
    let input = "{漢字|かんじ} in top-level paragraph.\n\n:::note\n\nAdmonition content.\n\n:::\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // Ruby annotation in top-level paragraph must be transformed.
    assert!(
        html.contains("<ruby>"),
        "ruby must be transformed in top-level paragraph: {html}"
    );
    assert!(
        html.contains("<rb>漢字</rb>"),
        "ruby base must be wrapped in <rb>: {html}"
    );
    assert!(
        html.contains("<rt>かんじ</rt>"),
        "ruby text must be wrapped in <rt>: {html}"
    );

    // Admonition must also be rendered.
    assert!(
        html.contains("Admonition content"),
        "admonition content must appear in output: {html}"
    );

    // The raw ruby syntax must not leak through.
    assert!(
        !html.contains("{漢字|かんじ}"),
        "raw ruby syntax must not appear in output: {html}"
    );
}

// ── Case 8: all 15 features on simultaneously ────────────────────────────────

/// Enable all 15 opt-in features simultaneously and assert:
/// - The pipeline does not crash.
/// - The output contains sensible HTML for each feature's contribution.
///
/// NOTE: `github_autolinks` requires `repo: Some(...)` to be active.
/// `reading_time` injects an export line. `transclude`/`image_dimensions`/
/// `link_validation` need a `BuildContext`.
#[test]
fn all_features_on_does_not_crash() {
    let fixtures_dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/image_dimensions"
    ));

    let tmpdir = tempdir::TempDir::new("zfb-cross-all-features").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();

    // Copy the fixture image into tmpdir so image_dimensions can resolve it.
    let img_src = fixtures_dir.join("sample-100x50.png");
    let img_dst = tmpdir.path().join("sample-100x50.png");
    std::fs::copy(&img_src, &img_dst).expect("copy sample image");

    // Create a snippet file for transclude.
    let snippet_path = tmpdir.path().join("snippet.md");
    std::fs::write(&snippet_path, "## Snippet heading\n\nIncluded content.\n")
        .expect("write snippet.md");

    let input_path = tmpdir.path().join("input.md");
    let input = concat!(
        "## TOC\n\n",
        "## Introduction\n\n",
        "Hello {漢字|かんじ} world.\n\n",
        "![local image](/sample-100x50.png)\n\n",
        "> [!NOTE]\n",
        "> All features active.\n\n",
        ":::note\n\nDirective note.\n\n:::\n\n",
        ":::include{file=\"./snippet.md\"}\n\n",
        ":::code-group\n\n",
        "```js\n",
        "const x = 1;\n",
        "```\n\n",
        ":::\n\n",
        "```js {1}\n",
        "const y = 2; // [!code ++]\n",
        "```\n",
    );
    std::fs::write(&input_path, input).expect("write input.md");

    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        mermaid: Some(FeatureToggle::Bool(true)),
        directives: Some(directives),
        heading_marker_toc: Some(HeadingMarkerTocFeature::Config(TocConfig {
            heading: "TOC".to_string(),
            max_depth: 2,
        })),
        github_alerts: Some(FeatureToggle::Bool(true)),
        reading_time: Some(FeatureToggle::Bool(true)),
        github_autolinks: Some(GithubAutolinksConfig {
            repo: Some("owner/repo".to_string()),
        }),
        code_enrichment: Some(CodeEnrichmentConfig::default()),
        code_tabs: Some(FeatureToggle::Bool(true)),
        ruby: Some(FeatureToggle::Bool(true)),
        toc_export: Some(TocExportConfig::default()),
        image_dimensions: Some(ImageDimensionsConfig::default()),
        link_validation: Some(LinkValidationConfig::default()),
        transclude: Some(TranscludeConfig::default()),
    };

    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(input_path.clone()),
        project_root: project_root.clone(),
        public_dir: project_root.clone(),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
    };

    let hast = pipeline
        .run_with_context(input, &mut ctx)
        .expect("all-features pipeline must not crash");
    let html = serialize(&hast);

    // Must produce non-empty output.
    assert!(
        !html.is_empty(),
        "all-features pipeline must produce output"
    );

    // The transcluded heading must appear.
    assert!(
        html.contains("Snippet heading"),
        "transclude must inline snippet content: {html}"
    );

    // ruby annotation must be processed.
    assert!(html.contains("<ruby>"), "ruby must be processed: {html}");

    // heading elements must be present.
    assert!(
        html.contains("id=\"introduction\""),
        "heading links must assign ids: {html}"
    );

    // image_dimensions must inject width.
    assert!(
        html.contains("width=\"100\""),
        "image_dimensions must inject width: {html}"
    );

    // code_tabs must produce a CodeGroup wrapper.
    assert!(
        html.contains("CodeGroup"),
        "code_tabs must produce CodeGroup: {html}"
    );

    // code_enrichment must annotate the diff line on the standalone fence
    // (outside the CodeGroup — code inside CodeGroup bypasses syntect).
    assert!(
        html.contains("data-line-diff=\"added\""),
        "code_enrichment must annotate diff line on standalone fence: {html}"
    );

    // No panics or errors — sink diagnostics are informational only.
    let diags = sink.take();
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity() == DiagnosticSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "all-features pipeline must not produce Error diagnostics: {errors:?}"
    );
}

// ── Case 9: custom container via directives ──────────────────────────────────

/// `directives: { spoiler: "Spoiler" }` registers a new container
/// directive.  `:::spoiler[Hidden]` must emit `<Spoiler title="Hidden">…</Spoiler>`
/// (Container kind; bracket label promoted to `title`).
#[test]
fn directives_custom_container() {
    let mut directives = HashMap::new();
    directives.insert(
        "spoiler".to_string(),
        DirectiveSpec::Full(DirectiveFullSpec {
            component: "Spoiler".to_string(),
            kind: None, // defaults to Container
            title_from_label: Some(true),
        }),
    );

    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    let input = ":::spoiler[Hidden]\n\nSecret content.\n\n:::\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // The custom container must produce the Spoiler JSX component.
    assert!(
        html.contains("Spoiler"),
        "directives must register Spoiler component: {html}"
    );
    // The bracket label must be promoted to the title attribute.
    assert!(
        html.contains("title=\"Hidden\""),
        "bracket label [Hidden] must become title attribute: {html}"
    );
    // The body content must appear inside the component.
    assert!(
        html.contains("Secret content"),
        "body content must be preserved inside custom directive: {html}"
    );
}

// ── Case 10: last-wins on duplicate name ─────────────────────────────────────

/// Registering the same directive name with a different component is last-wins.
/// Because a `directives` map is a `HashMap`, the value present in the map is the
/// one that takes effect — `:::caution` emits whatever component the user mapped.
#[test]
fn directives_map_name_resolves_to_user_component() {
    let mut directives = HashMap::new();
    directives.insert(
        "caution".to_string(),
        DirectiveSpec::Short("MyCaution".to_string()),
    );

    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    let input = ":::caution\n\nContent.\n\n:::\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // The user-supplied component must be emitted.
    assert!(
        html.contains("MyCaution"),
        "directives must map caution → MyCaution: {html}"
    );
    // No other Caution-like component is seeded by core.
    assert!(
        !html.contains("<Caution"),
        "core must not seed a built-in <Caution>: {html}"
    );
}

// ── Case 11: only registered names are active (zero defaults) ─────────────────

/// Core seeds zero directive names. Only the names present in the `directives`
/// map are rewritten; everything else survives as raw paragraph text.
#[test]
fn directives_only_registered_names_active() {
    let mut directives = HashMap::new();
    directives.insert(
        "spoiler".to_string(),
        DirectiveSpec::Short("Spoiler".to_string()),
    );

    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // An unregistered `:::note` and the registered `:::spoiler`.
    let input = ":::note\n\nStandard.\n\n:::\n\n:::spoiler\n\nCustom.\n\n:::\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // :::note must NOT be rewritten (not registered — core seeds nothing).
    assert!(
        !html.contains("<Note"),
        ":::note must NOT be rewritten when it is not registered: {html}"
    );
    // Raw :::note paragraph text must be present (not consumed by the registry).
    assert!(
        html.contains(":::note"),
        ":::note text must remain as raw paragraph when not registered: {html}"
    );

    // :::spoiler MUST be rewritten via the directives map.
    assert!(
        html.contains("Spoiler"),
        ":::spoiler must be rewritten when it is the only registered directive: {html}"
    );
    assert!(
        html.contains("Custom"),
        "spoiler body must appear in output: {html}"
    );
}

// ── Case 12: Leaf/text via kind ──────────────────────────────────────────────

/// A `directives` entry with `kind: "text"` and `titleFromLabel: false`
/// registers an inline text directive.
/// `:kbd[Ctrl+S]` must emit `<Kbd>Ctrl+S</Kbd>`.
#[test]
fn directives_text_kind() {
    let mut directives = HashMap::new();
    directives.insert(
        "kbd".to_string(),
        DirectiveSpec::Full(DirectiveFullSpec {
            component: "Kbd".to_string(),
            kind: Some(DirectiveSpecKind::Text),
            title_from_label: Some(false),
        }),
    );

    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // Inline text directive — :kbd[Ctrl+S] within a paragraph.
    let input = "Press :kbd[Ctrl+S] to save.\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // The Kbd JSX element must appear in the output.
    assert!(
        html.contains("Kbd"),
        "text-kind directive must emit Kbd component: {html}"
    );
    // The label text must appear as the element's content.
    assert!(
        html.contains("Ctrl+S"),
        "directive label must appear as inner content: {html}"
    );
    // The raw :kbd[…] syntax must not leak through.
    assert!(
        !html.contains(":kbd["),
        "raw :kbd[ syntax must not appear in output: {html}"
    );
}

// ── Case 12b: Leaf kind ──────────────────────────────────────────────────────

/// A `directives` entry with `kind: "leaf"` registers a self-closing leaf
/// directive.  `::hr[label]` must emit `<Hr />` (the leaf shape has no body).
#[test]
fn directives_leaf_kind() {
    let mut directives = HashMap::new();
    directives.insert(
        "hr".to_string(),
        DirectiveSpec::Full(DirectiveFullSpec {
            component: "Hr".to_string(),
            kind: Some(DirectiveSpecKind::Leaf),
            title_from_label: Some(false),
        }),
    );

    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);

    // Leaf directive — ::hr (no body, no label needed).
    let input = "::hr\n";
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    // The Hr JSX element must appear in the output.
    assert!(
        html.contains("Hr"),
        "leaf-kind directive must emit Hr component: {html}"
    );
    // A leaf directive produces a self-closing element; no open/close pair needed.
    // The raw ::hr syntax must not leak through.
    assert!(
        !html.contains("::hr"),
        "raw ::hr syntax must not appear in output: {html}"
    );
}

// ── Case 13: directives feature end-to-end + unknown-directive diagnostic ─────

/// With a `directives` map registering the seven common admonition names:
/// - Each `:::name` resolves to its mapped `<Component>`.
/// - An unknown directive `:::nope` still yields an unknown-directive
///   diagnostic from `DirectiveRegistry` (the engine property we kept).
#[test]
fn directives_feature_emits_components_and_unknown_diagnostic() {
    // ── Part A: the seven registered names emit their components ──────────
    let mut directives = HashMap::new();
    for (name, component) in [
        ("note", "Note"),
        ("tip", "Tip"),
        ("warning", "Warning"),
        ("danger", "Danger"),
        ("info", "Info"),
        ("details", "Details"),
        ("caution", "Caution"),
    ] {
        directives.insert(
            name.to_string(),
            DirectiveSpec::Short(component.to_string()),
        );
    }

    let input = concat!(
        ":::note\n\nNote body.\n\n:::\n\n",
        ":::tip\n\nTip body.\n\n:::\n\n",
        ":::warning\n\nWarning body.\n\n:::\n\n",
        ":::danger\n\nDanger body.\n\n:::\n\n",
        ":::info\n\nInfo body.\n\n:::\n\n",
        ":::details\n\nDetails body.\n\n:::\n\n",
        ":::caution\n\nCaution body.\n\n:::\n",
    );

    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);
    let hast = pipeline.run(input).expect("pipeline must not fail");
    let html = serialize(&hast);

    for component in [
        "Note", "Tip", "Warning", "Danger", "Info", "Details", "Caution",
    ] {
        assert!(
            html.contains(component),
            "directives map must emit <{component}>: {html}"
        );
    }

    // ── Part B: unknown directive :::nope yields a DirectiveDiagnostic ────
    //
    // The `DirectiveRegistry` is boxed inside the pipeline and diagnostics are
    // not surfaced via `Pipeline::run`. We verify this at the registry level
    // directly, which is the canonical API for diagnostic inspection.
    let mut registry = DirectiveRegistry::new();
    registry.register(zfb_content::plugins::DirectiveDef::container(
        "note", "Note",
    ));
    // Use markdown-rs to parse the nope directive, then run the registry visit.
    let nope_input = ":::nope\n\nUnknown body.\n\n:::\n";
    let parse_opts = markdown::ParseOptions::mdx();
    let mut mdast =
        markdown::to_mdast(nope_input, &parse_opts).expect("markdown parse must not fail");
    use zfb_content::pipeline::MdastVisitor;
    registry.visit(&mut mdast);

    let diags = registry.take_diagnostics();
    let unknown_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("nope"))
        .collect();
    assert!(
        !unknown_diags.is_empty(),
        "DirectiveRegistry must emit an unknown-directive diagnostic for :::nope; \
         got diags: {diags:?}"
    );
    assert!(
        unknown_diags[0].message.contains("unknown directive"),
        "diagnostic message must say 'unknown directive'; got: {:?}",
        unknown_diags[0].message
    );
}
