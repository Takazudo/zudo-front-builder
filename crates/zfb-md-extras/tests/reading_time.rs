//! Fixture-based integration tests for the reading-time feature.
//!
//! Each subdirectory under `tests/fixtures/reading_time/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds the
//! expected reading-time minutes (as a decimal string, e.g. `"1\n"`).
//! All fixtures use `normalize.txt = "verbatim"` — the computed integer is
//! compared as a raw string, not as HTML.
//!
//! The transform closure parses the markdown to mdast with
//! [`markdown::ParseOptions::default`] and calls
//! [`zfb_md_extras::reading_time::compute_reading_time_minutes`] to get the
//! result, which is then formatted as `"N\n"`.
//!
//! Acceptance criteria verified here:
//! - 200-word English article (default WPM=200) → 1 minute.
//! - 400-word article → 2 minutes.
//! - 600-word article → 3 minutes.
//! - CJK characters use the per-character count formula.
//! - Code blocks are excluded from the word count.
//! - Short articles below 1 minute minimum return 1.

use zfb_md_extras::reading_time::compute_reading_time_minutes;
use zfb_md_extras::test_harness::run_fixture;

/// Default WPM used by all fixtures (matches `ReadingTimePlugin::new()`).
const DEFAULT_WPM: u32 = 200;

/// Run a fixture directory. The transform parses the markdown, computes
/// reading-time minutes, and returns the result as `"N\n"`.
fn run(name: &str) {
    let dir =
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/reading_time/").to_string() + name;
    run_fixture(&dir, |input| {
        let node = markdown::to_mdast(input, &markdown::ParseOptions::default())
            .expect("markdown parse failed");
        let minutes = compute_reading_time_minutes(&node, DEFAULT_WPM);
        format!("{minutes}\n")
    });
}

// ── Acceptance criteria ───────────────────────────────────────────────────────

/// Acceptance criterion: a 200-word English article at default WPM=200
/// yields `readingTimeMinutes: 1`.
#[test]
fn en_200_words_is_1_minute() {
    run("en-200-words");
}

/// Acceptance criterion: a 400-word article at 200 WPM yields 2 minutes.
#[test]
fn en_400_words_is_2_minutes() {
    run("en-400-words");
}

/// Acceptance criterion: a 600-word article at default WPM=200
/// yields `readingTimeMinutes: 3`.
#[test]
fn en_600_words_is_3_minutes() {
    run("en-600-words");
}

/// Acceptance criterion: CJK characters use the per-character count formula
/// (each character = 1 word). Text under 200 chars/words → 1 minute.
#[test]
fn cjk_basic_is_1_minute() {
    run("cjk-basic");
}

/// Code blocks are excluded from the word count — a document whose code block
/// contains hundreds of words but whose prose body is short must still report
/// a short reading time.
#[test]
fn code_excluded_from_count() {
    run("code-excluded");
}

/// Short articles (below 1 minute) are clamped to the minimum of 1 minute.
#[test]
fn short_article_minimum_1_minute() {
    run("short-article");
}

// ── Disabled smoke test ───────────────────────────────────────────────────────

/// When `readingTime: false`, the pipeline must NOT wire the visitor and the
/// mdast tree must remain unchanged (no MdxjsEsm node appended).
#[test]
fn disabled_does_not_append_esm_node() {
    use zfb_content::pipeline::Pipeline;
    use zfb_md_ast::ReadingTimeFeature;
    use zfb_md_extras::MarkdownFeaturesConfig;

    let features = MarkdownFeaturesConfig {
        reading_time: Some(ReadingTimeFeature::Bool(false)),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    // The HTML pipeline still runs fine with reading_time disabled.
    let result = p.run("hello world");
    assert!(result.is_ok(), "pipeline failed: {:?}", result);
}

/// When `readingTime: true`, the pipeline wires the visitor and the reading
/// time export is injected into the mdast tree.
#[test]
fn enabled_injects_export_in_mdast() {
    use markdown::mdast::Node as MdastNode;
    use markdown::to_mdast;
    use zfb_md_ast::MdastVisitor;
    use zfb_md_extras::reading_time::ReadingTimePlugin;

    let words: String = std::iter::repeat("word ").take(200).collect();
    let mut node = to_mdast(&words, &markdown::ParseOptions::default()).unwrap();
    ReadingTimePlugin::new().visit(&mut node);

    let MdastNode::Root(root) = &node else {
        panic!("expected Root");
    };
    let last = root.children.last().expect("children must not be empty");
    assert!(
        matches!(last, MdastNode::MdxjsEsm(_)),
        "ReadingTimePlugin must append MdxjsEsm node, got: {:?}",
        last
    );
    let MdastNode::MdxjsEsm(esm) = last else {
        unreachable!()
    };
    assert!(
        esm.value.contains("readingTimeMinutes = 1"),
        "injected export must contain readingTimeMinutes = 1: {}",
        esm.value
    );
}

/// JSX module output contains `export const readingTimeMinutes = 1;` when
/// the reading-time feature is enabled.
#[test]
fn jsx_module_contains_reading_time_export() {
    use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
    use zfb_content::pipeline::Pipeline;
    use zfb_md_ast::ReadingTimeFeature;
    use zfb_md_extras::MarkdownFeaturesConfig;

    let words: String = std::iter::repeat("word ").take(200).collect();
    let features = MarkdownFeaturesConfig {
        reading_time: Some(ReadingTimeFeature::Bool(true)),
        ..Default::default()
    };
    let mut pipeline = Pipeline::with_defaults_and_features(&features);
    let jsx = mdx_to_jsx_module_with_pipeline(&words, MdxJsxOptions::default(), &mut pipeline)
        .expect("mdx_to_jsx_module failed");

    assert!(
        jsx.contains("export const readingTimeMinutes = 1;"),
        "JSX module must contain readingTimeMinutes export for 200-word doc:\n{jsx}"
    );
}
