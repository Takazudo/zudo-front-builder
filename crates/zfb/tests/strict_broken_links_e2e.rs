//! Command-layer integration proof for `--strict-broken` / `--no-strict-broken`
//! (issue #2118, Link Gating epic #2112).
//!
//! Sub #2117 wired `resolve_strict_broken_links` / `apply_strict_broken_links_override`
//! into `zfb build`'s `run()` (`crates/zfb/src/commands/build.rs`), overriding
//! `config.markdown.features.linkValidation.failOnBroken` before the config
//! reaches the bundler. Enforcement itself (severity → `fatal_findings` →
//! `bail!`) is pre-existing (`crates/zfb-build/src/bundler.rs`, gates 2c /
//! 2c-anchor). This file proves the WIRING end to end through a real,
//! spawned `zfb build` process — not just the unit-level resolver tests
//! already in `commands::build::tests`.
//!
//! ## Cases 6-8 (Link Validation Descent epic #2222, Wave 4 sub #2226)
//!
//! Cases 1-5 above predate #2184's fix and never exercise a link nested
//! inside MDX JSX or a container directive. Cases 6-8 close that gap at the
//! full-CLI level: a broken bare-fragment anchor nested inside `<Note>`
//! (hand-authored MDX JSX) and inside `:::note` (a registered container
//! directive) were previously INVISIBLE to `linkValidation` entirely (no
//! diagnostic at any severity) — the exact defect #2184 fixed at the
//! unit/pipeline level (see `zfb-md-extras`'s `link_validation_jsx_directive_descent.rs`
//! / `link_validation_jsx_descent_counts.rs` / `link_validation_jsx_descent_sweep.rs`).
//! Case 6/7 confirm `--strict-broken` now fails the build on these
//! newly-detected broken links; case 8 confirms the flip side — WITHOUT
//! strict mode, the same links surface only as non-fatal warnings (the
//! build still succeeds), proving the new detection is gated by severity
//! exactly like every other broken-link context, not unconditionally fatal.
//!
//! **This is a real behavior change to call out on upgrade:** a project
//! that sets `strictBrokenLinks: true` (or passes `--strict-broken`) and
//! currently builds green may start FAILING once this fix ships, if it has
//! a genuinely dead link inside `<Note>`/`:::note` it did not know about —
//! exactly the class of bug that let a dead anchor ship silently in
//! `zudolab/zudo-doc` (#2184). That is the intended, correct outcome, not a
//! regression.
//!
//! ## Fixture shape
//!
//! Every case shares a `content/docs` collection with:
//! - `target.mdx` — a valid compile target with one real heading, so the
//!   cross-file anchor check has a heading map to check against.
//! - `source.mdx` — carries a broken **bare (same-file) anchor**
//!   (`#does-not-exist-anchor`) AND a broken **cross-file anchor**
//!   (`./target.mdx#nonexistent-heading-xyz`), matching the `zfb#954` /
//!   `zfb#980` distinction: same-file anchors resolve at compile time
//!   (`mdx_jsx_emit.rs`), cross-file anchors resolve in the bundler's
//!   post-compile pass (`bundler.rs` gate 2c-anchor). Both paths push the
//!   SAME `MarkdownDiagnostic::BrokenLink` variant, formatted by the SAME
//!   `fmt_message` closure (`bundler.rs`, "broken link: {url}") — there is
//!   only one wording, so every assertion below checks for the two
//!   distinct RAW HREFS, never two different message shapes.
//!
//! No page ever calls `getCollection(...)` — `content/docs` is compiled by
//! the bundler unconditionally once `config.collections` is non-empty
//! (`commands/bundler_input.rs`), matching the precedent in
//! `crates/zfb-build/tests/bundler_anchor_check.rs`.
//!
//! ## Red-first (recorded, not re-derivable from this file)
//!
//! Every case here depends on sub #2117's wiring, which is already merged
//! into this branch's history. To prove RED-before-GREEN, `resolve_strict_broken_links`
//! was temporarily hardwired to always return `false` (reproducing the
//! pre-#2117 behavior), all five cases below were run and observed to fail
//! for the intended reason (the override never applied, so a project that
//! should hard-fail built successfully instead), then the hardwire was
//! reverted. The actual failure output is recorded in the commit message
//! and the completion report — this file's job is the permanent green
//! assertion, not the one-time red proof.
//!
//! ## Skip behaviour
//!
//! Uses the embedded esbuild/V8 runtime (`embed_v8` default feature) — no
//! `ZFB_ESBUILD_BIN` pinning needed, matching `end_to_end_basic_blog_build.rs`
//! / `content_snapshot_no_deferred.rs`. Guards a non-zero exit for a
//! known-skip indicator (missing embedded runtime) before treating it as a
//! real failure.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use zfb_test_utils::zfb_binary;

