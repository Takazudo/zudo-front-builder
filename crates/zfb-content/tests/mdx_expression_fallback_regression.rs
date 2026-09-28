//! Regression-pin for zfb #1729 / #1779 — MDX expression nodes carrying
//! a bare `{\letter}` shape silently degrade the WHOLE page to the
//! `<pre data-zfb-content-fallback>` render.
//!
//! ## Background
//!
//! A real docs page (`design-token-lint.mdx` in `zudo-css-wisdom`) that
//! mixes an island component (`<CssPreview>`) with fenced/inline code
//! demos compiled into JSX that tripped the bundler's
//! then-byte-scan `jsx_likely_breaks_downstream_parser` gate
//! (`crates/zfb-build/src/bundler.rs`; replaced in #2216 by the real
//! SWC parse `jsx_module_parse_failure`), degrading the ENTIRE page —
//! no island, no prose — to a raw `<pre>` fallback. That gate fired on
//! a `{` (optionally `-`) directly followed by `\` + an ASCII letter.
//!
//! ## Diagnosis (issue #1779)
//!
//! Fenced/inline code never leaks: it parses to `Code`/`InlineCode` and
//! is brace-escaped (`{` → `&#123;`) inside an MDX JSX body, or
//! string-literal'd at top level. The `{\letter}` shape is produced
//! ONLY by an MDX expression node (`MdxFlowExpression` /
//! `MdxTextExpression`) or an expression-valued attribute whose value
//! begins with a backslash-escape (`\n`, `\d`, …) — the signature of a
//! JS string's inner escape sequence leaking OUTSIDE a string, which is
//! never a valid *bare* JS expression. The emitter's expression sinks
//! (`jsx_raw_recursive`, `jsx_render_child`, `render_jsx_attrs`) emitted
//! such values verbatim, so a single one degraded the whole page.
//!
//! The leak is NOT specific to island descendants: `mdast_to_hast_with`'s
//! `JsxEmitStrategy::JsxPath` routes every JSX-shaped node (any depth)
//! through `jsx_raw_recursive`, so a top-level `{\n}` leaks identically
//! to one nested inside `<CssPreview>…</CssPreview>`. Both fixtures
//! below therefore pin the same emitter contract.
//!
//! ## Repair
//!
//! Each expression sink now routes through `emit_mdx_expression_braced`:
//! if emitting `{value}` verbatim would trip the emitter's `{\letter}`
//! byte pre-filter (a string/comment-aware scan, historically mirroring
//! the pre-#2216 gate), the value is recovered as a
//! JS string literal (`{"…escaped…"}`) so the bytes stay visible and the
//! module parses. Only the breaking shape is recovered — valid MDX
//! expressions (`{1 + 1}`), expression attributes (`count={1 + 2}`),
//! spreads (`{...rest}`), and JSX comments (`{/* … */}`) never begin
//! with `\letter`, so they are emitted verbatim, unchanged.
//!
//! ## Pinned contract
//!
//! For each fixture (island-descendant and top-level):
//!   (a) MDX parse + compile succeeds.
//!   (b) The compiled JSX does NOT trip `heuristic_says_jsx_breaks`
//!       (mirror of the pre-#2216 byte-scan bundler gate) — no
//!       whole-page fallback.
//!   (c) With `compiler`, the compiled JSX parses through SWC directly.
//!   (d) The recovered code bytes remain visibly present.
//!   (e) Valid expression / attribute / spread / comment shapes survive
//!       verbatim.
//!   (f) Snapshot `module_specifier` hash equals the bundler-side
//!       `compile_mdx_to_jsx_module_cached` hash (bridge-key parity).

use std::path::{Path, PathBuf};

use zfb_content::pipeline::Pipeline;
use zfb_content::{
    build_snapshot_with_config, compile_mdx_to_jsx_module_cached, parse_mdx_specifier,
    CollectionConfig, PipelineSpec,
};

#[cfg(feature = "compiler")]
#[path = "support/swc_parse.rs"]
mod swc_parse;
#[cfg(feature = "compiler")]
use swc_parse::{CompileOptions, SwcPipeline};

// -----------------------------------------------------------------------------
// Heuristic mirror of the bundler's pre-#2216 byte-scan gate
// (`jsx_likely_breaks_downstream_parser`, replaced in #2216 by the real
// SWC parse `jsx_module_parse_failure` in
// `crates/zfb-build/src/bundler.rs`); identical to the mirror in
// `large_mdx_fallback_regression.rs`. Pins properties of EMITTER
// OUTPUT — no live byte-scan gate left to sync with.
// -----------------------------------------------------------------------------

