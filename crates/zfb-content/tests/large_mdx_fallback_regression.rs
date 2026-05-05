//! Regression-pin for zfb #203 / #206 — residual large-MDX fallback.
//!
//! ## Background
//!
//! After PR #202 fixed three pipeline-shape mismatches between the
//! snapshot walker (`zfb_content::build_snapshot`) and the bundler's
//! shadow-write loop (`zfb_build::bundler::materialise_collection`),
//! a downstream zudo-doc bilingual fallback audit dropped from 104
//! fallback pages to 2. The remaining 2 entries — the same
//! auto-generated meta-doc (`l-lessons-zfb-migration-parity.mdx` and
//! its docs-ja mirror, ≈ 74 KB each) — have these distinctive traits
//! that none of PR #202's regression fixtures exercised:
//!
//!   1. body ≥ 70 KB,
//!   2. inline code spans containing HTML-tag-like text such as
//!      `` `<link rel="stylesheet">` `` and
//!      `` `<script type="module">` ``,
//!   3. inline code spans with curly-brace patterns such as
//!      `` `{main-deploy,preview-deploy,pr-checks}.yml` `` and
//!      `` `@theme { --color-*: initial; }` ``,
//!   4. single-line bullets ≥ 1000 chars long mixing inline code with
//!      prose.
//!
//! ## Investigation findings (issue #206 trace deliverable)
//!
//! Crate / file / function the issue suggested as the most likely
//! responsible site:
//!   `crates/zfb-build/src/bundler.rs::jsx_likely_breaks_downstream_parser`
//!
//! Divergence classification per issue #206 taxonomy:
//!   **(d) other (audit false positive — see below).**
//!
//! After running the actual `l-lessons-zfb-migration-parity.mdx` file
//! end-to-end through both halves of the pipeline (snapshot side via
//! `build_snapshot_with_config` with `strip_md_ext: true`, bundler side
//! via `compile_mdx_to_jsx_module_cached` with the same default
//! pipeline), three observations:
//!
//!   1. MDX parse succeeds.
//!   2. The compiled JSX does NOT trip
//!      `jsx_likely_breaks_downstream_parser`. The emitter wraps every
//!      inline-code value in `js_string_literal_in_braces` (see
//!      `crates/zfb-content/src/mdx_jsx_emit.rs`), producing
//!      `{"…escaped…"}` shapes whose `{` is followed by `"` (not `-`
//!      or `\\`), which the heuristic correctly skips. HTML-tag-like
//!      content and curly-brace shell patterns inside inline-code
//!      values are escaped into the string literal (`<` → `<`, `{` →
//!      `{` — JS strings allow them verbatim) and never surface as
//!      bare JSX expressions.
//!   3. Snapshot `module_specifier` hash **equals** bundler-side
//!      compile hash byte-for-byte (`959febef` for the live file at
//!      the time of writing). The bridge map in the live worker
//!      bundle (`dist/.zfb-build/bundle.mjs`) registers exactly this
//!      hash, so `globalThis.__zfb.content.get(specifier)` lands.
//!
//! Inspection of the live `dist/docs/claude-skills/l-lessons-zfb-migration-parity/index.html`
//! found ZERO occurrences of the actual fallback marker
//! `[zfb fallback render]` and zero `<pre data-zfb-content-fallback>`
//! elements rendered as page content. The matches the audit script
//! counted are textual mentions of `data-zfb-content-fallback` and
//! "fallback rendering" inside `<code>` and `<p>` elements that
//! describe the original bug — not actual fallback emissions.
//!
//! The audit was thus a grep-based false positive on these two pages.
//! No code-side fix is needed; the regression risk PR #202 closed is
//! genuinely closed for this content shape. To prevent re-occurrence
//! and to give future maintainers an executable contract for "this
//! 4-trait content shape must not fall back", we commit the test
//! below as a forward-defense pin.
//!
//! ## Pinned contract
//!
//! Given a deterministic 70 KB+ MDX fixture exhibiting all four
//! distinctive traits enumerated above, the test asserts:
//!
//!   (a) MDX parse succeeds.
//!   (b) Snapshot `module_specifier` hash equals an independent
//!       bundler-style `compile_mdx_to_jsx_module_cached` hash.
//!   (c) The compiled JSX does NOT trip the heuristic mirror in
//!       `heuristic_says_jsx_breaks` (kept in sync with
//!       `crates/zfb-build/src/bundler.rs::jsx_likely_breaks_downstream_parser`).
//!
//! Any future regression that reintroduces a heuristic false-positive
//! or a hash divergence on this content shape would fail this test.
//!
//! ## Fixture generator
//!
//! [`build_residual_fallback_fixture`] is the deterministic
//! generator: same constants → same bytes across runs. The fixture
//! is built inline from constants — no `rand`, no external file
//! dependency. Trait coverage is asserted up front so the test
//! self-describes its own preconditions.