/// A broken same-file (bare) anchor — no filename prefix, so it resolves
/// against `source.mdx`'s own (empty) heading set at compile time (zfb#954).
const BARE_BROKEN_HREF: &str = "#does-not-exist-anchor";

/// A broken cross-file anchor — `target.mdx` compiles and has headings, but
/// not this one, so the bundler's post-compile pass (zfb#980) flags it.
const CROSS_FILE_BROKEN_HREF: &str = "./target.mdx#nonexistent-heading-xyz";

fn write_minimal_page(root: &Path) {
    fs::create_dir_all(root.join("pages")).expect("create pages dir");
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <title>strict-broken-links integration proof</title>
      </head>
      <body>
        <h1>Home</h1>
      </body>
    </html>
  );
}
"#,
    )
    .expect("write pages/index.tsx");
}

fn write_target_doc(root: &Path) {
    fs::create_dir_all(root.join("content/docs")).expect("create content/docs dir");
    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Real Heading\n\nBody.\n",
    )
    .expect("write content/docs/target.mdx");
}

/// `source.mdx` carrying both broken-link shapes this proof needs.
fn write_broken_source_doc(root: &Path) {
    fs::write(
        root.join("content/docs/source.mdx"),
        format!(
            "---\ntitle: Source\n---\n\n\
             See [broken same-file anchor]({BARE_BROKEN_HREF}).\n\n\
             See [broken cross-file anchor]({CROSS_FILE_BROKEN_HREF}).\n"
        ),
    )
    .expect("write content/docs/source.mdx");
}

/// Full fixture: minimal page + `content/docs` collection with both broken
/// links. `config_json` is written verbatim as `zfb.config.json`.
fn write_fixture(root: &Path, config_json: &str) {
    write_minimal_page(root);
    write_target_doc(root);
    write_broken_source_doc(root);
    fs::write(root.join("zfb.config.json"), config_json).expect("write zfb.config.json");
}

/// `true` when a non-zero `zfb build` exit is a known-skip (embedded
/// V8/esbuild unavailable) rather than a genuine failure — same pattern as
/// `content_snapshot_no_deferred.rs` / `end_to_end_basic_blog_build.rs`.
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8") || combined.contains("no esbuild")
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Spawns `zfb build <extra_args>` in `root`. Returns `None` on a
/// known-skip (caller should return early, treating the test as skipped).
fn run_zfb_build(root: &Path, extra_args: &[&str]) -> Option<Output> {
    let output = Command::new(zfb_binary!())
        .arg("build")
        .args(extra_args)
        .current_dir(root)
        .output()
        .expect("spawn zfb binary");
    let combined = combined_output(&output);
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!(
            "[strict_broken_links_e2e] zfb build exited non-zero with a known-skip \
             indicator (embedded V8/esbuild unavailable); skipping test.\n--- combined \
             output ---\n{combined}"
        );
        return None;
    }
    Some(output)
}

// ── Case 1 — main case: --strict-broken forces non-zero + both hrefs ───────

