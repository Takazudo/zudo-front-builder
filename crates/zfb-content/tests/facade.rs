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
//!   touch the filesystem through this facade; (e) `codeHighlight` facade
//!   ROUTING (knob → class output through `build_pipeline` — the engine
//!   itself, prefix/roles/multiline/fingerprint, is already covered by
//!   `tests/class_mode_pipeline.rs` and deliberately NOT re-tested here)
//!   plus its validation (exclusion, `deny_unknown_fields`) and the
//!   legacy-interaction matrix (absent / `null` / `mode:"inline"` with and
//!   without `theme` / `mode:"class"` rejection); (f) `theme: null`
//!   semantics (zfb#1852) — pinned to "built-in default theme, still
//!   highlighted", NOT "no highlighting".
//! - **Blind spots** (explicitly NOT covered here): wasm-target
//!   compilation (crate still builds for the host triple only — that's
//!   the sibling `zfb-md-wasm` crate's job in a later wave); performance
//!   characteristics of rebuilding a `Pipeline` per call; the syntect
//!   `fancy-regex` backend swap (a sibling sub-issue, zfb#1573).

use zfb_content::facade::{
    build_pipeline, build_pipeline_from_json, compile_mdx_jsx_from_config, parse_pipeline_options,
    render_html, render_html_from_config, render_mdx_jsx_module, CodeHighlightMode,
    CodeHighlightOptions, FacadeError, GfmOptions, ParseDialect, ParseMdastOptions,
    PipelineOptions,
};
use zfb_content::frontmatter::extract_from_filename;
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::serializer::serialize;

// ── PipelineOptions JSON shape ──────────────────────────────────────────────

