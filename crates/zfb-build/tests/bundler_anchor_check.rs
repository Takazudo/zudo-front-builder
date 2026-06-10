//! Bundler-wiring integration tests for the post-compile cross-file anchor
//! check (zfb#980).
//!
//! Pins the contract that when `BundlerInput::pipeline_spec` is armed with
//! build-context roots and `linkValidation` enabled, the bundler post-compile
//! anchor check:
//!
//! - **Broken cross-file anchor, failOnBroken:true** → `bundle()` returns
//!   `Err`; error message references the broken fragment.
//! - **Broken cross-file anchor, failOnBroken:false** → `bundle()` succeeds;
//!   warning to stderr, build continues.
//! - **Valid cross-file anchor** → `bundle()` succeeds with no diagnostics.
//! - **Heading arrives via transclusion** → the transcluded heading is in the
//!   heading map; the anchor check passes.
//! - **Out-of-build target** → existence-only degrade: candidate is recorded
//!   but target is never compiled → check skipped, build succeeds.
//! - **Unarmed config** → unarmed pipeline produces no candidates and no
//!   headings → bundle byte-identical to pre-#980 (no new diagnostics).
//! - **Cache-hit replay equivalence** → running `bundle()` twice produces the
//!   same verdict on both runs (second run hits the compile cache).
//! - **Dev-tick-shaped double bundle** → proves `dev.rs` needs no changes:
//!   calling `bundle()` twice sequentially (mimicking dev tick replay) gives
//!   the same verdict.
//!
//! ## Esbuild gating
//!
//! Same as other bundler integration tests: `ZFB_ESBUILD_BIN` env var →
//! `crates/zfb/binaries/esbuild/esbuild` slot → `which esbuild` PATH fallback.
//! If no binary is available, tests print a skip note and return early.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec};
use zfb_content::{LinkValidationConfig, MarkdownFeaturesConfig, PipelineSpec, TranscludeConfig};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

// ── Shared fixture helpers ──────────────────────────────────────────────────

/// Create the standard project tree used by all anchor-check tests.
///
/// - `pages/index.mdx` — minimal page (route anchor).
/// - `content/docs/` — content collection root (populated per test).
/// - `layouts/default.tsx` — minimal layout stub.
fn write_common_dirs(root: &Path) {
    for d in ["pages", "content/docs", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Home\n---\n\nWelcome.\n",
    )
    .unwrap();
}

/// Base `BundlerInput` with the standard docs collection wiring.
fn make_base_input(
    root: &Path,
    esbuild: &Path,
    outdir_name: &str,
    pipeline_spec: PipelineSpec,
) -> BundlerInput {
    BundlerInput {
        main_fields: Vec::new(),
        project_root: root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![ContentCollectionSpec::new(
            "docs",
            PathBuf::from("content/docs"),
        )],
        pipeline_spec,
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
    }
}

/// Arm `PipelineSpec` with `linkValidation` enabled and `build_context_roots`
/// set. `fail_on_broken` controls whether broken anchors are errors or warnings.
fn armed_link_validation_spec(root: &Path, fail_on_broken: bool) -> PipelineSpec {
    PipelineSpec {
        features: Some(MarkdownFeaturesConfig {
            link_validation: Some(LinkValidationConfig {
                fail_on_broken: Some(fail_on_broken),
            }),
            ..Default::default()
        }),
        build_context_roots: Some((root.to_path_buf(), root.join("public"))),
        ..PipelineSpec::default()
    }
}

/// Arm `PipelineSpec` with both `linkValidation` and `transclude` enabled.
fn armed_link_validation_with_transclude_spec(root: &Path, fail_on_broken: bool) -> PipelineSpec {
    PipelineSpec {
        features: Some(MarkdownFeaturesConfig {
            link_validation: Some(LinkValidationConfig {
                fail_on_broken: Some(fail_on_broken),
            }),
            transclude: Some(TranscludeConfig::default()),
            ..Default::default()
        }),
        build_context_roots: Some((root.to_path_buf(), root.join("public"))),
        ..PipelineSpec::default()
    }
}

// ── (1) Broken cross-file anchor fails an armed build with failOnBroken:true ─

/// Armed pipeline + failOnBroken:true + `./target.md#typo` that references a
/// heading that doesn't exist → `bundle()` returns `Err`.
///
/// Scenario: `source.mdx` links to `target.mdx#nonexistent-heading`.
/// `target.mdx` has heading `## Real Heading` (id=`real-heading`).
/// The candidate is recorded at compile time; the post-compile check resolves
/// the fragment against the heading map and synthesizes an Error-severity
/// `BrokenLink` diagnostic → the build fails.
#[test]
fn broken_cross_file_anchor_fails_armed_build_with_fail_on_broken() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_anchor_check] no esbuild binary available; \
             set ZFB_ESBUILD_BIN or install esbuild on PATH. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    // target.mdx: has heading "Real Heading" (id = "real-heading")
    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Real Heading\n\nSome content.\n",
    )
    .unwrap();

    // source.mdx: links to ./target.mdx#nonexistent-heading (typo)
    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this link](./target.mdx#nonexistent-heading).\n",
    )
    .unwrap();

    let spec = armed_link_validation_spec(&root, true);
    let input = make_base_input(&root, &esbuild, "dist-anchor-fail", spec);
    let err = bundle(input)
        .expect_err("bundle should fail: broken cross-file anchor with failOnBroken:true");
    let msg = format!("{err:#}");

    assert!(
        msg.contains("broken link") || msg.contains("error"),
        "error message should describe the broken-link finding; got: {msg}"
    );
    assert!(
        msg.contains("nonexistent-heading") || msg.contains("target.mdx"),
        "error message should reference the broken fragment or file; got: {msg}"
    );
}