/// zudo-doc preset shape: `failOnBroken: false` set explicitly in the
/// project's own config, plus a broken bare anchor AND a broken cross-file
/// anchor. `--strict-broken` must force the build to fail and name BOTH
/// distinct raw hrefs in the diagnostic output.
#[test]
fn strict_broken_flag_forces_non_zero_exit_and_lists_both_broken_hrefs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_fixture(
        root,
        r#"{
  "collections": [{ "name": "docs", "path": "content/docs" }],
  "markdown": { "features": { "linkValidation": { "failOnBroken": false } } }
}"#,
    );

    let Some(output) = run_zfb_build(root, &["--strict-broken"]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        !output.status.success(),
        "`zfb build --strict-broken` must exit non-zero when the project's own \
         config sets failOnBroken:false but broken links exist\nstatus: {:?}\n--- output \
         ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(BARE_BROKEN_HREF),
        "diagnostic output must name the broken bare-anchor href {BARE_BROKEN_HREF:?}\n\
         --- output ---\n{combined}"
    );
    assert!(
        combined.contains(CROSS_FILE_BROKEN_HREF),
        "diagnostic output must name the broken cross-file href {CROSS_FILE_BROKEN_HREF:?}\n\
         --- output ---\n{combined}"
    );
}

// ── Case 2 — control: no flag, preset's failOnBroken:false wins, exit 0 ────

/// Same project as case 1, no CLI flag at all: the preset's own
/// `failOnBroken: false` applies unmodified, so both broken links are
/// warnings (non-fatal) and the build succeeds.
#[test]
fn control_no_flag_exits_zero_with_nonfatal_warnings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_fixture(
        root,
        r#"{
  "collections": [{ "name": "docs", "path": "content/docs" }],
  "markdown": { "features": { "linkValidation": { "failOnBroken": false } } }
}"#,
    );

    let Some(output) = run_zfb_build(root, &[]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        output.status.success(),
        "`zfb build` with no strict-broken flag must exit 0 when the project's own \
         config sets failOnBroken:false\nstatus: {:?}\n--- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(BARE_BROKEN_HREF) && combined.contains(CROSS_FILE_BROKEN_HREF),
        "the build must still surface both broken links as non-fatal warnings\n\
         --- output ---\n{combined}"
    );
}

// ── Case 3 — force-enable: no linkValidation config at all ─────────────────

/// A bare project with NO `linkValidation` config at all (no `markdown`
/// block whatsoever) plus `--strict-broken` plus a broken anchor must still
/// exit non-zero — proving the `get_or_insert_default()` force-enable chain
/// (epic #2112 Decision 1) end-to-end, not just at the unit level.
#[test]
fn force_enable_with_no_link_validation_config_at_all() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_fixture(
        root,
        r#"{
  "collections": [{ "name": "docs", "path": "content/docs" }]
}"#,
    );

    let Some(output) = run_zfb_build(root, &["--strict-broken"]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        !output.status.success(),
        "`zfb build --strict-broken` must exit non-zero even when the project \
         declares NO linkValidation config at all (force-enable)\nstatus: {:?}\n\
         --- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(BARE_BROKEN_HREF) || combined.contains(CROSS_FILE_BROKEN_HREF),
        "diagnostic output must name at least one broken href\n--- output ---\n{combined}"
    );
}

// ── Case 4 — config-only E2E: strictBrokenLinks:true, no CLI flag ──────────

/// `strictBrokenLinks: true` set in the project's config with NO CLI flag
/// passed must still exit non-zero — proving the persistent-config path
/// end-to-end (not just the CLI-flag path).
#[test]
fn config_only_strict_broken_links_true_with_no_cli_flag() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_fixture(
        root,
        r#"{
  "collections": [{ "name": "docs", "path": "content/docs" }],
  "strictBrokenLinks": true
}"#,
    );

    let Some(output) = run_zfb_build(root, &[]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        !output.status.success(),
        "`zfb build` with no CLI flag must exit non-zero when the config's own \
         strictBrokenLinks:true is set\nstatus: {:?}\n--- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(BARE_BROKEN_HREF) || combined.contains(CROSS_FILE_BROKEN_HREF),
        "diagnostic output must name at least one broken href\n--- output ---\n{combined}"
    );
}

// ── Case 5 — --no-strict-broken never downgrades an independent nested flag ─

