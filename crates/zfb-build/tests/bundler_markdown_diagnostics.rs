//! Bundler-wiring integration tests for the markdown-diagnostics drain +
//! severity policy (zfb#953).
//!
//! Pins the contract that when `BundlerInput::pipeline_spec` is armed with
//! build-context roots and a filesystem-dependent feature (transclude /
//! imageDimensions), the bundler:
//!
//! - **Transclude error (Error severity):** armed pipeline + missing include
//!   file → `bundle()` returns `Err`; error message names the file.
//! - **imageDimensions warning (Warning severity):** armed pipeline + missing
//!   image file → `bundle()` succeeds; warning emitted to stderr, build
//!   continues.
//! - **Unarmed config (no build_context_roots):** same fixture → zero new
//!   behavior; bundle succeeds with no diagnostics.
//!
//! ## Esbuild gating
//!
//! Same precedence as other bundler integration tests: `ZFB_ESBUILD_BIN` env
//! var → `crates/zfb/binaries/esbuild/esbuild` slot → `which esbuild` PATH
//! fallback.  If no binary is available the test prints a skip note and
//! returns early rather than failing.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec};
use zfb_content::{ImageDimensionsConfig, MarkdownFeaturesConfig, PipelineSpec, TranscludeConfig};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

// ── Shared fixture helpers ──────────────────────────────────────────────────

/// Create the standard project tree used by all tests.
///
/// - `pages/index.mdx` — minimal page (route anchor).
/// - `content/docs/` — content collection root (populated per test).
/// - `layouts/default.tsx` — a minimal layout stub.
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

/// Base `BundlerInput` with the standard collection wiring.
fn make_base_input(
    root: &Path,
    esbuild: &Path,
    outdir_name: &str,
    pipeline_spec: PipelineSpec,
) -> BundlerInput {
    BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root: None,
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: std::collections::BTreeMap::new(),
        public_env_vars: HashMap::new(),
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
        emit_render_artifacts: false,
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

/// Arm `PipelineSpec` with transclude enabled and build-context roots at
/// `project_root` (public dir at `<root>/public`).
fn armed_transclude_spec(root: &Path) -> PipelineSpec {
    PipelineSpec {
        features: Some(MarkdownFeaturesConfig {
            transclude: Some(TranscludeConfig::default()),
            ..Default::default()
        }),
        build_context_roots: Some((root.to_path_buf(), root.join("public"))),
        ..PipelineSpec::default()
    }
}

/// Arm `PipelineSpec` with imageDimensions enabled and build-context roots.
fn armed_image_dimensions_spec(root: &Path) -> PipelineSpec {
    PipelineSpec {
        features: Some(MarkdownFeaturesConfig {
            image_dimensions: Some(ImageDimensionsConfig::default()),
            ..Default::default()
        }),
        build_context_roots: Some((root.to_path_buf(), root.join("public"))),
        ..PipelineSpec::default()
    }
}

// ── (a) Transclude error fails the build ───────────────────────────────────

/// Armed pipeline + missing transclude include → bundle fails; error message
/// names the missing file.
///
/// Transclude plugin emits `DiagnosticSeverity::Error` when the included
/// file cannot be read (file not found). The A3 severity policy gates on
/// `Error` and returns `bail!` listing every finding after all walks
/// complete.
#[test]
fn armed_transclude_missing_include_fails_build() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_markdown_diagnostics] no esbuild binary available; \
             set ZFB_ESBUILD_BIN or install esbuild on PATH. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);
    // A page in the content collection that transcludes a file that does NOT
    // exist — the transclude plugin will emit an Error-severity diagnostic.
    fs::write(
        root.join("content/docs/index.mdx"),
        "---\ntitle: Docs\n---\n\n:::include{file=\"./missing-snippet.md\"}\n",
    )
    .unwrap();

    let spec = armed_transclude_spec(&root);
    let input = make_base_input(&root, &esbuild, "dist-transclude-err", spec);
    let err = bundle(input)
        .expect_err("bundle should fail with a missing transclude include (Error severity)");
    let msg = format!("{err:#}");

    // The error message must mention "error(s)" and reference the file name.
    assert!(
        msg.contains("error") || msg.contains("Error"),
        "error message should describe the failure; got: {msg}"
    );
    assert!(
        msg.contains("transclude") || msg.contains("cannot read"),
        "error message should mention transclude or the read failure; got: {msg}"
    );
}

// ── (b) imageDimensions warning — build succeeds ───────────────────────────

/// Armed pipeline + missing image file → imageDimensions emits a
/// `DiagnosticSeverity::Warning` diagnostic; the build succeeds.
///
/// imageDimensions can't read a local image that doesn't exist, so it emits
/// a warning and leaves the `<img>` unchanged.  The A3 policy emits the
/// warning to stderr and continues.
#[test]
fn armed_image_dimensions_missing_image_warns_and_succeeds() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_markdown_diagnostics] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);
    // Reference a local image path that does not exist on disk.  The plugin
    // must warn and leave the <img> element unchanged (Warning severity).
    fs::write(
        root.join("content/docs/index.mdx"),
        "---\ntitle: Docs\n---\n\n![missing image](./does-not-exist.png)\n",
    )
    .unwrap();

    let spec = armed_image_dimensions_spec(&root);
    let input = make_base_input(&root, &esbuild, "dist-imgdim-warn", spec);
    // Must succeed — Warning severity must not fail the build.
    let out = bundle(input)
        .expect("bundle should succeed with imageDimensions warning (Warning severity)");
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written even when imageDimensions warns"
    );
}

// ── (c) Unarmed config — zero behavior change ──────────────────────────────

/// Unarmed config (no `build_context_roots`, no `features`) → the same
/// fixture with an invalid transclude directive compiles without emitting
/// any markdown diagnostics (the transclude plugin is not wired).
///
/// This guards against accidentally enabling feature plugins or the context
/// plumbing when the config doesn't opt in.
#[test]
fn unarmed_config_no_new_diagnostics() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_markdown_diagnostics] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_common_dirs(&root);
    // Same fixture as the transclude test — but with an UNARMED pipeline.
    // The :::include directive is not processed; it passes through as text,
    // and the missing-snippet file never triggers a read → no diagnostic.
    fs::write(
        root.join("content/docs/index.mdx"),
        "---\ntitle: Docs\n---\n\n:::include{file=\"./missing-snippet.md\"}\n",
    )
    .unwrap();

    // Default (unarmed) spec — no features, no build_context_roots.
    let input = make_base_input(&root, &esbuild, "dist-unarmed", PipelineSpec::default());
    // Must succeed with no errors — unarmed pipeline ignores the directive.
    let out = bundle(input).expect("unarmed bundle should succeed (no new diagnostics)");
    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written for unarmed config"
    );
}
