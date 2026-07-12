//! Integration tests for the class-emission code-highlight pipeline (#1532).
//!
//! Verifies the end-to-end wiring of `codeHighlight.mode: "class"` through
//! `Pipeline::with_defaults_and_full_config_class` (and `PipelineSpec` with
//! `code_highlight_mode: CodeHighlightMode::Class`), asserting:
//!
//! 1. Token spans carry semantic role classes (`hi-kw`, …) and NO inline
//!    `color:` / `--shiki-*` custom property; the `<pre>` is classed
//!    `{prefix}root` (default `hi-root`), NOT `syntect-{slug}`.
//! 2. `roleClasses` overrides are emitted verbatim end-to-end.
//! 3. A multi-line construct classifies line-2 correctly through the pipeline
//!    (the ParseState + ScopeStack persistence contract, observed end-to-end).
//! 4. The `<span class="line">` structure is identical to the single/dual path.
//! 5. The INLINE and DUAL paths are unchanged — characteristic-substring
//!    regression guards confirm this sub did not perturb them.

use std::collections::BTreeMap;

use zfb_content::pipeline::{HastNode, Pipeline, ResolvedGfmConstructs};
use zfb_content::{CodeHighlightMode, PipelineSpec};

const RUST_CODE: &str = "```rust\nfn main() {}\n```\n";
const RUST_MULTILINE: &str = "```rust\n/* c1\n c2 */\n```\n";

/// Build a pipeline and run a markdown snippet through it, returning the
/// serialised HTML.
fn run_markdown(pipeline: &mut Pipeline, md: &str) -> String {
    let hast = pipeline
        .run(md)
        .expect("pipeline.run must succeed for valid markdown");
    zfb_content::serializer::serialize(&hast)
}

/// Find the first `<pre>` element anywhere in the HAST.
fn find_pre(node: &HastNode) -> Option<&HastNode> {
    match node {
        HastNode::Root { children } | HastNode::Element { children, .. } => {
            for c in children {
                if let HastNode::Element { tag, .. } = c {
                    if tag == "pre" {
                        return Some(c);
                    }
                }
                if let Some(found) = find_pre(c) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

// ── Class-emission path ──────────────────────────────────────────────────────

/// Primary acceptance test: class mode emits `<pre class="hi-root">` with
/// role-classed token spans and NO inline colour form.
#[test]
fn class_pipeline_emits_role_classes_and_root_class() {
    let mut pipeline = Pipeline::with_defaults_and_full_config_class(
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false,
        None,
        "hi-",
        &BTreeMap::new(),
    )
    .expect("no themes_dir — cannot fail");

    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("class=\"hi-root\""),
        "pre must carry class=\"hi-root\": {html}"
    );
    assert!(
        html.contains("class=\"hi-kw\""),
        "token spans must carry role classes: {html}"
    );
    assert!(
        !html.contains("color:#"),
        "class mode must NOT emit inline color: {html}"
    );
    assert!(
        !html.contains("--shiki-"),
        "class mode must NOT emit shiki vars: {html}"
    );
    assert!(
        !html.contains("class=\"syntect-"),
        "class mode must NOT carry a syntect-* class: {html}"
    );
}

/// `PipelineSpec` with `code_highlight_mode: Class` builds a class-mode
/// pipeline — the path the bundler and snapshot walker actually use.
#[test]
fn pipeline_spec_class_mode_builds_class_pipeline() {
    let spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        code_highlight_class_prefix: "hi-".to_string(),
        ..Default::default()
    };
    let mut pipeline = spec
        .build_pipeline()
        .expect("PipelineSpec::build_pipeline ok");
    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("class=\"hi-root\""),
        "spec-built class pipeline must carry hi-root: {html}"
    );
    assert!(
        html.contains("class=\"hi-kw\""),
        "spec-built class pipeline must carry role classes: {html}"
    );
    assert!(
        !html.contains("color:#"),
        "spec-built class pipeline must NOT emit inline color: {html}"
    );
    assert!(
        !html.contains("--shiki-"),
        "spec-built class pipeline must NOT emit shiki vars: {html}"
    );
}

/// A configured `roleClasses` override reaches the emitted span verbatim
/// through the full `PipelineSpec` path; the overridden role no longer uses
/// its `{prefix}{short_name}` default.
#[test]
fn pipeline_spec_class_mode_applies_role_classes_override() {
    let mut roles = BTreeMap::new();
    // Keyed by the FULL role name (`"keyword"`), exactly as `codeHighlight.roleClasses`
    // supplies it — NOT the short suffix `"kw"`. Looking overrides up by short
    // name silently dropped them (zfb#1528 deep-review fix).
    roles.insert(
        "keyword".to_string(),
        "text-violet-600 dark:text-violet-400".to_string(),
    );
    let spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        code_highlight_class_prefix: "hi-".to_string(),
        code_highlight_role_classes: roles,
        ..Default::default()
    };
    let mut pipeline = spec
        .build_pipeline()
        .expect("PipelineSpec::build_pipeline ok");
    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("class=\"text-violet-600 dark:text-violet-400\""),
        "override class must be emitted verbatim end-to-end: {html}"
    );
    assert!(
        !html.contains("class=\"hi-kw\""),
        "overridden role must not use the default class: {html}"
    );
}