/// A config that independently sets `markdown.features.linkValidation.failOnBroken: true`
/// (never touching `strictBrokenLinks`) combined with `--no-strict-broken`
/// must STILL exit non-zero — proving `--no-strict-broken` disables only the
/// top-level `strictBrokenLinks` overlay and never downgrades an
/// independently-authored nested `failOnBroken` setting.
#[test]
fn no_strict_broken_flag_does_not_downgrade_independently_configured_fail_on_broken() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_fixture(
        root,
        r#"{
  "collections": [{ "name": "docs", "path": "content/docs" }],
  "markdown": { "features": { "linkValidation": { "failOnBroken": true } } }
}"#,
    );

    let Some(output) = run_zfb_build(root, &["--no-strict-broken"]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        !output.status.success(),
        "`zfb build --no-strict-broken` must STILL exit non-zero when the project's \
         own config independently sets markdown.features.linkValidation.failOnBroken:true \
         — --no-strict-broken must only disable the strictBrokenLinks overlay, never \
         downgrade an independently-configured nested setting\nstatus: {:?}\n\
         --- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(BARE_BROKEN_HREF) || combined.contains(CROSS_FILE_BROKEN_HREF),
        "diagnostic output must name at least one broken href\n--- output ---\n{combined}"
    );
}

// ── Cases 6-8 — the newly-fixed MDX-JSX / container-directive contexts ─────

/// A broken bare-fragment anchor nested inside `<Note>…</Note>` (hand-authored
/// MDX JSX) — previously invisible to `linkValidation` at any severity.
const JSX_NESTED_BROKEN_HREF: &str = "#missing-in-note-jsx";

/// A broken bare-fragment anchor nested inside `:::note … :::` (a registered
/// container directive) — same pre-existing blind spot, same mechanism.
const DIRECTIVE_NESTED_BROKEN_HREF: &str = "#missing-in-note-directive";

/// `content/docs/nested.mdx` carrying whichever of the two newly-fixed
/// contexts the caller asks for.
///
/// Cases 6 and 7 each pass exactly ONE context `true` — a codex review
/// finding on this file's first draft: a single fixture carrying BOTH
/// broken links at once would let a severity regression isolated to just
/// ONE context hide behind the other. With both links present, a
/// hypothetical bug that made (say) the JSX-nested candidate stop being
/// bumped to `Error` under `--strict-broken` would still leave the build
/// exiting non-zero (the directive link is still fatal) AND still leave
/// the JSX href named in the output (now merely as a non-fatal warning) —
/// so case 6's two assertions would both keep passing for the wrong
/// reason. Isolating each case to its own single-link fixture means a
/// non-zero exit and a named href can ONLY come from the context that
/// test claims to prove. Case 8 (the non-strict control) deliberately asks
/// for BOTH — it never inspects exit-status-per-context, only "both hrefs
/// are named as warnings while the build succeeds", which is not exposed
/// to the same ambiguity.
fn write_nested_broken_source_doc(root: &Path, include_jsx: bool, include_directive: bool) {
    let mut body = String::from("---\ntitle: Nested\n---\n\n");
    if include_jsx {
        body.push_str(&format!(
            "<Note>\n\nSee [broken nested-JSX anchor]({JSX_NESTED_BROKEN_HREF}).\n\n</Note>\n\n"
        ));
    }
    if include_directive {
        body.push_str(&format!(
            ":::note\n\nSee [broken nested-directive anchor]({DIRECTIVE_NESTED_BROKEN_HREF}).\n\n:::\n"
        ));
    }
    fs::write(root.join("content/docs/nested.mdx"), body).expect("write content/docs/nested.mdx");
}

/// Full fixture for cases 6-8: minimal page + `content/docs` collection with
/// ONLY the nested-context broken doc (deliberately NOT case 1-5's
/// `source.mdx`/top-level-anchor fixture — keeps each case's diagnostic
/// output scoped to exactly the hrefs it asserts on).
fn write_nested_fixture(
    root: &Path,
    config_json: &str,
    include_jsx: bool,
    include_directive: bool,
) {
    write_minimal_page(root);
    write_target_doc(root);
    write_nested_broken_source_doc(root, include_jsx, include_directive);
    fs::write(root.join("zfb.config.json"), config_json).expect("write zfb.config.json");
}

