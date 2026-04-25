//! Integration-style tests for the public surface of `zfb-css`.
//!
//! These exercise the engine trait, the CSS Modules processor, and the
//! top-level `CssPipeline`. Tests that need the real Tailwind v4 binary
//! are gated by `#[ignore]` and a comment — they will be enabled in a
//! release-engineering follow-up once the binary is checked in to
//! `crates/zfb/binaries/tailwindcss-v4` (Topic B / Sub 4 reserves the slot
//! but does not yet download the binary).

use std::path::{Path, PathBuf};

use zfb_css::{
    link_href, CssEngine, CssPipeline, CssPipelineConfig, CssModulesOutput,
    CssModulesProcessor, NativeRustEngine, TailwindSubprocessConfig,
    TailwindSubprocessEngine,
};

#[test]
fn native_engine_returns_not_implemented_error() {
    let engine = NativeRustEngine::new();
    let err = engine
        .produce_utility_css(&[PathBuf::from("pages/index.tsx")])
        .expect_err("NativeRustEngine must return an error");
    let msg = format!("{err}");
    assert!(
        msg.contains("not yet implemented"),
        "expected 'not yet implemented' in error, got: {msg}"
    );
}

#[test]
fn subprocess_engine_mock_short_circuits_command() {
    // Use the mock-output escape hatch so this test does not require the
    // tailwindcss binary to be present.
    let cfg = TailwindSubprocessConfig::default()
        .with_mock_output(".mock-utility { color: red; }\n");
    let engine = TailwindSubprocessEngine::new(cfg);
    let css = engine
        .produce_utility_css(&[PathBuf::from("pages/index.tsx")])
        .expect("mock engine should succeed");
    assert!(css.contains(".mock-utility"));
}

#[test]
#[ignore = "Requires the real tailwindcss v4 binary at crates/zfb/binaries/tailwindcss-v4. \
            Will be enabled in a release-engineering follow-up (Topic B / Sub 4)."]
fn subprocess_engine_against_real_binary() {
    let engine = TailwindSubprocessEngine::with_default_config();
    let css = engine
        .produce_utility_css(&[PathBuf::from("pages/index.tsx")])
        .expect("real tailwindcss binary should produce CSS");
    assert!(!css.is_empty(), "real engine should not return empty CSS");
}

#[test]
fn subprocess_engine_reports_missing_binary_clearly() {
    let cfg = TailwindSubprocessConfig::default()
        .with_binary_path("/nonexistent/tailwindcss-v4-please-do-not-create");
    let engine = TailwindSubprocessEngine::new(cfg);
    let err = engine
        .produce_utility_css(&[])
        .expect_err("missing binary must error");
    let msg = format!("{err}");
    assert!(msg.contains("not found"), "got: {msg}");
}

#[test]
fn css_modules_processor_scopes_class_names() {
    let proc = CssModulesProcessor::with_default_config();
    let path = Path::new("button.module.css");
    let source = ".btn { color: blue; }\n.btn-primary { font-weight: bold; }\n";

    let (css, names) = proc
        .process_source(path, source)
        .expect("CSS Modules processing must succeed");

    // Original names appear as keys.
    assert!(names.contains_key("btn"), "names: {names:?}");
    assert!(names.contains_key("btn-primary"), "names: {names:?}");

    // Each scoped name must NOT equal the original (lightningcss rewrites
    // them under the default pattern).
    assert_ne!(names.get("btn"), Some(&"btn".to_string()));
    assert_ne!(names.get("btn-primary"), Some(&"btn-primary".to_string()));

    // The compiled CSS contains the scoped names.
    let scoped_btn = names.get("btn").expect("btn entry");
    assert!(
        css.contains(scoped_btn),
        "compiled CSS must reference scoped name {scoped_btn}, got: {css}"
    );
}

#[test]
fn css_modules_processor_handles_empty_input() {
    let proc = CssModulesProcessor::with_default_config();
    let out: CssModulesOutput = proc.process(&[]).expect("empty input must succeed");
    assert!(out.css.is_empty());
    assert!(out.class_maps.is_empty());
}

#[test]
fn pipeline_writes_hashed_asset_with_mock_engine() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output_root = tmp.path().to_path_buf();

    let engine = TailwindSubprocessEngine::new(
        TailwindSubprocessConfig::default()
            .with_mock_output(".u-text-red { color: red; }\n"),
    );
    let cfg = CssPipelineConfig {
        sources: vec![PathBuf::from("pages/index.tsx")],
        css_modules: vec![],
        output_root: output_root.clone(),
        base_url: "/".to_string(),
        ..CssPipelineConfig::default()
    };

    let pipeline = CssPipeline::new(engine, cfg);
    let out = pipeline.build().expect("pipeline build");

    assert_eq!(out.hash.len(), 8);
    assert!(out.css.contains(".u-text-red"));
    assert!(out.asset_path.starts_with(&output_root));
    assert!(out.asset_path.exists(), "asset must be written to disk");
    let on_disk = std::fs::read_to_string(&out.asset_path).expect("read asset");
    assert_eq!(on_disk, out.css);

    // link_href derives the correct public URL.
    let href = link_href("/", &out.asset_path);
    let expected = format!("/assets/styles-{}.css", out.hash);
    assert_eq!(href, expected);
}

#[test]
fn pipeline_hash_changes_when_engine_output_changes() {
    let tmp1 = tempfile::tempdir().expect("tempdir");
    let tmp2 = tempfile::tempdir().expect("tempdir");

    let make = |output: &str, root: &Path| {
        let engine = TailwindSubprocessEngine::new(
            TailwindSubprocessConfig::default().with_mock_output(output),
        );
        let cfg = CssPipelineConfig {
            output_root: root.to_path_buf(),
            ..CssPipelineConfig::default()
        };
        CssPipeline::new(engine, cfg).build().expect("build")
    };

    let a = make(".x { color: red }", tmp1.path());
    let b = make(".x { color: green }", tmp2.path());
    assert_ne!(
        a.hash, b.hash,
        "changing the engine's CSS output must change the hash"
    );
}