use std::path::PathBuf;
use zfb_content::pipeline::Pipeline;
use zfb_content::{
    build_snapshot_with_config, compile_mdx_to_jsx_module_cached, parse_mdx_specifier,
    CollectionConfig, SnapshotPipelineConfig,
};

// -----------------------------------------------------------------------------
// Fixture generator
// -----------------------------------------------------------------------------

/// Deterministic fixture generator. Returns
/// `(combined_with_frontmatter, body_only)` so the test can hand the
/// pre-frontmatter form to disk and the post-frontmatter form to the
/// independent compile call (mirroring how the snapshot walker splits
/// frontmatter before compilation).
///
/// Trait coverage:
///
///   1. body ≥ 70 KB — driven by `padding_repeats` × `PADDING_PARAGRAPH`.
///   2. inline code spans with HTML-tag-like text — see
///      `INLINE_CODE_HTML_TAG_LIKE`.
///   3. inline code spans with curly-brace patterns — see
///      `INLINE_CODE_CURLY_BRACE`.
///   4. single-line bullet ≥ 1000 chars mixing inline code with
///      prose — see [`build_long_single_line_bullet`].
fn build_residual_fallback_fixture() -> (String, String) {
    // Inline-code spans with HTML-tag-like content. The MDX emitter
    // wraps each value in `js_string_literal_in_braces`, so the
    // post-emission JSX shape is `{"<link rel=\"stylesheet\">"}` — a
    // JSX expression that immediately enters a string literal. The
    // heuristic at `crates/zfb-build/src/bundler.rs::jsx_likely_breaks_downstream_parser`
    // must NOT flag these.
    const INLINE_CODE_HTML_TAG_LIKE: &[&str] = &[
        "<link rel=\"stylesheet\">",
        "<script type=\"module\">",
        "<script>",
        "<link>",
    ];

    // Inline-code spans containing curly-brace patterns. Same emitter
    // path, same heuristic concern. The `{main-deploy,…}.yml` shape is
    // a brace-expansion idiom from shell scripts; `@theme { --color-*: initial; }`
    // mirrors a Tailwind v4 pattern that appeared in zudo-doc.
    const INLINE_CODE_CURLY_BRACE: &[&str] = &[
        "{main-deploy,preview-deploy,pr-checks}.yml",
        "@theme { --color-*: initial; }",
        "{ name: \"x\" }",
        "${ env.VAR }",
    ];

    // Body padding — repeated paragraph that pushes the file size past
    // 70 KB. Each paragraph is ~510 bytes; we repeat until the body
    // crosses the threshold. Pure prose so no plugin-stress here —
    // the unique-token signals come from the curated inline-code
    // sections above.
    const PADDING_PARAGRAPH: &str = "\
This paragraph exists purely to push the fixture body past the 70 KB \
size threshold called out in issue #206. The content is intentionally \
prosaic — no fenced code, no admonitions, no math, no GFM tables — so \
the only pipeline-stress signals come from the curated inline-code \
spans elsewhere in the fixture. The same paragraph is repeated many \
times below; markdown-rs handles long sequences of identical paragraph \
nodes without any special-casing, so this padding contributes only \
size, not unique tokenization edge cases.\n\n";
    let padding_repeats = 145; // 145 × ~510 bytes ≈ 74 KB

    let frontmatter = "---\ntitle: \"Residual Fallback Fixture\"\n---\n\n";

    let mut body = String::with_capacity(80_000);

    // Section 1 — inline-code spans with HTML-tag-like text and curly
    // braces.
    body.push_str("## Inline code with HTML-tag-like content\n\n");
    for span in INLINE_CODE_HTML_TAG_LIKE {
        body.push_str(&format!(
            "Use `{span}` here to embed a tag in the document head. The surrounding prose makes the inline code render through the MDX `_components.code` path.\n\n"
        ));
    }
    body.push_str("## Inline code with curly-brace content\n\n");
    for span in INLINE_CODE_CURLY_BRACE {
        body.push_str(&format!(
            "Reference `{span}` in your config. The brace expansion is a shell idiom.\n\n"
        ));
    }

    // Section 2 — the long-single-line bullet. ≥ 1000 chars, mixes
    // inline code with prose. This is the worst content shape a
    // single mdast list-item paragraph can carry: a long sequence of
    // `<_components.code>{"…"}</_components.code>` expressions joined
    // by JSX-string-literal text fragments.
    body.push_str("## Long bullet line\n\n");
    body.push_str(&build_long_single_line_bullet(
        INLINE_CODE_HTML_TAG_LIKE,
        INLINE_CODE_CURLY_BRACE,
    ));
    body.push_str("\n\n");

    // Section 3 — body size padding to push past 70 KB.
    body.push_str("## Padding for size threshold\n\n");
    for _ in 0..padding_repeats {
        body.push_str(PADDING_PARAGRAPH);
    }

    let combined = format!("{frontmatter}{body}");
    (combined, body)
}

