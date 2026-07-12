//! Integration tests for the wasm-safe pipeline facade (zfb#1574).
//!
//! Test plan (declared per project testing discipline):
//! - **Level**: 1 (unit/logic) — every assertion here is a pure
//!   string/struct comparison against in-memory pipeline output. No DOM,
//!   no build artifact, no process/browser.
//! - **Why level 1**: the facade's contract is "config JSON in, string
//!   out" — a pure function boundary. There is no UI/visual surface, so
//!   escalating past level 1 (per the project's testing ladder) would not
//!   exercise anything this change touches.
//! - **What's covered**: (a) `PipelineOptions` JSON deserialization for
//!   every pure plugin named in the issue's acceptance criteria; (b) the
//!   `render_html` composition matches the equivalent hand-assembled
//!   `pipeline.run()` + `serialize()` path; (c) the frontmatter
//!   filename-wrapper's parsed-value/body/body_offset correctness on both
//!   a with-frontmatter and a without-frontmatter fixture; (d) the three
//!   fs-bound plugins (`transclude`, `imageDimensions`, `linkValidation`)
//!   are registered but produce no error, no diagnostics, and no output
//!   mutation when pointed at nonexistent paths — proving they never
//!   touch the filesystem through this facade.
//! - **Blind spots** (explicitly NOT covered here): wasm-target
//!   compilation (crate still builds for the host triple only — that's
//!   the sibling `zfb-md-wasm` crate's job in a later wave); performance
//!   characteristics of rebuilding a `Pipeline` per call; the syntect
//!   `fancy-regex` backend swap (a sibling sub-issue, zfb#1573).

use zfb_content::facade::{
    build_pipeline, build_pipeline_from_json, compile_mdx_jsx_from_config, parse_pipeline_options,
    render_html, render_html_from_config, render_mdx_jsx_module, GfmOptions, PipelineOptions,
};
use zfb_content::frontmatter::extract_from_filename;
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::serializer::serialize;

// ── PipelineOptions JSON shape ──────────────────────────────────────────────

#[test]
fn pipeline_options_default_matches_bare_full_config_constructor() {
    let options = PipelineOptions::default();
    let mut from_facade = build_pipeline(&options);
    let mut from_bare = Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false,
        None,
    )
    .expect("no themes_dir — cannot fail");

    let input = "# Title\n\nSome *text* with `code` and a [link](https://example.com).\n";
    let facade_html = render_html(&mut from_facade, input).expect("facade render");
    let bare_html = serialize(&from_bare.run(input).expect("bare pipeline run"));
    assert_eq!(
        facade_html, bare_html,
        "PipelineOptions::default() must reproduce the bare with_defaults_and_full_config shape"
    );
}

#[test]
fn empty_json_object_deserializes_to_defaults() {
    let options = parse_pipeline_options("{}").expect("empty object is valid PipelineOptions");
    assert_eq!(options.theme, None);
    assert_eq!(options.gfm, GfmOptions::default());
    assert!(options.cjk_friendly);
    assert!(!options.hard_breaks);
    assert_eq!(
        options.features,
        zfb_content::MarkdownFeaturesConfig::default()
    );
}

#[test]
fn unknown_top_level_field_is_rejected() {
    let err = parse_pipeline_options(r#"{"bogus": true}"#).expect_err("must reject unknown field");
    let msg = err.to_string();
    assert!(
        msg.contains("bogus") || msg.contains("unknown field"),
        "error should name the unknown field, got: {msg}"
    );
}

#[test]
fn gfm_options_json_round_trips_into_resolved_constructs() {
    let options = parse_pipeline_options(
        r#"{
            "gfm": {
                "strikethrough": false,
                "table": true,
                "autolinkLiteral": true,
                "taskListItem": true,
                "footnoteDefinition": true
            }
        }"#,
    )
    .expect("valid gfm shape");
    let resolved: ResolvedGfmConstructs = options.gfm.into();
    assert!(!resolved.strikethrough);
    assert!(resolved.table);
    assert!(resolved.autolink_literal);
    assert!(resolved.task_list_item);
    assert!(resolved.footnote_definition);
}

// ── render_html matches the equivalent existing test-composed path ─────────