/// Cross-line coverage through the pipeline: a two-line block comment must
/// classify its line-2 content as a comment. This only holds because the
/// engine persists ONE ParseState and ONE ScopeStack across the newline.
#[test]
fn class_pipeline_multiline_comment_classifies_line2() {
    let spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        ..Default::default()
    };
    let mut pipeline = spec.build_pipeline().expect("build ok");
    let hast = pipeline.run(RUST_MULTILINE).expect("run ok");

    let pre = find_pre(&hast).expect("pre element must be in the HAST");
    let HastNode::Element {
        children: pre_children,
        ..
    } = pre
    else {
        panic!("expected Element<pre>");
    };
    let code = pre_children
        .iter()
        .find(|c| matches!(c, HastNode::Element { tag, .. } if tag == "code"))
        .expect("pre must have <code>");
    let HastNode::Element {
        children: code_children,
        ..
    } = code
    else {
        panic!("expected Element<code>");
    };
    assert_eq!(code_children.len(), 2, "two source lines → two line spans");
    let HastNode::Element {
        children: line2_children,
        ..
    } = &code_children[1]
    else {
        panic!("line 2: expected Element<span>");
    };
    let HastNode::Raw(raw) = &line2_children[0] else {
        panic!("line 2 span must wrap a Raw node");
    };
    assert!(
        raw.contains("hi-com"),
        "line 2 of the block comment must classify as hi-com (ScopeStack must \
         persist across lines): {raw}"
    );
}

/// The `<span class="line">` wrapper structure must be identical to the
/// single/dual path — each line is a mutable Element under `<code>`.
#[test]
fn class_pipeline_produces_span_line_structure() {
    let spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        ..Default::default()
    };
    let mut pipeline = spec.build_pipeline().expect("build ok");
    let hast = pipeline.run(RUST_CODE).expect("run ok");

    let pre = find_pre(&hast).expect("pre element must be in the HAST");
    let HastNode::Element {
        attrs,
        children: pre_children,
        ..
    } = pre
    else {
        panic!("expected Element<pre>");
    };
    assert!(
        attrs.iter().any(|(k, v)| k == "class" && v == "hi-root"),
        "pre class must be hi-root: {attrs:?}"
    );
    let code = pre_children
        .iter()
        .find(|c| matches!(c, HastNode::Element { tag, .. } if tag == "code"))
        .expect("pre must have <code>");
    let HastNode::Element {
        children: code_children,
        ..
    } = code
    else {
        panic!("expected Element<code>");
    };
    for (i, span) in code_children.iter().enumerate() {
        let HastNode::Element { tag, attrs, .. } = span else {
            panic!("code child {i}: expected Element<span>, got {span:?}");
        };
        assert_eq!(tag, "span", "code child {i}: tag must be span");
        assert!(
            attrs.iter().any(|(k, v)| k == "class" && v == "line"),
            "code child {i}: must have class=\"line\": {attrs:?}"
        );
    }
}

// ── Regression guards: inline + dual paths unchanged (characteristic substrings) ─