/// `zfb.config.json` shared by cases 6-8: registers the `"note"` container
/// directive (`:::note` -> `<Note>`, matching `link_validation_jsx_directive_descent.rs`'s
/// pipeline configuration) alongside `linkValidation`.
fn nested_config_json(fail_on_broken: bool) -> String {
    format!(
        r#"{{
  "collections": [{{ "name": "docs", "path": "content/docs" }}],
  "markdown": {{
    "features": {{
      "directives": {{ "note": "Note" }},
      "linkValidation": {{ "failOnBroken": {fail_on_broken} }}
    }}
  }}
}}"#
    )
}

/// Case 6: `--strict-broken` must fail the build on a broken link nested
/// inside hand-authored MDX JSX (`<Note>`), naming the href — the context
/// #2184 found silently swallowed end-to-end (no diagnostic at all, let
/// alone a fatal one).
#[test]
fn strict_broken_flag_forces_non_zero_exit_for_link_nested_in_mdx_jsx() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    // JSX-only fixture (no directive link at all) — the non-zero exit and
    // the named href can only come from THIS context.
    write_nested_fixture(root, &nested_config_json(false), true, false);

    let Some(output) = run_zfb_build(root, &["--strict-broken"]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        !output.status.success(),
        "`zfb build --strict-broken` must exit non-zero for a broken link \
         nested inside <Note>…</Note> — previously invisible entirely\n\
         status: {:?}\n--- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(JSX_NESTED_BROKEN_HREF),
        "diagnostic output must name the broken nested-JSX href \
         {JSX_NESTED_BROKEN_HREF:?}\n--- output ---\n{combined}"
    );
}

/// Case 7: the container-directive counterpart of case 6 — `:::note` is the
/// SAME underlying mechanism (`DirectiveRegistry` expands it to an
/// `MdxJsxFlowElement` at the mdast phase), so it must fail identically.
#[test]
fn strict_broken_flag_forces_non_zero_exit_for_link_nested_in_container_directive() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    // Directive-only fixture (no JSX link at all) — the non-zero exit and
    // the named href can only come from THIS context.
    write_nested_fixture(root, &nested_config_json(false), false, true);

    let Some(output) = run_zfb_build(root, &["--strict-broken"]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        !output.status.success(),
        "`zfb build --strict-broken` must exit non-zero for a broken link \
         nested inside :::note … ::: — previously invisible entirely\n\
         status: {:?}\n--- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(DIRECTIVE_NESTED_BROKEN_HREF),
        "diagnostic output must name the broken nested-directive href \
         {DIRECTIVE_NESTED_BROKEN_HREF:?}\n--- output ---\n{combined}"
    );
}

/// Case 8: the control proving the behavior change is severity-gated, not
/// unconditional — WITHOUT `--strict-broken` (and with `failOnBroken`
/// explicitly false), the SAME nested broken links must surface as
/// non-fatal warnings and the build must still succeed. Before #2184's fix
/// this fixture built clean with ZERO mention of either href at any
/// severity; after the fix it must build clean while naming both.
#[test]
fn control_nested_links_without_strict_flag_warn_but_build_succeeds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    // Both contexts together: this test never inspects exit-status-per-context
    // (the build must succeed regardless of which link "caused" anything), so
    // combining them here is not exposed to case 6/7's isolation concern.
    write_nested_fixture(root, &nested_config_json(false), true, true);

    let Some(output) = run_zfb_build(root, &[]) else {
        return;
    };
    let combined = combined_output(&output);

    assert!(
        output.status.success(),
        "`zfb build` with no strict-broken flag must exit 0 when \
         failOnBroken is false, even with broken links nested in <Note>/:::note\n\
         status: {:?}\n--- output ---\n{combined}",
        output.status,
    );
    assert!(
        combined.contains(JSX_NESTED_BROKEN_HREF)
            && combined.contains(DIRECTIVE_NESTED_BROKEN_HREF),
        "the build must still surface BOTH newly-detected nested broken \
         links as non-fatal warnings\n--- output ---\n{combined}"
    );
}