#[test]
fn render_html_matches_manual_run_and_serialize_composition() {
    let config_json = r#"{
        "theme": "InspiredGitHub",
        "cjkFriendly": true,
        "hardBreaks": false,
        "gfm": { "strikethrough": true, "table": true },
        "features": { "mermaid": true, "codeEnrichment": {} }
    }"#;
    let input = "## Heading\n\n\
        これは**重要**な機能です\n\n\
        ```mermaid\n\
        graph TD\n\
        A-->B\n\
        ```\n\n\
        ```js\n\
        const a = 1; // [!code ++]\n\
        ```\n";

    let mut via_facade =
        build_pipeline_from_json(config_json).expect("config JSON builds a pipeline");
    let facade_html = render_html(&mut via_facade, input).expect("facade render");

    // Hand-assemble the equivalent pipeline the same way
    // `tests/integration_pipeline.rs::render_fixture_with` does: build
    // directly via `Pipeline::with_defaults_and_full_config`, call
    // `pipeline.run(input)`, then `serializer::serialize` — the exact
    // composition `facade::render_html` promotes to a public function.
    let features = zfb_content::MarkdownFeaturesConfig {
        mermaid: Some(zfb_content::FeatureToggle::Bool(true)),
        code_enrichment: Some(zfb_md_extras::CodeEnrichmentConfig::default()),
        ..Default::default()
    };
    let mut manual = Pipeline::with_defaults_and_full_config(
        Some("InspiredGitHub"),
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false,
        Some(&features),
    )
    .expect("no themes_dir — cannot fail");
    let manual_hast = manual.run(input).expect("manual pipeline run");
    let manual_html = serialize(&manual_hast);

    assert_eq!(
        facade_html, manual_html,
        "facade::render_html must byte-match the hand-composed run()+serialize() path"
    );
    // Sanity: both plugins actually fired, so this comparison isn't
    // vacuously trivial.
    assert!(facade_html.contains(r#"class="mermaid""#));
    assert!(facade_html.contains("data-line-diff=\"added\""));
}

// ── Per-feature coverage, driven entirely from JSON config ─────────────────

fn render(config_json: &str, input: &str) -> String {
    render_html_from_config(config_json, input).unwrap_or_else(|e| {
        panic!("render_html_from_config failed for config {config_json:?} / input {input:?}: {e}")
    })
}

#[test]
fn github_alerts_from_json_config() {
    let html = render(
        r#"{"features": {"githubAlerts": true}}"#,
        "> [!NOTE]\n> Something worth noting.\n",
    );
    assert!(html.contains("<Note"), "got: {html}");
}

#[test]
fn code_tabs_from_json_config() {
    let input = ":::code-group\n\n\
        ```ts title=\"index.ts\"\n\
        const x = 1;\n\
        ```\n\n\
        ```js title=\"index.js\"\n\
        const y = 2;\n\
        ```\n\n\
        :::\n";
    let html = render(r#"{"features": {"codeTabs": true}}"#, input);
    assert!(html.contains("<CodeGroup"), "got: {html}");
}

#[test]
fn ruby_from_json_config() {
    let html = render(r#"{"features": {"ruby": true}}"#, "{これは}^{これ}\n");
    assert!(
        html.contains("<ruby><rb>これは</rb><rt>これ</rt></ruby>"),
        "got: {html}"
    );
}

#[test]
fn directives_from_json_config() {
    let html = render(
        r#"{"features": {"directives": {"note": "Note"}}}"#,
        ":::note\nBody text.\n:::\n",
    );
    assert!(html.contains("<Note"), "got: {html}");
}

#[test]
fn cjk_emphasis_from_json_config() {
    let html = render(r#"{"cjkFriendly": true}"#, "これは**重要**な機能です\n");
    assert!(
        html.contains("<p>これは<strong>重要</strong>な機能です</p>"),
        "got: {html}"
    );
}

#[test]
fn heading_links_are_always_on() {
    let html = render("{}", "## Hello World\n");
    assert!(html.contains(r#"id="hello-world""#), "got: {html}");
}

#[test]
fn toc_export_from_json_config() {
    let html = render(
        r#"{"features": {"tocExport": {"maxDepth": 3}}}"#,
        "## Intro\n\nBody text.\n",
    );
    assert!(html.contains("export const toc"), "got: {html}");
    assert!(html.contains(r#""id":"intro""#), "got: {html}");
}

#[test]
fn reading_time_from_json_config_via_jsx_tier() {
    // Reading-time is emitted as an MdxjsEsm export appended to the mdast
    // Root; the HTML serializer path drops unhandled mdast node types
    // (including MdxjsEsm — see `mdast_to_hast_inner`'s wildcard arm), so
    // this feature is only observable through the JSX/MDX compile tier.
    let mut pipeline = build_pipeline_from_json(r#"{"features": {"readingTime": {"wpm": 1}}}"#)
        .expect("valid config");
    let jsx = render_mdx_jsx_module(
        &mut pipeline,
        "This is a short paragraph with several words in it.\n",
        "reading-time.mdx",
    )
    .expect("mdx compiles");
    assert!(
        jsx.contains("export const readingTimeMinutes ="),
        "got: {jsx}"
    );
}

#[test]
fn code_enrichment_diff_marker_from_json_config() {
    let input = "```js\nconst a = 1; // [!code ++]\n```\n";
    let html = render(r#"{"features": {"codeEnrichment": {}}}"#, input);
    assert!(html.contains(r#"data-line-diff="added""#), "got: {html}");
}

#[test]
fn code_enrichment_line_highlight_from_json_config() {
    let input = "```js {1}\nconst a = 1;\nconst b = 2;\n```\n";
    let html = render(r#"{"features": {"codeEnrichment": {}}}"#, input);
    assert!(
        html.contains(r#"data-line-highlight="true""#),
        "got: {html}"
    );
}

#[test]
fn mermaid_marker_swap_from_json_config() {
    let input = "```mermaid\ngraph TD\nA-->B\n```\n";
    let html = render(r#"{"features": {"mermaid": true}}"#, input);
    assert!(html.contains(r#"class="mermaid""#), "got: {html}");
    assert!(html.contains("data-mermaid"), "got: {html}");
    assert!(
        !html.contains("<pre"),
        "mermaid pre must be fully replaced; got: {html}"
    );
}

#[test]
fn github_autolinks_exercises_repo_config_string() {
    let html = render(
        r#"{"features": {"githubAutolinks": {"repo": "owner/repo"}}}"#,
        "See #123 for details.\n",
    );
    assert!(
        html.contains(r#"href="https://github.com/owner/repo/issues/123""#),
        "got: {html}"
    );
}

#[test]
fn github_autolinks_without_repo_reports_config_error_not_a_panic() {
    let mut pipeline =
        build_pipeline_from_json(r#"{"features": {"githubAutolinks": {}}}"#).expect("valid json");
    let html = render_html(&mut pipeline, "See #123.\n").expect("run still succeeds");
    // No repo configured -> no autolink rewrite; the plugin never wires in,
    // and `register_features` records a build-blocking config-error
    // diagnostic instead (#1392) rather than panicking or silently
    // guessing a repo.
    assert!(!html.contains("https://github.com/"), "got: {html}");
    let diagnostics = pipeline.take_markdown_diagnostics();
    assert!(
        !diagnostics.is_empty(),
        "missing repo must surface a diagnostic"
    );
}

// ── fs-bound plugins: registered but inert ──────────────────────────────────

#[test]
fn fs_bound_plugins_are_registered_but_inert() {
    let config_json = r#"{
        "features": {
            "transclude": {},
            "imageDimensions": {},
            "linkValidation": {}
        }
    }"#;
    let input = "\
        :::include{file=\"./does-not-exist-1574.md\"}\n\n\
        ![alt text](./does-not-exist-1574.png)\n\n\
        [broken link](./does-not-exist-1574.md)\n";

    let mut pipeline = build_pipeline_from_json(config_json).expect("valid config");
    let html = render_html(&mut pipeline, input)
        .expect("no filesystem access is attempted, so a nonexistent path never errors");

    // transclude: context-free `visit` is a no-op — the `:::include{...}`
    // paragraph text survives completely untouched.
    assert!(
        html.contains(":::include{file=&quot;./does-not-exist-1574.md&quot;}")
            || html.contains(":::include{file=\"./does-not-exist-1574.md\"}"),
        "transclude directive text should survive unprocessed; got: {html}"
    );
    // imageDimensions: context-free `visit` is a no-op — no width/height
    // attributes are injected on the <img>.
    assert!(html.contains("does-not-exist-1574.png"), "got: {html}");
    assert!(!html.contains("width="), "got: {html}");
    assert!(!html.contains("height="), "got: {html}");
    // linkValidation: context-free `visit` is a no-op — the href is
    // untouched and no diagnostics are raised.
    assert!(
        html.contains(r#"href="./does-not-exist-1574.md""#),
        "got: {html}"
    );
    assert!(
        pipeline.take_markdown_diagnostics().is_empty(),
        "context-free run must never surface link-validation diagnostics"
    );
}

// ── frontmatter filename wrapper ────────────────────────────────────────────

#[test]
fn extract_from_filename_with_frontmatter_returns_parsed_values_and_body() {
    let src = "---\ntitle: Hello\ndraft: false\n---\n# Body\n\nSome text.\n";
    let uf = extract_from_filename("post.md", src).expect("extract ok");
    assert_eq!(uf.value["title"].as_str(), Some("Hello"));
    assert_eq!(uf.value["draft"].as_bool(), Some(false));
    assert_eq!(uf.body.as_deref(), Some("# Body\n\nSome text.\n"));
    let body_offset = uf.body_offset.expect("body_offset present");
    assert_eq!(&src[body_offset..], uf.body.as_deref().unwrap());
}

#[test]
fn extract_from_filename_without_frontmatter_returns_null_and_full_body() {
    let src = "# Just markdown\n\nNo frontmatter here.\n";
    let uf = extract_from_filename("post.md", src).expect("extract ok");
    assert!(uf.value.is_null());
    assert_eq!(uf.body.as_deref(), Some(src));
    assert_eq!(uf.body_offset, Some(0));
}

// ── one-shot config-JSON convenience wrappers ───────────────────────────────

#[test]
fn compile_mdx_jsx_from_config_one_shot_matches_two_step() {
    let config_json = r#"{"features": {"ruby": true}}"#;
    let input = "{これは}^{これ}\n";
    let one_shot =
        compile_mdx_jsx_from_config(config_json, input, "ruby.mdx").expect("one-shot compiles");

    let mut pipeline = build_pipeline_from_json(config_json).expect("valid config");
    let two_step =
        render_mdx_jsx_module(&mut pipeline, input, "ruby.mdx").expect("two-step compiles");

    assert_eq!(one_shot, two_step);
}
