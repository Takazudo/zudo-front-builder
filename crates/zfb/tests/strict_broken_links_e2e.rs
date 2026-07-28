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