// ── (2) Broken cross-file anchor warns and succeeds with failOnBroken:false ──

/// Armed pipeline + failOnBroken:false + broken cross-file anchor → `bundle()`
/// succeeds; warning emitted to stderr (not testable here), build continues.
#[test]
fn broken_cross_file_anchor_warns_and_succeeds_with_fail_on_broken_false() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Real Heading\n\nSome content.\n",
    )
    .unwrap();

    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this link](./target.mdx#nonexistent-heading).\n",
    )
    .unwrap();

    // failOnBroken:false → Warning severity → build succeeds
    let spec = armed_link_validation_spec(&root, false);
    let input = make_base_input(&root, &esbuild, "dist-anchor-warn", spec);
    let out = bundle(input).expect(
        "bundle should succeed: broken cross-file anchor with failOnBroken:false is a warning",
    );
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written even when broken anchor is a warning"
    );
}

// ── (3) Valid cross-file anchor passes ─────────────────────────────────────

/// Armed pipeline + valid cross-file anchor (`./target.mdx#real-heading`) →
/// `bundle()` succeeds with no diagnostics.
#[test]
fn valid_cross_file_anchor_passes() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Real Heading\n\nSome content.\n",
    )
    .unwrap();

    // source.mdx links to the exact heading id that target.mdx produces
    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this link](./target.mdx#real-heading).\n",
    )
    .unwrap();

    let spec = armed_link_validation_spec(&root, true);
    let input = make_base_input(&root, &esbuild, "dist-anchor-valid", spec);
    let out = bundle(input).expect("bundle should succeed: valid cross-file anchor");
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written for a valid cross-file anchor"
    );
}

// ── (4) Heading arrives via transclusion ────────────────────────────────────

/// Armed pipeline with transclude + linkValidation: when the target heading
/// comes from an `:::include{file=…}` directive that inlines a heading from
/// another snippet, the bundler's heading map captures it and the anchor check
/// passes.
///
/// Fixture:
///   - `snippet.md`: `## Transcluded Heading`
///   - `target.mdx`: transcludes `snippet.md` → effective heading id = `transcluded-heading`
///   - `source.mdx`: links to `./target.mdx#transcluded-heading`
#[test]
fn heading_from_transclusion_passes_anchor_check() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    // snippet.md: the heading that will be transcluded
    fs::write(
        root.join("content/docs/snippet.md"),
        "## Transcluded Heading\n\nSnippet body.\n",
    )
    .unwrap();

    // target.mdx: transcludes snippet.md so the heading appears in target's AST
    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n:::include{file=\"./snippet.md\"}\n",
    )
    .unwrap();

    // source.mdx: links to the transcluded heading id in target.mdx
    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this link](./target.mdx#transcluded-heading).\n",
    )
    .unwrap();

    let spec = armed_link_validation_with_transclusion_spec(&root, true);
    let input = make_base_input(&root, &esbuild, "dist-anchor-transclusion", spec);
    let out = bundle(input)
        .expect("bundle should succeed: heading provided by transclusion is in the heading map");
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written when transcluded heading satisfies anchor check"
    );
}

// ── (5) Out-of-build target → skipped (existence-only) ────────────────────

/// Armed pipeline + link to a file outside the bundler's walked dirs
/// (`./outside.md#heading`). The target is never compiled → no `FileHeadings`
/// entry → the post-compile check skips the candidate (existence-only degrade).
/// Build succeeds even with failOnBroken:true.
///
/// The link target exists on disk (so the per-compile existence check passes)
/// but is not inside any walked collection or pages dir.
#[test]
fn out_of_build_target_skipped_existence_only() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    // outside.md: exists on disk but is NOT inside content/docs (not walked)
    fs::write(
        root.join("outside.md"),
        "## Out Of Build Heading\n\nNot compiled.\n",
    )
    .unwrap();

    // source.mdx: links to outside.md#out-of-build-heading
    // The link target exists on disk → per-compile existence check passes.
    // The target has no FileHeadings entry → post-compile check skips it.
    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this link](../../outside.md#out-of-build-heading).\n",
    )
    .unwrap();

    // Even with failOnBroken:true, out-of-build targets are skipped.
    let spec = armed_link_validation_spec(&root, true);
    let input = make_base_input(&root, &esbuild, "dist-anchor-out-of-build", spec);
    let out = bundle(input)
        .expect("bundle should succeed: out-of-build target is skipped (existence-only degrade)");
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written for out-of-build target (existence-only)"
    );
}

