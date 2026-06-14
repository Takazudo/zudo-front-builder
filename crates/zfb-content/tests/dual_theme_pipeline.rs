//! Integration tests for the dual-theme code-highlight pipeline (#1067).
//!
//! Verifies the end-to-end wiring of `{ themeLight, themeDark }` through
//! `Pipeline::with_defaults_and_full_config_dual` (and `PipelineSpec` with the
//! dual fields set), asserting:
//!
//! 1. Token spans carry `--shiki-light` / `--shiki-dark` CSS custom properties
//!    and NO inline `color:`.
//! 2. The `<pre>` element carries `class="syntect-dual"` and
//!    `--shiki-light-bg` / `--shiki-dark-bg` in its `style` attribute.
//! 3. The single-theme path is BYTE-IDENTICAL to the pre-dual build (no
//!    inline `color:` regression in the old path).
//! 4. `PipelineSpec` with the dual fields builds a dual-mode pipeline.

use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::serializer::serialize;
use zfb_content::PipelineSpec;

const RUST_CODE: &str = "```rust\nfn main() {}\n```\n";

/// Build a pipeline and run a markdown snippet through it, returning the
/// serialised HTML.
fn run_markdown(pipeline: &mut Pipeline, md: &str) -> String {
    let hast = pipeline
        .run(md)
        .expect("pipeline.run must succeed for valid markdown");
    serialize(&hast)
}

// ── Dual-theme path ──────────────────────────────────────────────────────────

/// The primary acceptance test: dual-theme mode emits `--shiki-light` /
/// `--shiki-dark` custom properties (NO inline `color:`) and wraps the
/// block in `<pre class="syntect-dual" style="--shiki-light-bg:…;--shiki-dark-bg:…">`.
#[test]
fn dual_pipeline_emits_shiki_vars_and_dual_class() {
    let mut pipeline = Pipeline::with_defaults_and_full_config_dual(
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false,
        None,
        "base16-ocean.light",
        "base16-ocean.dark",
    )
    .expect("no themes_dir — cannot fail");

    let html = run_markdown(&mut pipeline, RUST_CODE);

    // class="syntect-dual"
    assert!(
        html.contains("class=\"syntect-dual\""),
        "pre must carry class=\"syntect-dual\": {html}"
    );
    // --shiki-light-bg and --shiki-dark-bg in the style attribute
    assert!(
        html.contains("--shiki-light-bg:"),
        "pre style must carry --shiki-light-bg: {html}"
    );
    assert!(
        html.contains("--shiki-dark-bg:"),
        "pre style must carry --shiki-dark-bg: {html}"
    );
    // Per-token CSS custom properties
    assert!(
        html.contains("--shiki-light:#"),
        "token spans must carry --shiki-light: {html}"
    );
    assert!(
        html.contains("--shiki-dark:#"),
        "token spans must carry --shiki-dark: {html}"
    );
    // The whole point: NO inline color: in dual mode.
    assert!(
        !html.contains("color:#"),
        "dual mode must NOT emit inline color: {html}"
    );
}

/// The `<span class="line">` wrapper structure must be identical to the
/// single-theme path — each line is a mutable Element.
#[test]
fn dual_pipeline_produces_span_line_structure() {
    let mut pipeline = Pipeline::with_defaults_and_full_config_dual(
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false,
        None,
        "base16-ocean.light",
        "base16-ocean.dark",
    )
    .expect("no themes_dir — cannot fail");

    let hast = pipeline.run(RUST_CODE).expect("run ok");
    use zfb_content::pipeline::HastNode;
    // Walk looking for the pre element.
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

    let pre = find_pre(&hast).expect("pre element must be in the HAST");
    let HastNode::Element {
        attrs,
        children: pre_children,
        ..
    } = pre
    else {
        panic!("expected Element<pre>");
    };
    // class must be "syntect-dual"
    assert!(
        attrs
            .iter()
            .any(|(k, v)| k == "class" && v == "syntect-dual"),
        "pre class must be syntect-dual: {attrs:?}"
    );
    // There must be a <code> child with <span class="line"> children.
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

// ── Single-theme byte-identity (codex #4) ────────────────────────────────────

/// Single-theme path must still emit inline `color:` and NO `--shiki-` vars.
/// This assertion was already in `syntect_plugin.rs`; we also pin it at the
/// full-pipeline level to cover the wiring chain end-to-end.
#[test]
fn single_pipeline_still_emits_inline_color_not_shiki_vars() {
    let mut pipeline = Pipeline::with_defaults_and_full_config(
        Some("base16-ocean.dark"),
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false,
        None,
    )
    .expect("no themes_dir — cannot fail");

    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("color:#"),
        "single mode must still emit inline color: {html}"
    );
    assert!(
        !html.contains("--shiki-"),
        "single mode must NOT emit --shiki- vars: {html}"
    );
    assert!(
        html.contains("class=\"syntect-"),
        "single mode must carry syntect-* class: {html}"
    );
    assert!(
        !html.contains("class=\"syntect-dual\""),
        "single mode must NOT carry syntect-dual class: {html}"
    );
}

// ── PipelineSpec dual fields ─────────────────────────────────────────────────

/// `PipelineSpec` with both `code_highlight_theme_light` and
/// `code_highlight_theme_dark` set must produce the same dual-mode output as
/// `Pipeline::with_defaults_and_full_config_dual`.
#[test]
fn pipeline_spec_dual_fields_build_dual_pipeline() {
    let spec = PipelineSpec {
        code_highlight_theme_light: Some("base16-ocean.light".to_string()),
        code_highlight_theme_dark: Some("base16-ocean.dark".to_string()),
        ..Default::default()
    };
    let mut pipeline = spec
        .build_pipeline()
        .expect("PipelineSpec::build_pipeline ok");
    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("class=\"syntect-dual\""),
        "spec-built pipeline must carry syntect-dual class: {html}"
    );
    assert!(
        html.contains("--shiki-light:#"),
        "spec-built pipeline must carry --shiki-light: {html}"
    );
    assert!(
        html.contains("--shiki-dark:#"),
        "spec-built pipeline must carry --shiki-dark: {html}"
    );
    assert!(
        !html.contains("color:#"),
        "spec-built dual pipeline must NOT emit inline color: {html}"
    );
}

/// `PipelineSpec` with only `code_highlight_theme` set (single mode) must
/// still behave identically to the pre-dual single-theme path.
#[test]
fn pipeline_spec_single_field_unchanged() {
    let spec = PipelineSpec {
        code_highlight_theme: Some("base16-ocean.dark".to_string()),
        ..Default::default()
    };
    let mut pipeline = spec
        .build_pipeline()
        .expect("PipelineSpec::build_pipeline ok");
    let html = run_markdown(&mut pipeline, RUST_CODE);

    assert!(
        html.contains("color:#"),
        "single-theme spec must still emit inline color: {html}"
    );
    assert!(
        !html.contains("--shiki-"),
        "single-theme spec must NOT emit --shiki- vars: {html}"
    );
}