fn heuristic_says_jsx_breaks(jsx: &str) -> bool {
    let bytes = jsx.as_bytes();
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];

        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(q) = in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == q {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == b'/' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'/' => {
                    in_line_comment = true;
                    i += 2;
                    continue;
                }
                b'*' => {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        if c == b'"' || c == b'\'' || c == b'`' {
            in_string = Some(c);
            i += 1;
            continue;
        }

        if c == b'{' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'-' {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'\\' && bytes[j + 1].is_ascii_alphabetic() {
                return true;
            }
        }

        i += 1;
    }
    false
}

#[cfg(feature = "compiler")]
fn assert_swc_accepts(jsx_module: &str, label: &str) {
    let pipeline = SwcPipeline::new();
    let opts = CompileOptions::default().with_filename(format!("{label}.tsx"));
    let compiled = pipeline.compile(jsx_module, &opts).unwrap_or_else(|e| {
        panic!("SWC rejected emitter output for {label}: {e}\n--- src ---\n{jsx_module}")
    });
    assert!(
        !compiled.code.is_empty(),
        "SWC produced empty code for {label}"
    );
}

/// Assert the compiled JSX is (b) not gate-flagged, (c) SWC-parseable,
/// and returns the JSX so callers can assert byte presence.
fn compile_body(label: &str, body: &str) -> String {
    let mut pipeline = Pipeline::with_defaults();
    pipeline.reset_per_entry();
    let compiled =
        compile_mdx_to_jsx_module_cached(body, Path::new("t.mdx"), None, Some(&mut pipeline))
            .unwrap_or_else(|e| panic!("compile failed for {label}: {e}"));
    assert!(
        !heuristic_says_jsx_breaks(&compiled.jsx_source),
        "{label}: compiled JSX trips the byte-scan mirror (heuristic_says_jsx_breaks) — a \
         `{{\\letter}}` leak regressed; a genuinely-bare leak fails the bundler's SWC parse \
         gate too, so the bundler would skip this file in the bridge map and degrade the \
         WHOLE page to the <pre data-zfb-content-fallback> shape (issue #1729). Emitted \
         JSX:\n{}",
        compiled.jsx_source
    );
    #[cfg(feature = "compiler")]
    assert_swc_accepts(&compiled.jsx_source, label);
    compiled.jsx_source
}

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

/// Island-descendant leak: a `<CssPreview>`-style island (template-literal
/// attributes, like the real page) wrapping markdown whose raw-HTML
/// descendant carries a `{\d}` expression — the exact reroute through
/// `jsx_render_child` that #1779 targets. Also carries the valid shapes
/// that MUST survive verbatim so a future over-eager recovery would
/// regress this test.
const ISLAND_DESCENDANT_FIXTURE: &str = r#"import CssPreview from '@/components/CssPreview';

# Design token lint

<CssPreview client:load
  title="Before / After"
  html={`<div class="demo">before</div>`}
  css={`.demo { color: red; }`}
>

A demo whose highlighted template leaked a bare expression: <pre><code>{\d}</code></pre>

Valid child expression that must survive: {1 + 1}

A JSX comment that must survive: {/* design-token-lint-ignore */}

</CssPreview>

An attribute expression that must survive: <Note count={1 + 2} />

A spread attribute that must survive: <Note {...rest} />
"#;

/// Top-level leak (no island): proves the emitter contract is not
/// descendant-specific — a top-level `{\n}` flows through the same
/// `jsx_raw_recursive` sink.
const TOP_LEVEL_FIXTURE: &str =
    "# Heading\n\nA raw template leaked at top level: <pre>{\\n}</pre>\n\nValid: {1 + 1}\n";