/// The inline (default) mode must still emit inline `color:` and NO `--shiki-`
/// vars, keep its `syntect-*` class, and NEVER carry a class-mode root. This
/// pins that the class-emission sub did not perturb the inline path.
#[test]
fn inline_spec_unchanged_still_emits_inline_color() {
    let spec = PipelineSpec::default();
    let mut pipeline = spec.build_pipeline().expect("build ok");
    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("color:#"),
        "inline mode must still emit inline color: {html}"
    );
    assert!(
        !html.contains("--shiki-"),
        "inline mode must NOT emit --shiki- vars: {html}"
    );
    assert!(
        html.contains("class=\"syntect-"),
        "inline mode must carry a syntect-* class: {html}"
    );
    assert!(
        !html.contains("class=\"hi-root\""),
        "inline mode must NOT carry a class-mode root: {html}"
    );
}

// ── Cross-feature: CodeTitlePlugin × class mode (confirm sub #1535, check 2) ──

/// A fence carrying `title="…"` + a language, run through a class-mode
/// pipeline: `CodeTitlePlugin` (always-on Core plugin) wraps the block in
/// `.code-block-container`/`.code-block-title` AND the inner `<pre>` is
/// class-mode output (`class="hi-root"`, `hi-*` token spans, no inline
/// `color:`).
///
/// `CodeTitlePlugin` runs pre-syntect and reads the fence's `data-meta`
/// before `SyntectPlugin` consumes/replaces the `<pre>` element (see the
/// visitor-ordering contract on `Pipeline::with_defaults_and_full_config_inner`
/// and the doc comment on `CodeTitlePlugin` itself) — that ordering is
/// identical in inline, dual, and class mode, since the class-mode
/// constructor (`with_defaults_and_full_config_class`) reuses the exact
/// same `CodeTitlePlugin::new()` wiring as the inline path. The
/// `04-code-block` snapshot (`crates/zfb-content/tests/integration_pipeline.rs`)
/// only covers this interaction for INLINE mode; this test is the class-mode
/// counterpart.
#[test]
fn code_title_wraps_class_mode_pre_with_title_container() {
    let spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        ..Default::default()
    };
    let mut pipeline = spec.build_pipeline().expect("build ok");
    let input = "```rust title=\"main.rs\"\nfn main() {}\n```\n";
    let html = run_markdown(&mut pipeline, input);

    // CodeTitlePlugin wrapper is present.
    assert!(
        html.contains("<div class=\"code-block-container\">"),
        "code-title wrapper must be present in class mode: {html}"
    );
    assert!(
        html.contains("<div class=\"code-block-title\">main.rs</div>"),
        "code-title must carry the fence's title text in class mode: {html}"
    );

    // The inner <pre> is still class-mode output: hi-root root class,
    // hi-* token spans, no inline color, no syntect-* slug, no leaked
    // title= remnant in data-meta.
    assert!(
        html.contains("class=\"hi-root\""),
        "inner pre must carry class=\"hi-root\" even when titled: {html}"
    );
    assert!(
        html.contains("class=\"hi-kw\""),
        "inner pre token spans must carry role classes even when titled: {html}"
    );
    assert!(
        !html.contains("color:#"),
        "titled class-mode block must NOT emit inline color: {html}"
    );
    assert!(
        !html.contains("class=\"syntect-"),
        "titled class-mode block must NOT carry a syntect-* class: {html}"
    );
    assert!(
        !html.contains("title=\\\"main.rs\\\""),
        "title=… must be stripped from data-meta, not leaked into the class-mode pre: {html}"
    );
}

/// The dual mode must still emit `--shiki-light` / `--shiki-dark` custom
/// properties, the `syntect-dual` class, NO inline `color:`, and NEVER a
/// class-mode root. Regression guard for the dual path.
#[test]
fn dual_spec_unchanged_still_emits_shiki_vars() {
    let spec = PipelineSpec {
        code_highlight_theme_light: Some("base16-ocean.light".to_string()),
        code_highlight_theme_dark: Some("base16-ocean.dark".to_string()),
        ..Default::default()
    };
    let mut pipeline = spec.build_pipeline().expect("build ok");
    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("class=\"syntect-dual\""),
        "dual mode must carry syntect-dual: {html}"
    );
    assert!(
        html.contains("--shiki-light:#"),
        "dual mode must carry --shiki-light: {html}"
    );
    assert!(
        html.contains("--shiki-dark:#"),
        "dual mode must carry --shiki-dark: {html}"
    );
    assert!(
        !html.contains("color:#"),
        "dual mode must NOT emit inline color: {html}"
    );
    assert!(
        !html.contains("class=\"hi-root\""),
        "dual mode must NOT carry a class-mode root: {html}"
    );
}
