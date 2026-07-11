//! Confirm sub (zfb#1535): a class-mode project's bundler build-output ties
//! together token EMISSION (the content pipeline's `hi-kw`/`hi-root`
//! markup, zfb#1532) and framework CSS INJECTION
//! (`zfb_css::default_hi_css()`, zfb#1533).
//!
//! Models the wiring-proof shape of
//! `bundler_default_plugins.rs::bundler_threads_default_plugins_through_mdx_compile`
//! (the `syntect-` class-hook marker check at that file's line 208-216),
//! but for `codeHighlight.mode: "class"` instead of the inline default.
//!
//! ## Scope
//!
//! - **Emission**: runs the REAL bundler (`zfb_build::bundle`) against a
//!   fixture MDX page carrying a fenced `rust` block, with
//!   `pipeline_spec.code_highlight_mode = Class`. Asserts the compiled
//!   bundle body contains `hi-root` and `hi-kw` — the same class-mode
//!   markers `crates/zfb-content/tests/class_mode_pipeline.rs` proves at
//!   the content-pipeline layer, now proven all the way through MDX
//!   compile + esbuild, i.e. a real *built page*.
//! - **Injection**: drives `zfb_css::CssPipeline` directly with
//!   `framework_css: Some(zfb_css::default_hi_css())` — the exact value
//!   `crates/zfb/src/commands/build.rs::resolve_framework_css` computes
//!   for a class-mode `Config` with the default `classPrefix` (see that
//!   file's `css_payload_ships_default_hi_stylesheet_in_class_mode` unit
//!   test) — and asserts the emitted `styles-*.css` bytes contain
//!   `--zfb-hi-kw`.
//!
//! `zfb-build` cannot itself resolve `codeHighlight` from a user's
//! `zfb.config.ts` — that Config->knobs conversion
//! (`resolve_framework_css`, `pipeline_spec_from_config`) is
//! `zfb`-crate-private (issues #1530/#1533) and unit-tested there. This
//! test proves the two class-mode building blocks `zfb-build`/`zfb-css`
//! themselves own — the bundler's content pipeline and the CSS pipeline's
//! `framework_css` knob — agree on the SAME class names (`hi-root`/
//! `hi-kw`) under the shared default `classPrefix` ("hi-"), so a real
//! built page and its stylesheet are never out of sync at this layer.
//!
//! ## Esbuild gating
//!
//! Same precedence as `bundler_default_plugins.rs`:
//! `ZFB_ESBUILD_BIN` env var -> `crates/zfb/binaries/esbuild/esbuild`
//! slot -> `which esbuild` PATH fallback. If no binary is available the
//! test prints a skip note and returns early.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_content::{CodeHighlightMode, PipelineSpec};
use zfb_css::{AuthoredCssEngine, CssPipeline, CssPipelineConfig};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// A minimal project with one MDX page carrying a fenced `rust` block —
/// enough to exercise the class-mode syntect path end-to-end through the
/// bundler.
fn write_fixture_project(root: &std::path::Path) {
    for d in ["pages", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Class Mode Confirm\n---\n\n```rust\nfn main() {}\n```\n",
    )
    .unwrap();
}

#[test]
fn bundler_class_mode_project_ties_emission_and_css_injection() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_class_mode_confirm] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let pipeline_spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        ..Default::default()
    };
    // `PipelineSpec::default()` already carries `classPrefix: "hi-"` — the
    // same default `resolve_framework_css` assumes in
    // `crates/zfb/src/commands/build.rs`. Assert it explicitly so a future
    // default-prefix change fails loudly here instead of silently
    // desyncing this fixture from the CSS assertions below.
    assert_eq!(pipeline_spec.code_highlight_class_prefix, "hi-");

    let input = BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: root.clone(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root: None,
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: BTreeMap::new(),
        public_env_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: Vec::new(),
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
    };

    let out = bundle(input).expect("bundle should succeed");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(!body.is_empty(), "bundle should be non-empty");

    // ---- Emission: class-mode markup survives MDX compile + esbuild ----
    assert!(
        body.contains("hi-root"),
        "class-mode pre root class must reach the compiled bundle.\n--- bundle excerpt ---\n{}",
        &body[..body.len().min(2000)]
    );
    assert!(
        body.contains("hi-kw"),
        "class-mode role class must reach the compiled bundle.\n--- bundle excerpt ---\n{}",
        &body[..body.len().min(2000)]
    );
    // The inline-mode class-hook must be ABSENT — proves this really is
    // the class-mode path, not a stale inline fallback (regression guard
    // mirroring `class_mode_pipeline.rs::class_pipeline_emits_role_classes_and_root_class`
    // at the bundler layer).
    assert!(
        !body.contains("syntect-"),
        "class mode must NOT emit the inline syntect- class hook.\n--- bundle excerpt ---\n{}",
        &body[..body.len().min(2000)]
    );

    // ---- Injection: the matching stylesheet ships --zfb-hi-kw ----------
    let css_pipeline = CssPipeline::new(
        AuthoredCssEngine::new(String::new()),
        CssPipelineConfig {
            framework_css: Some(zfb_css::default_hi_css().to_string()),
            ..CssPipelineConfig::default()
        },
    );
    let css_out = css_pipeline
        .build_emitter()
        .expect("css emitter must succeed");
    let css_text = String::from_utf8(css_out.bytes).expect("css bytes must be utf8");
    assert!(
        css_text.contains("--zfb-hi-kw"),
        "framework stylesheet must ship the --zfb-hi-kw token: {css_text}"
    );
    assert!(
        css_text.contains(".hi-kw"),
        "framework stylesheet must ship the .hi-kw role selector: {css_text}"
    );
}