// ── (6) Unarmed config — zero new diagnostics (extend unarmed guard) ────────

/// Unarmed config (no `build_context_roots`, no `features`) with the same
/// broken cross-file anchor fixture → zero new behavior; bundle succeeds.
///
/// Guards against accidentally enabling the anchor check for unarmed configs.
#[test]
fn unarmed_config_no_new_diagnostics() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Real Heading\n\nSome content.\n",
    )
    .unwrap();

    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this link](./target.mdx#nonexistent-heading).\n",
    )
    .unwrap();

    // Default (unarmed) spec — no features, no build_context_roots.
    // The link validation plugin is not wired, so no candidates are recorded.
    let input = make_base_input(
        &root,
        &esbuild,
        "dist-anchor-unarmed",
        PipelineSpec::default(),
    );
    let out = bundle(input).expect("unarmed bundle should succeed (no new diagnostics)");
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written for unarmed config"
    );
}

// ── (7) Cache-hit replay equivalence ────────────────────────────────────────

/// Bundle twice sequentially with the same fixture. The second run hits the
/// process-global compile cache (zfb#905). The cache-hit replay path re-injects
/// both `CrossFileLinkCandidate`s and `FileHeadings` (#977), so the
/// post-compile anchor check must produce the same verdict on both runs.
///
/// Scenario: broken anchor + failOnBroken:true → both runs fail.
#[test]
fn cache_hit_replay_same_verdict_both_runs() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Real Heading\n\nContent.\n",
    )
    .unwrap();

    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this](./target.mdx#nonexistent-heading).\n",
    )
    .unwrap();

    let spec = armed_link_validation_spec(&root, true);

    // First run: fresh compile, builds the cache.
    let input1 = make_base_input(&root, &esbuild, "dist-anchor-cache1", spec.clone());
    let err1 = bundle(input1).expect_err("first bundle run should fail (broken anchor)");
    let msg1 = format!("{err1:#}");

    // Second run: should hit the compile cache; verdict must be identical.
    let input2 = make_base_input(&root, &esbuild, "dist-anchor-cache2", spec);
    let err2 = bundle(input2).expect_err("second bundle run should also fail (cache hit)");
    let msg2 = format!("{err2:#}");

    // Both runs must report the same broken-link finding.
    assert!(
        msg1.contains("nonexistent-heading") || msg1.contains("broken link"),
        "first run error must reference the broken fragment; got: {msg1}"
    );
    assert!(
        msg2.contains("nonexistent-heading") || msg2.contains("broken link"),
        "second run (cache hit) error must reference the broken fragment; got: {msg2}"
    );
}

// ── (8) Dev-tick-shaped double-bundle — proves dev.rs needs no changes ──────

/// Simulate two consecutive dev ticks by calling `bundle()` twice with the
/// same spec and fixture. Dev mode re-runs `bundle()` on every tick, so the
/// second call is the "cache warm" tick. Both must agree: a valid anchor
/// passes on tick 1 AND tick 2 (cache replay).
///
/// This proves `dev.rs` needs NO changes — cache-hit replay is fully handled
/// by the materialise + pipeline drain chain.
#[test]
fn dev_tick_shaped_double_bundle_valid_anchor() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_anchor_check] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);

    fs::write(
        root.join("content/docs/target.mdx"),
        "---\ntitle: Target\n---\n\n## Valid Heading\n\nContent.\n",
    )
    .unwrap();

    fs::write(
        root.join("content/docs/source.mdx"),
        "---\ntitle: Source\n---\n\nSee [this](./target.mdx#valid-heading).\n",
    )
    .unwrap();

    let spec = armed_link_validation_spec(&root, true);

    // Tick 1: fresh compile.
    let input1 = make_base_input(&root, &esbuild, "dist-dev-tick1", spec.clone());
    let out1 = bundle(input1).expect("dev tick 1 should succeed (valid anchor)");
    assert!(out1.bundle_path.exists(), "tick 1 bundle must be written");

    // Tick 2: cache warm (all files cached from tick 1).
    let input2 = make_base_input(&root, &esbuild, "dist-dev-tick2", spec);
    let out2 = bundle(input2).expect("dev tick 2 (cache warm) should succeed (valid anchor)");
    assert!(out2.bundle_path.exists(), "tick 2 bundle must be written");
}

// ── Helper used by transclusion test ────────────────────────────────────────

/// Same as `armed_link_validation_with_transclude_spec` but named consistently
/// with the test that uses it.
fn armed_link_validation_with_transclusion_spec(root: &Path, fail_on_broken: bool) -> PipelineSpec {
    armed_link_validation_with_transclude_spec(root, fail_on_broken)
}