/// Build a single bullet point ≥ 1000 chars, alternating between
/// connector prose and inline-code spans. Result is one `- ` line
/// followed by a single newline so markdown parses it as a single
/// list item with one paragraph child.
fn build_long_single_line_bullet(
    html_spans: &[&str],
    brace_spans: &[&str],
) -> String {
    const CONNECTOR: &str = " — and combined with this connector phrase the surrounding prose grows long enough to push the bullet over the kilobyte mark while still mixing inline code with explanatory text — ";
    let mut bullet = String::with_capacity(1500);
    bullet.push_str("- ");
    let mut idx = 0;
    while bullet.len() < 1000 {
        let html = html_spans[idx % html_spans.len()];
        let brace = brace_spans[idx % brace_spans.len()];
        bullet.push_str(&format!("`{html}`{CONNECTOR}`{brace}`"));
        bullet.push_str(CONNECTOR);
        idx += 1;
    }
    bullet
}

// -----------------------------------------------------------------------------
// Heuristic re-implementation
// -----------------------------------------------------------------------------

/// Local copy of `zfb_build::bundler::jsx_likely_breaks_downstream_parser`
/// so this `zfb-content`-side test does not need a `pub` re-export
/// from `zfb-build`. The function is small and self-contained; if it
/// ever diverges visibly the `jsx_breakage_heuristic_flags_…` test in
/// zfb-build will catch the drift.
///
/// **Keep in sync with `crates/zfb-build/src/bundler.rs`.** When
/// touching the heuristic, mirror the change here too.
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
            if j + 1 < bytes.len()
                && bytes[j] == b'\\'
                && bytes[j + 1].is_ascii_alphabetic()
            {
                return true;
            }
        }

        i += 1;
    }
    false
}

// -----------------------------------------------------------------------------
// The test
// -----------------------------------------------------------------------------