#[test]
fn pipeline_options_default_matches_bare_full_config_constructor() {
    let options = PipelineOptions::default();
    let mut from_facade = build_pipeline(&options).expect("default options build a pipeline");
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

// ── theme-name validation surfaces as an error, not a panic ─────────────────

/// `with_defaults_and_full_config` validates theme names at build start
/// (zfb#1067 / zfb#1070) even with `themes_dir = None`, so an unknown
/// `theme` in user options JSON reaches `build_pipeline`. Before zfb#1576
/// the facade `.expect`ed this path away — on wasm32 that panic would trap
/// and poison the instance. It must surface as `FacadeError::Highlight`.
#[test]
fn unknown_theme_name_is_an_error_not_a_panic() {
    let err = build_pipeline_from_json(r#"{"theme": "NoSuchThemeName"}"#)
        .map(|_| ())
        .expect_err("unknown theme must be rejected");
    assert!(
        matches!(err, FacadeError::Highlight(_)),
        "expected FacadeError::Highlight, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("NoSuchThemeName"),
        "error must name the unknown theme: {msg}"
    );
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
fn code_enrichment_word_highlight_from_json_config() {
    let input = "```js /answer/\nconst answer = 42;\n```\n";
    let html = render(r#"{"features": {"codeEnrichment": {}}}"#, input);
    assert!(html.contains(r#"class="highlighted-word""#), "got: {html}");
    assert_eq!(
        html.matches(r#"class="highlighted-word""#).count(),
        1,
        "got: {html}"
    );
}

#[test]
fn code_enrichment_word_highlight_can_be_disabled_from_json_config() {
    let input = "```js {1} /answer/\nconst answer = 42; // [!code ++]\n```\n";
    let html = render(
        r#"{"features": {"codeEnrichment": {"wordHighlight": false}}}"#,
        input,
    );
    assert!(!html.contains("highlighted-word"), "got: {html}");
    assert!(html.contains(r#"data-line-diff="added""#), "got: {html}");
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

// ── codeHighlight: facade routing (zfb#1852) ────────────────────────────────
//
// The class-emission ENGINE (prefix, roles, multiline state, fingerprint)
// is already covered end-to-end by `tests/class_mode_pipeline.rs` and is
// deliberately NOT re-tested here — these tests only prove the facade's
// `codeHighlight` JSON knob reaches `Pipeline::with_defaults_and_full_config_class`
// (and that the inline arm is unaffected).

const RUST_FENCE: &str = "```rust\nfn main() {}\n```\n";

#[test]
fn code_highlight_class_mode_routes_to_class_arm() {
    let html = render(r#"{"codeHighlight": {"mode": "class"}}"#, RUST_FENCE);
    assert!(
        html.contains("class=\"hi-root\""),
        "pre must carry the default hi-root class: {html}"
    );
    assert!(
        html.contains("class=\"hi-kw\""),
        "token spans must carry role classes: {html}"
    );
    assert!(!html.contains("style=\"color:"), "got: {html}");
    assert!(!html.contains("color:#"), "got: {html}");
    assert!(!html.contains("class=\"syntect-"), "got: {html}");
}

#[test]
fn code_highlight_class_mode_honours_custom_prefix_and_role_classes() {
    let html = render(
        r#"{"codeHighlight": {
            "mode": "class",
            "classPrefix": "token-",
            "roleClasses": {"keyword": "text-violet-600 dark:text-violet-400"}
        }}"#,
        RUST_FENCE,
    );
    assert!(html.contains("class=\"token-root\""), "got: {html}");
    assert!(
        html.contains("class=\"text-violet-600 dark:text-violet-400\""),
        "got: {html}"
    );
    assert!(
        !html.contains("class=\"token-kw\""),
        "override must replace the default class, not add to it: {html}"
    );
}

/// `build_pipeline` (not just `Pipeline::with_defaults_and_full_config_class`
/// directly) actually produces a distinct compile-cache fingerprint for
/// class mode vs. the default inline pipeline — proving the facade's
/// routing reaches the same fingerprint machinery `pipeline.rs:1874-1878`
/// already wires into the class arm, not a fingerprint-losing bypass.
#[test]
fn code_highlight_class_mode_fingerprint_differs_from_inline_default() {
    let inline = build_pipeline(&PipelineOptions::default()).expect("inline builds");
    let class_options = PipelineOptions {
        code_highlight: Some(CodeHighlightOptions {
            mode: CodeHighlightMode::Class,
            ..CodeHighlightOptions::default()
        }),
        ..PipelineOptions::default()
    };
    let class = build_pipeline(&class_options).expect("class builds");
    assert_ne!(
        inline.config_fingerprint(),
        class.config_fingerprint(),
        "class-mode and inline-mode configs must never alias one compile-cache fingerprint"
    );
}

// ── codeHighlight: validation ───────────────────────────────────────────────

#[test]
fn code_highlight_class_mode_with_theme_is_rejected() {
    let err = build_pipeline_from_json(
        r#"{"theme": "InspiredGitHub", "codeHighlight": {"mode": "class"}}"#,
    )
    .map(|_| ())
    .expect_err("mode class + theme must be rejected");
    assert!(
        matches!(err, FacadeError::ClassModeExcludesTheme),
        "expected FacadeError::ClassModeExcludesTheme, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("codeHighlight.mode") && msg.contains("theme"),
        "error must name both the mode and theme: {msg}"
    );
}

#[test]
fn code_highlight_invalid_class_prefix_is_rejected() {
    let err =
        build_pipeline_from_json(r#"{"codeHighlight": {"mode": "class", "classPrefix": "1hi-"}}"#)
            .map(|_| ())
            .expect_err("invalid classPrefix must be rejected");
    assert!(
        matches!(err, FacadeError::ClassHighlight(_)),
        "expected FacadeError::ClassHighlight, got: {err:?}"
    );
    assert!(err.to_string().contains("codeHighlight."), "got: {err}");
}

#[test]
fn code_highlight_unknown_role_key_is_rejected() {
    let err = build_pipeline_from_json(
        r#"{"codeHighlight": {"mode": "class", "roleClasses": {"kw": "text-blue-600"}}}"#,
    )
    .map(|_| ())
    .expect_err("short role name must be rejected");
    assert!(matches!(err, FacadeError::ClassHighlight(_)));
}

#[test]
fn code_highlight_unknown_field_is_rejected() {
    let err = parse_pipeline_options(r#"{"codeHighlight": {"bogus": true}}"#)
        .expect_err("must reject unknown field inside codeHighlight");
    let msg = err.to_string();
    assert!(
        msg.contains("bogus") || msg.contains("unknown field"),
        "error should name the unknown field, got: {msg}"
    );
}

#[test]
fn code_highlight_unknown_mode_value_is_rejected() {
    let err = parse_pipeline_options(r#"{"codeHighlight": {"mode": "block"}}"#)
        .expect_err("must reject unknown mode value");
    let msg = err.to_string();
    assert!(
        msg.contains("block") || msg.contains("unknown variant"),
        "got: {msg}"
    );
}

#[test]
fn code_highlight_default_class_prefix_matches_shared_constant() {
    let options = parse_pipeline_options(r#"{"codeHighlight": {"mode": "class"}}"#)
        .expect("valid class-mode config");
    let ch = options.code_highlight.expect("code_highlight present");
    assert_eq!(ch.class_prefix, zfb_content::DEFAULT_CLASS_HIGHLIGHT_PREFIX);
    assert!(ch.role_classes.is_none());
}

// ── codeHighlight: legacy-interaction matrix (zfb#1852) ─────────────────────
//
// Absent `codeHighlight`; `codeHighlight: null`; `mode: "inline"` with and
// without `theme`; `mode: "class"` + `theme` rejection (covered above).
// `theme` stays top-level while the class-mode knobs nest under
// `codeHighlight` — a deliberate partial congruence with native
// `zfb::config::CodeHighlightConfig` (which nests `theme` alongside `mode`);
// nesting `theme` under `codeHighlight` here too would be a breaking change
// to the pre-#1852 `PipelineOptions::theme` field, and this facade has no
// dual-theme knob to nest beside it either.

#[test]
fn legacy_interaction_matrix_matches_pre_1852_inline_behavior() {
    let baseline = {
        let mut p = Pipeline::with_defaults_and_full_config(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            None,
            true,
            false,
            None,
        )
        .expect("no themes_dir — cannot fail");
        serialize(&p.run(RUST_FENCE).expect("baseline run"))
    };

    for config_json in [
        "{}",
        r#"{"codeHighlight": null}"#,
        r#"{"codeHighlight": {"mode": "inline"}}"#,
    ] {
        let mut pipeline = build_pipeline_from_json(config_json).expect("valid config");
        let html = render_html(&mut pipeline, RUST_FENCE).expect("render");
        assert_eq!(
            html, baseline,
            "config {config_json:?} must reproduce the pre-#1852 inline pipeline byte-for-byte"
        );
    }

    // mode:"inline" WITH a theme is the pre-existing themed-inline path,
    // unaffected by the new class-mode routing — distinct from the
    // untethed baseline above (proves the theme actually took effect).
    let themed_html = render(
        r#"{"theme": "InspiredGitHub", "codeHighlight": {"mode": "inline"}}"#,
        RUST_FENCE,
    );
    assert_ne!(
        themed_html, baseline,
        "mode:\"inline\" + theme must still apply the theme"
    );
    assert!(
        themed_html.contains("class=\"syntect-inspiredgithub\""),
        "got: {themed_html}"
    );
}

// ── theme: null semantics (zfb#1852 review finding) ─────────────────────────
//
// The TS-side docs (`crates/zfb-md-wasm/npm/src/types.ts`) used to claim
// `theme: null` meant "no syntax highlighting". That was never what this
// deserializer does: `Option<String>` maps JSON `null` to `None` exactly
// like an absent key, and `None` selects the built-in default theme
// (`base16-ocean.dark`) — fenced code is highlighted either way. This test
// pins the ACTUAL resolved semantics; the docs were corrected to match
// (not the other way around) so this facade's `theme` keeps the exact same
// contract as native `zfb::config::CodeHighlightConfig::theme`.
#[test]
fn theme_null_uses_built_in_default_theme_not_no_highlighting() {
    let explicit_null = render(r#"{"theme": null}"#, RUST_FENCE);
    let absent = render("{}", RUST_FENCE);
    assert_eq!(
        explicit_null, absent,
        "theme: null must be indistinguishable from theme being absent"
    );
    assert!(
        explicit_null.contains("class=\"syntect-base16-ocean-dark\""),
        "theme: null must still highlight with the built-in default theme, not skip highlighting: {explicit_null}"
    );
    assert!(
        explicit_null.contains("style=\"color:"),
        "theme: null must still emit inline per-token color — highlighting is NOT disabled: {explicit_null}"
    );
}

// ── parse_mdast (PROTOTYPE — zfb#1855, epic zfb#1854) ───────────────────────
//
// Raw-parser mdast export spike. Level 1 (pure `&str` → tree logic). These
// tests pin the LOCKED contract: raw markdown-rs output under the exact
// `constructs_for_pipeline` construct set — no visitors, every node carrying
// real parser positions. Pruned with the facade fn on a Wave-2 no-go.

#[test]
fn parse_mdast_returns_raw_tree_with_positions() {
    use markdown::mdast::Node;
    let ast =
        zfb_content::facade::parse_mdast(ParseMdastOptions::default(), "# Hi\n\nSome *em* text.\n")
            .expect("valid markdown parses");
    let Node::Root(root) = &ast else {
        panic!("top node is a Root");
    };
    assert_eq!(root.children.len(), 2, "heading + paragraph");
    assert!(
        matches!(root.children[0], Node::Heading(_)),
        "first child is the heading"
    );
    fn assert_positions(node: &Node) {
        assert!(
            node.position().is_some(),
            "raw parser mdast node must carry a position: {node:?}"
        );
        if let Some(children) = node.children() {
            for child in children {
                assert_positions(child);
            }
        }
    }
    assert_positions(&ast);
}

#[test]
fn parse_mdast_respects_resolved_gfm_construct_toggles() {
    use markdown::mdast::Node;
    let table_src = "| a | b |\n| - | - |\n| 1 | 2 |\n";
    fn contains_table(node: &Node) -> bool {
        matches!(node, Node::Table(_))
            || node
                .children()
                .is_some_and(|c| c.iter().any(contains_table))
    }

    let default_on = zfb_content::facade::parse_mdast(ParseMdastOptions::default(), table_src)
        .expect("table source parses");
    assert!(
        contains_table(&default_on),
        "conservative default has gfm_table ON"
    );

    let options = ParseMdastOptions {
        gfm: GfmOptions {
            table: false,
            ..GfmOptions::default()
        },
        ..ParseMdastOptions::default()
    };
    let toggled_off =
        zfb_content::facade::parse_mdast(options, table_src).expect("still parses without table");
    assert!(
        !contains_table(&toggled_off),
        "gfm.table=false must not produce a Table node"
    );
}

#[test]
fn parse_mdast_keeps_mdx_constructs_and_surfaces_parse_errors() {
    use markdown::mdast::Node;
    let options = ParseMdastOptions::default();
    let ast = zfb_content::facade::parse_mdast(options, "<Card label=\"x\" />\n")
        .expect("MDX JSX parses under the mdx construct set");
    fn contains_jsx(node: &Node) -> bool {
        matches!(
            node,
            Node::MdxJsxFlowElement(_) | Node::MdxJsxTextElement(_)
        ) || node.children().is_some_and(|c| c.iter().any(contains_jsx))
    }
    assert!(contains_jsx(&ast), "JSX element parses as an mdx node");

    // An OPENED-but-never-closed element errors at EOF; a merely incomplete
    // tag (`<Card` with no `>`) is NOT an error at the raw-mdast stage —
    // markdown-rs degrades it to text.
    let err = zfb_content::facade::parse_mdast(options, "<Card>\n")
        .expect_err("unclosed JSX element is a parse error");
    assert!(
        matches!(err, zfb_content::pipeline::PipelineError::Parse(_)),
        "parse failures surface as PipelineError::Parse: {err:?}"
    );
}

fn serialized_node_type_count(node: &markdown::mdast::Node, node_type: &str) -> usize {
    fn count(value: &serde_json::Value, node_type: &str) -> usize {
        match value {
            serde_json::Value::Object(map) => {
                usize::from(map.get("type").and_then(serde_json::Value::as_str) == Some(node_type))
                    + map
                        .values()
                        .map(|value| count(value, node_type))
                        .sum::<usize>()
            }
            serde_json::Value::Array(items) => {
                items.iter().map(|value| count(value, node_type)).sum()
            }
            _ => 0,
        }
    }

    count(
        &serde_json::to_value(node).expect("raw mdast serializes"),
        node_type,
    )
}

#[test]
fn markdown_dialect_preserves_commonmark_constructs_and_literal_braces() {
    let source = "<!-- marker -->\n\n<div>raw</div>\n\n![alt](x.png){w=full}\n\n<https://example.com>\n\n    indented\n";
    let ast = zfb_content::facade::parse_mdast(
        ParseMdastOptions {
            dialect: ParseDialect::Markdown,
            ..ParseMdastOptions::default()
        },
        source,
    )
    .expect("CommonMark-only constructs parse in markdown mode");

    assert_eq!(serialized_node_type_count(&ast, "html"), 2);
    assert_eq!(serialized_node_type_count(&ast, "image"), 1);
    assert_eq!(serialized_node_type_count(&ast, "link"), 1);
    assert_eq!(serialized_node_type_count(&ast, "code"), 1);
    let json = serde_json::to_value(&ast).expect("raw mdast serializes");
    assert!(
        json.to_string().contains("{w=full}"),
        "image attribute-like braces remain literal Markdown text"
    );
}

#[test]
fn raw_parser_applies_each_gfm_switch_independently_in_both_dialects() {
    struct Case {
        name: &'static str,
        source: &'static str,
        required_types: &'static [&'static str],
        enable: fn(&mut GfmOptions),
    }

    let cases = [
        Case {
            name: "strikethrough",
            source: "before ~~gone~~ after\n",
            required_types: &["delete"],
            enable: |gfm| gfm.strikethrough = true,
        },
        Case {
            name: "table",
            source: "| a | b |\n| - | - |\n| 1 | 2 |\n",
            required_types: &["table"],
            enable: |gfm| gfm.table = true,
        },
        Case {
            name: "autolinkLiteral",
            source: "Visit www.example.com today.\n",
            required_types: &["link"],
            enable: |gfm| gfm.autolink_literal = true,
        },
        Case {
            name: "taskListItem",
            source: "- [x] done\n",
            required_types: &[],
            enable: |gfm| gfm.task_list_item = true,
        },
        Case {
            name: "footnoteDefinition",
            source: "[^note]: detail\n\nSee [^note].\n",
            required_types: &["footnoteDefinition", "footnoteReference"],
            enable: |gfm| gfm.footnote_definition = true,
        },
    ];

    for dialect in [ParseDialect::Markdown, ParseDialect::Mdx] {
        for case in &cases {
            let off = zfb_content::facade::parse_mdast(
                ParseMdastOptions {
                    dialect,
                    gfm: GfmOptions {
                        strikethrough: false,
                        table: false,
                        autolink_literal: false,
                        task_list_item: false,
                        footnote_definition: false,
                    },
                },
                case.source,
            )
            .unwrap_or_else(|error| panic!("{} off in {dialect:?}: {error}", case.name));

            let mut gfm = GfmOptions {
                strikethrough: false,
                table: false,
                autolink_literal: false,
                task_list_item: false,
                footnote_definition: false,
            };
            (case.enable)(&mut gfm);
            let on =
                zfb_content::facade::parse_mdast(ParseMdastOptions { dialect, gfm }, case.source)
                    .unwrap_or_else(|error| panic!("{} on in {dialect:?}: {error}", case.name));

            for required_type in case.required_types {
                assert_eq!(
                    serialized_node_type_count(&off, required_type),
                    0,
                    "{} off must suppress `{required_type}` in {dialect:?}",
                    case.name
                );
                assert!(
                    serialized_node_type_count(&on, required_type) > 0,
                    "{} on must produce `{required_type}` in {dialect:?}",
                    case.name
                );
            }
            if case.name == "taskListItem" {
                let off_json = serde_json::to_string(&off).expect("off tree serializes");
                let on_json = serde_json::to_string(&on).expect("on tree serializes");
                assert!(
                    !off_json.contains("\"checked\":"),
                    "taskListItem off leaves ordinary list items in {dialect:?}"
                );
                assert!(
                    on_json.contains("\"checked\":true"),
                    "taskListItem on marks the checked item in {dialect:?}"
                );
            }
        }
    }
}