#[test]
fn island_descendant_backslash_expression_does_not_fall_back() {
    let jsx = compile_body("island-descendant", ISLAND_DESCENDANT_FIXTURE);

    // (d) The leaked code bytes are recovered (visible), not dropped: the
    // `\d` survives as an escaped JS string literal `{"\\d"}`.
    assert!(
        jsx.contains(r#"{"\\d"}"#),
        "recovered `\\d` bytes must remain present as a string literal; emitted:\n{jsx}"
    );

    // (e) Valid shapes survive verbatim — a regression toward blanket
    // string-wrapping would break these.
    assert!(
        jsx.contains("{1 + 1}"),
        "valid child expression `{{1 + 1}}` must survive verbatim; emitted:\n{jsx}"
    );
    assert!(
        jsx.contains("count={1 + 2}"),
        "valid attribute expression `count={{1 + 2}}` must survive verbatim; emitted:\n{jsx}"
    );
    assert!(
        jsx.contains("{...rest}"),
        "spread `{{...rest}}` must survive verbatim; emitted:\n{jsx}"
    );
    assert!(
        jsx.contains("{/* design-token-lint-ignore */}"),
        "JSX comment must survive verbatim; emitted:\n{jsx}"
    );
}

#[test]
fn no_pipeline_path_backslash_expression_does_not_fall_back() {
    // The legacy no-pipeline emitter (`mdx_to_jsx_module` → `JsxEmitter`)
    // carries the same expression sinks and must not leak either.
    let jsx = zfb_content::mdx_to_jsx_module(
        "A raw template leaked at top level: <pre>{\\n}</pre>\n\nValid: {1 + 1}\n",
        zfb_content::MdxJsxOptions::default(),
    )
    .expect("no-pipeline compile succeeds");
    assert!(
        !heuristic_says_jsx_breaks(&jsx),
        "no-pipeline path must not trip the gate; emitted:\n{jsx}"
    );
    #[cfg(feature = "compiler")]
    assert_swc_accepts(&jsx, "no-pipeline");
    assert!(
        jsx.contains(r#"{"\\n"}"#),
        "recovered `\\n` bytes must remain present; emitted:\n{jsx}"
    );
    assert!(
        jsx.contains("{1 + 1}"),
        "valid expression must survive verbatim; emitted:\n{jsx}"
    );
}

#[test]
fn top_level_backslash_expression_does_not_fall_back() {
    let jsx = compile_body("top-level", TOP_LEVEL_FIXTURE);
    assert!(
        jsx.contains(r#"{"\\n"}"#),
        "recovered `\\n` bytes must remain present as a string literal; emitted:\n{jsx}"
    );
    assert!(
        jsx.contains("{1 + 1}"),
        "valid expression must survive verbatim; emitted:\n{jsx}"
    );
}

// -----------------------------------------------------------------------------
// Valid-JS regex-literal recovery (codex P2)
//
// The byte scanner behind `emit_mdx_expression_braced` is only a heuristic:
// it visits `{\letter}` bytes even when they sit inside a regex literal
// (`/[{\d}]/`), which is perfectly valid JS. Stringifying such an
// expression would silently turn executable JavaScript into a quoted
// string. The recovery must therefore leave a VALID expression verbatim and
// only string-wrap the genuinely-invalid `\d`-leak class.
//
// Because the emitted regex still contains the raw `{\d}` bytes, the
// emitter's byte mirror (`heuristic_says_jsx_breaks`) is expected to fire
// on it — that is accepted and documented: the mirror is only the
// emitter-side pre-filter shape. Since #2216 the real bundler gate is an
// SWC parse (`jsx_module_parse_failure`), which such a valid module
// PASSES — no whole-page fallback occurs (the pre-#2216 byte-scan gate
// did false-positive here; that documented residual is fixed).
// -----------------------------------------------------------------------------

/// The verbatim JS text a valid regex-literal expression must keep.
const REGEX_EXPR_SRC: &str = r"/[{\d}]/.test(value)";

/// Compile an MDX body through the pipeline WITHOUT the `compile_body`
/// heuristic assertion — the recovered-regex output is expected to trip the
/// byte mirror (see the module comment above), so only the compile+SWC
/// contract is enforced here.
fn compile_body_allowing_heuristic(label: &str, body: &str) -> String {
    let mut pipeline = Pipeline::with_defaults();
    pipeline.reset_per_entry();
    let compiled =
        compile_mdx_to_jsx_module_cached(body, Path::new("t.mdx"), None, Some(&mut pipeline))
            .unwrap_or_else(|e| panic!("compile failed for {label}: {e}"));
    #[cfg(feature = "compiler")]
    assert_swc_accepts(&compiled.jsx_source, label);
    compiled.jsx_source
}

#[test]
fn valid_regex_literal_expression_survives_verbatim_pipeline_path() {
    let body = format!("A regex expression that must survive: {{{REGEX_EXPR_SRC}}}\n");
    let jsx = compile_body_allowing_heuristic("regex-pipeline", &body);

    // The regex source is emitted as executable JS, NOT wrapped as a string.
    assert!(
        jsx.contains(REGEX_EXPR_SRC),
        "valid regex expression must survive verbatim as JS; emitted:\n{jsx}"
    );
    assert!(
        !jsx.contains(r#"{"/["#),
        "valid regex expression must NOT be string-wrapped; emitted:\n{jsx}"
    );

    // Documented, accepted property: the emitted regex keeps the raw
    // `{\d}` bytes, so the emitter's byte mirror fires. Since #2216 the
    // real bundler gate (an SWC parse) accepts this valid module — the
    // page bridges; only the emitter-side pre-filter sees the pattern.
    assert!(
        heuristic_says_jsx_breaks(&jsx),
        "the regex's raw `{{\\d}}` bytes are expected to trip the byte mirror; emitted:\n{jsx}"
    );
}

#[test]
fn valid_regex_literal_expression_survives_verbatim_no_pipeline_path() {
    let body = format!("A regex expression that must survive: {{{REGEX_EXPR_SRC}}}\n");
    let jsx = zfb_content::mdx_to_jsx_module(&body, zfb_content::MdxJsxOptions::default())
        .expect("no-pipeline compile succeeds");
    #[cfg(feature = "compiler")]
    assert_swc_accepts(&jsx, "regex-no-pipeline");
    assert!(
        jsx.contains(REGEX_EXPR_SRC),
        "valid regex expression must survive verbatim as JS on the no-pipeline path; emitted:\n{jsx}"
    );
    assert!(
        !jsx.contains(r#"{"/["#),
        "valid regex expression must NOT be string-wrapped on the no-pipeline path; emitted:\n{jsx}"
    );
}

/// Snapshot↔bridge hash parity (f): the snapshot walker's
/// `module_specifier` hash must equal an independent bundler-style
/// compile hash, or `bridge.get(specifier)` misses and the page falls
/// back regardless of the gate.
#[test]
fn snapshot_bridge_hash_parity_holds_for_recovered_fixture() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    // Wrap the body in frontmatter so the snapshot walker and the bundler
    // strip identical bytes before compiling.
    let combined = format!("---\ntitle: Design token lint\n---\n\n{ISLAND_DESCENDANT_FIXTURE}");
    let path: PathBuf = root.join("design-token-lint.mdx");
    std::fs::write(&path, &combined).unwrap();

    let pipeline_config = PipelineSpec {
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        code_highlight_theme_light: None,
        code_highlight_theme_dark: None,
        code_highlight_mode: zfb_content::CodeHighlightMode::Inline,
        code_highlight_class_prefix: "hi-".to_string(),
        code_highlight_role_classes: std::collections::BTreeMap::new(),
        strip_md_ext: true,
        resolve_source_map: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        toc: None,
        external_links: None,
        cjk_friendly: true,
        hard_breaks: false,
        features: None,
        build_context_roots: None,
    };
    let snap =
        build_snapshot_with_config(&[CollectionConfig::new("docs", &root)], &pipeline_config)
            .expect("build_snapshot must succeed");
    let docs = snap.collections.get("docs").expect("docs collection");
    assert_eq!(docs.len(), 1, "exactly one entry in fixture");
    let entry = &docs[0];

    let mut pipeline = Pipeline::with_defaults();
    pipeline.add_strip_md_ext();
    pipeline.reset_per_entry();
    // The body the bundler compiles is the frontmatter-stripped form.
    let body = zfb_content::frontmatter::extract(&path, &combined)
        .expect("frontmatter parses")
        .body
        .unwrap_or_default();
    let compiled = compile_mdx_to_jsx_module_cached(&body, &path, None, Some(&mut pipeline))
        .expect("independent compile must succeed");

    assert!(
        !heuristic_says_jsx_breaks(&compiled.jsx_source),
        "bundler-side compile must not trip the byte-scan mirror"
    );

    let snap_spec =
        parse_mdx_specifier(&entry.module_specifier).expect("snapshot specifier parses");
    let bundle_spec =
        parse_mdx_specifier(&compiled.specifier).expect("bundler-style specifier parses");
    assert_eq!(
        snap_spec.content_hash, bundle_spec.content_hash,
        "snapshot module_specifier hash ({}) must equal bundler bridge-key hash ({}) — a \
         divergence means bridge.get(specifier) misses and the page falls back.",
        snap_spec.content_hash, bundle_spec.content_hash,
    );
    assert_eq!(snap_spec.collection, bundle_spec.collection);
    assert_eq!(snap_spec.slug, bundle_spec.slug);
}