/// End-to-end pin for issue #206.
///
/// Builds the deterministic 4-trait fixture, runs both pipeline
/// halves, and asserts the snapshot/bundler hash agreement and the
/// no-heuristic-false-positive guarantee.
///
/// The pipeline is configured with `strip_md_ext: true` to mirror the
/// real zudo-doc `zfb.config.ts` setting that exercises the
/// downstream fallback audit.
#[test]
fn large_mdx_with_inline_code_html_curly_braces_does_not_fall_back() {
    let (combined, body) = build_residual_fallback_fixture();

    // Trait-1: body size ≥ 70 KB.
    assert!(
        body.len() >= 70_000,
        "fixture body must be ≥ 70 KB to mirror the live trigger; got {} bytes",
        body.len()
    );
    // Trait-2: each HTML-tag-like inline code span appears at least once.
    for needle in [
        "`<link rel=\"stylesheet\">`",
        "`<script type=\"module\">`",
        "`<script>`",
        "`<link>`",
    ] {
        assert!(
            body.contains(needle),
            "fixture must contain inline-code span {needle}"
        );
    }
    // Trait-3: each curly-brace inline code span appears at least once.
    for needle in [
        "`{main-deploy,preview-deploy,pr-checks}.yml`",
        "`@theme { --color-*: initial; }`",
    ] {
        assert!(
            body.contains(needle),
            "fixture must contain inline-code span {needle}"
        );
    }
    // Trait-4: at least one line ≥ 1000 chars.
    let max_line = body.lines().map(|l| l.len()).max().unwrap_or(0);
    assert!(
        max_line >= 1000,
        "fixture must have at least one line ≥ 1000 chars; longest is {max_line}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    let path: PathBuf = root.join("residual.mdx");
    std::fs::write(&path, &combined).unwrap();

    // Snapshot side — mirror zudo-doc's actual pipeline shape so the
    // hashes computed match the production path.
    let pipeline_config = SnapshotPipelineConfig {
        code_highlight_theme: None,
        strip_md_ext: true,
        resolve_source_map: None,
    };
    let snap = build_snapshot_with_config(
        &[CollectionConfig::new("docs", &root)],
        &pipeline_config,
    )
    .expect("build_snapshot must succeed");
    let docs = snap.collections.get("docs").expect("docs collection");
    assert_eq!(docs.len(), 1, "exactly one entry in fixture");
    let entry = &docs[0];

    // Bundler side — independent compile mirroring `materialise_collection`.
    let mut pipeline = Pipeline::with_defaults();
    pipeline.add_strip_md_ext();
    pipeline.reset_per_entry();
    let compiled =
        compile_mdx_to_jsx_module_cached(&body, &path, None, Some(&mut pipeline))
            .expect("independent compile must succeed");

    // (a) Heuristic must NOT flag the compiled JSX. A trip here
    // would mean the bundler skips this file in the bridge map and
    // the page renders the `<pre data-zfb-content-fallback>` shape —
    // exactly the residual-fallback failure mode issue #206 chases.
    assert!(
        !heuristic_says_jsx_breaks(&compiled.jsx_source),
        "jsx_likely_breaks_downstream_parser must NOT flag this fixture's compiled JSX. \
         A trip here is issue #206's residual-fallback bug — the bundler will skip this \
         file in the bridge map and the page will render the <pre data-zfb-content-fallback> \
         fallback. The heuristic would then be over-eager on JSX produced by the MDX \
         emitter for inline code spans containing HTML-tag-like text and curly-brace \
         patterns. Today the emitter wraps inline-code values in `js_string_literal_in_braces`, \
         producing `{{\"…\"}}` shapes whose `{{` is followed by `\"` — neither `-` nor `\\\\` — \
         so the heuristic correctly skips."
    );

    // (b) Snapshot hash must equal bundler-side hash.
    let snap_spec = parse_mdx_specifier(&entry.module_specifier)
        .expect("snapshot specifier parses");
    let bundle_spec = parse_mdx_specifier(&compiled.specifier)
        .expect("bundler-style specifier parses");
    assert_eq!(
        snap_spec.content_hash, bundle_spec.content_hash,
        "snapshot module_specifier hash ({snap}) must equal bundler bridge-key hash ({bundle}). \
         A divergence here means `bridge.get(specifier)` will miss and the page will fall \
         back to <pre data-zfb-content-fallback>. This is a pipeline-shape mismatch in the \
         spirit of zfb #132 / #188 / #190 — but on the large-MDX content shape that PR #202 \
         did not specifically cover.",
        snap = snap_spec.content_hash,
        bundle = bundle_spec.content_hash,
    );
    assert_eq!(snap_spec.collection, bundle_spec.collection);
    assert_eq!(snap_spec.slug, bundle_spec.slug);
}
