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
    build_synthesised_entry_css, link_href, scan_css_module_imports, CssEngine, CssPipeline,
    CssPipelineConfig, CssModulesOutput, CssModulesProcessor, NativeRustEngine,
    TailwindSubprocessConfig, TailwindSubprocessEngine,
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

// ----------------------------------------------------------------------
// Acceptance fixture (Sub 9 of issue #53):
//
// CSS Modules + Tailwind utilities + cross-package framework globs.
// We can't run the real Tailwind binary in CI yet (the slot is reserved
// but not populated — Topic B / Sub 4). So this test:
//
// 1. Materialises a fake user project + a fake framework package on
//    disk — they each contain TSX with both Tailwind utility classes
//    and `import * from "*.module.css"` statements.
// 2. Runs the pipeline with a mocked Tailwind engine. The mock returns
//    a CSS string that includes class names from both packages — this
//    simulates the real binary scanning both `@source` directives and
//    keeping framework-only classes.
// 3. Asserts the synthesised entry CSS the engine *would have* fed to
//    Tailwind contains `@source` directives for both packages, in the
//    correct order, plus the user's `@theme {…}` block.
// 4. Asserts CSS Modules auto-discovery picked up the modules from
//    both the user TSX and the framework TSX, and that the output
//    stylesheet contains the scoped class name (proves real CSS
//    Modules processing happened end-to-end on both packages).
// 5. Asserts the framework-only utility class is present in the
//    combined CSS output (verifies the cross-package content-glob
//    plumbing — when the real binary lands, this check reduces to
//    "the binary kept the class" because we put the class in the
//    `@source` set).
// 6. Asserts the JSON class-name maps were emitted (the JS-side
//    rewrite contract).
// ----------------------------------------------------------------------

#[test]
fn acceptance_css_modules_plus_tailwind_with_framework_package_globs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // ---- user project ----
    let user_proj = root.join("project");
    let user_pages = user_proj.join("pages");
    std::fs::create_dir_all(&user_pages).unwrap();
    let user_tsx = user_pages.join("index.tsx");
    std::fs::write(
        &user_tsx,
        r#"import styles from "./button.module.css";
export default function Page() {
    return <div className={"flex items-center " + styles.btn}>hello</div>;
}
"#,
    )
    .unwrap();
    let user_module = user_pages.join("button.module.css");
    std::fs::write(
        &user_module,
        ".btn { color: red; }\n.btn-primary { font-weight: bold; }\n",
    )
    .unwrap();
    let user_global_css = user_proj.join("styles");
    std::fs::create_dir_all(&user_global_css).unwrap();
    let user_global_css_file = user_global_css.join("global.css");
    std::fs::write(
        &user_global_css_file,
        "@theme {\n  --color-brand: #123456;\n}\n",
    )
    .unwrap();

    // ---- framework package (mimics Phase B's packages/zudo-doc-v2/) ----
    let fw_pkg = root.join("packages").join("zudo-doc-v2");
    let fw_components = fw_pkg.join("components");
    std::fs::create_dir_all(&fw_components).unwrap();
    let fw_tsx = fw_components.join("doc-shell.tsx");
    std::fs::write(
        &fw_tsx,
        r#"import shellStyles from "./shell.module.css";
export function DocShell() {
    return <aside className={"prose-zfb-only " + shellStyles.shell}>fw</aside>;
}
"#,
    )
    .unwrap();
    let fw_module = fw_components.join("shell.module.css");
    std::fs::write(
        &fw_module,
        ".shell { padding: 1rem; }\n",
    )
    .unwrap();

    // ---- Tailwind engine config (mocked subprocess) ----
    // We feed a mock output that simulates the real binary scanning
    // both @source globs and emitting utilities for both pages —
    // including the framework-only `prose-zfb-only` class.
    let mock_tailwind = ".flex{display:flex}.items-center{align-items:center}.prose-zfb-only{max-width:65ch}\n";

    let tw_cfg = TailwindSubprocessConfig::default()
        .with_working_dir(&user_proj)
        .with_input_css(&user_global_css_file)
        .with_content_globs(vec!["pages/**/*.{tsx,jsx,ts,js}"])
        .with_framework_package_globs(vec![format!(
            "{}/**/*.{{tsx,jsx,ts,js}}",
            fw_pkg.display()
        )])
        .with_theme_block("@theme inline {\n  --font-display: 'Inter';\n}\n")
        .with_mock_output(mock_tailwind);
    let engine = TailwindSubprocessEngine::new(tw_cfg);

    // Sources fed to the pipeline: the user's TSX *and* the framework
    // TSX. The pipeline's auto-discover will pull in both .module.css
    // files via the scanner.
    let sources = vec![user_tsx.clone(), fw_tsx.clone()];

    let class_map_dir = root.join("dist").join("css-modules");
    let cfg = CssPipelineConfig {
        sources: sources.clone(),
        css_modules: vec![],
        output_root: root.join("dist"),
        base_url: "/".to_string(),
        auto_discover_modules: true,
        class_map_dir: Some(class_map_dir.clone()),
        ..CssPipelineConfig::default()
    };

    // ---- Run ----
    let pipeline = CssPipeline::new(engine, cfg);
    let out = pipeline.build().expect("acceptance pipeline build");

    // Re-borrow the engine to inspect the synthesised entry CSS.
    let engine_ref: &TailwindSubprocessEngine = pipeline.engine_ref();
    let entry = engine_ref
        .last_entry_css()
        .expect("synthesised entry CSS must be recorded");

    // (3) Synthesised entry CSS contains both user-project and
    // framework-package @source directives, plus the user's @theme.
    assert!(
        entry.contains("@import \"tailwindcss\""),
        "entry CSS must include the tailwind import; got:\n{entry}"
    );
    assert!(
        entry.contains("@source \"pages/**/*.{tsx,jsx,ts,js}\""),
        "entry CSS must include the user-project content glob; got:\n{entry}"
    );
    let fw_marker = format!("{}/**/", fw_pkg.display());
    assert!(
        entry.contains(&fw_marker),
        "entry CSS must include the framework-package content glob ({fw_marker}); got:\n{entry}"
    );
    let user_at_pos = entry
        .find("@source \"pages/**/*.{tsx,jsx,ts,js}\"")
        .expect("user @source position");
    let fw_at_pos = entry.find(&fw_marker).expect("fw @source position");
    assert!(
        user_at_pos < fw_at_pos,
        "user-project @source must precede framework @source in cascade order"
    );
    assert!(
        entry.contains("--color-brand"),
        "entry CSS must include the user's @theme block contents; got:\n{entry}"
    );
    assert!(
        entry.contains("--font-display"),
        "entry CSS must include the inline theme_block contents; got:\n{entry}"
    );

    // (4) Auto-discovery picked up *both* modules. Per-source
    // metadata reflects which TSX uses which module.
    assert_eq!(out.class_maps.len(), 2, "two .module.css files processed");
    assert!(
        out.class_maps.contains_key(&user_module),
        "user button.module.css must be processed"
    );
    assert!(
        out.class_maps.contains_key(&fw_module),
        "framework shell.module.css must be processed"
    );
    let user_btn_scoped = out
        .class_maps
        .get(&user_module)
        .and_then(|m| m.get("btn"))
        .expect("user btn scoped name");
    assert_ne!(user_btn_scoped, "btn", "btn must be hashed");
    assert!(
        out.css.contains(user_btn_scoped),
        "combined output must contain the scoped user btn"
    );

    // per_source_modules: one entry per source TSX, pointing at its
    // own module.
    assert_eq!(out.per_source_modules.len(), 2);
    let user_usage = out
        .per_source_modules
        .iter()
        .find(|u| u.source == user_tsx)
        .expect("user source usage entry");
    assert_eq!(user_usage.modules, vec![user_module.clone()]);
    let fw_usage = out
        .per_source_modules
        .iter()
        .find(|u| u.source == fw_tsx)
        .expect("fw source usage entry");
    assert_eq!(fw_usage.modules, vec![fw_module.clone()]);

    // (5) Framework-only utility class survives in the combined
    // stylesheet — proves the cross-package content-glob plumbing
    // would carry it through the real binary too.
    assert!(
        out.css.contains(".prose-zfb-only"),
        "framework-only utility class must survive in the combined output:\n{}",
        out.css
    );

    // (6) JSON class-name maps emitted to the configured directory.
    assert_eq!(
        out.class_map_files.len(),
        2,
        "one classes.json per .module.css"
    );
    for (module_path, json_path) in &out.class_map_files {
        assert!(
            json_path.starts_with(&class_map_dir),
            "json {} must live under class_map_dir {}",
            json_path.display(),
            class_map_dir.display()
        );
        assert!(json_path.exists(), "json {} must exist", json_path.display());
        let body = std::fs::read_to_string(json_path).unwrap();
        let parsed: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&body).unwrap_or_else(|e| {
                panic!("invalid JSON for {}: {e}\n{body}", module_path.display())
            });
        // Each map must contain at least one binding.
        assert!(!parsed.is_empty());
    }

    // Sanity: the asset file is on disk and link_href works.
    assert!(out.asset_path.exists());
    let href = link_href("/", &out.asset_path);
    assert_eq!(href, format!("/assets/styles-{}.css", out.hash));
}

#[test]
fn synthesised_entry_drops_duplicate_tailwind_import_from_user_css() {
    let cfg = TailwindSubprocessConfig::default()
        .with_content_globs(vec!["pages/**/*.tsx"]);
    let user = "@import \"tailwindcss\";\n.foo { color: red; }\n";
    let out = build_synthesised_entry_css(&cfg, Some(user));
    let occurrences = out.matches("@import \"tailwindcss\"").count();
    assert_eq!(
        occurrences, 1,
        "duplicate tailwind import must be elided; got:\n{out}"
    );
    assert!(out.contains(".foo"));
    assert!(out.contains("@source \"pages/**/*.tsx\""));
}

#[test]
fn scanner_finds_module_imports_in_real_tsx_file() {
    let tmp = tempfile::tempdir().unwrap();
    let tsx = tmp.path().join("page.tsx");
    std::fs::write(
        &tsx,
        r#"
        import styles from "./local.module.css";
        // import "./commented.module.css";
        import "./side.module.css";
        import other from "./other.css"; // not a module
        "#,
    )
    .unwrap();

    let scan = scan_css_module_imports(std::slice::from_ref(&tsx)).expect("scan");
    assert_eq!(scan.modules.len(), 2);
    assert!(
        scan.modules
            .iter()
            .any(|p| p.ends_with("local.module.css")),
        "scan: {scan:?}"
    );
    assert!(
        scan.modules.iter().any(|p| p.ends_with("side.module.css")),
        "scan: {scan:?}"
    );
}

// ----------------------------------------------------------------------
// Bytes-only emitter adapter (Prod Asset Graph S2).
//
// The new `CssPipeline::build_emitter()` entry point is what
// `ProductionAssetPipeline` calls — bytes + stable URL, no disk write
// for the hashed asset. The existing `build()` keeps writing the
// hashed file directly for any caller that still depends on that
// contract.
// ----------------------------------------------------------------------

#[test]
fn build_emitter_returns_bytes_and_stable_url_without_writing_asset() {
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
    let out = pipeline
        .build_emitter()
        .expect("build_emitter must succeed");

    // Bytes are populated and contain the engine's CSS.
    assert!(!out.bytes.is_empty(), "emitter bytes must not be empty");
    let s = std::str::from_utf8(&out.bytes).expect("utf8");
    assert!(
        s.contains(".u-text-red"),
        "emitter bytes must include the engine output, got: {s}"
    );

    // Stable URL matches the cross-crate constant.
    assert_eq!(out.stable_url, zfb_types::STABLE_CSS_URL);

    // Crucially: no `assets/` directory was written under output_root.
    // `ProductionAssetPipeline` owns the hashed-file write; emitter
    // double-writes would race with it.
    let assets_dir = output_root.join("assets");
    assert!(
        !assets_dir.exists(),
        "build_emitter must NOT write the hashed asset to disk; found {}",
        assets_dir.display()
    );
}

#[test]
fn build_emitter_bytes_match_build_output_for_same_inputs() {
    // The bytes-only entry point and the file-writing entry point
    // must agree on byte content for the same engine output, so
    // `ProductionAssetPipeline`'s hash matches what `build()` would
    // have computed locally — guards against a divergent code path
    // emitting different separators or trailing whitespace.
    let mock = ".u-x { color: red; }\n";

    let tmp1 = tempfile::tempdir().expect("tempdir");
    let cfg1 = CssPipelineConfig {
        sources: vec![PathBuf::from("pages/index.tsx")],
        output_root: tmp1.path().to_path_buf(),
        ..CssPipelineConfig::default()
    };
    let p1 = CssPipeline::new(
        TailwindSubprocessEngine::new(
            TailwindSubprocessConfig::default().with_mock_output(mock),
        ),
        cfg1,
    );
    let built = p1.build().expect("build");

    let tmp2 = tempfile::tempdir().expect("tempdir");
    let cfg2 = CssPipelineConfig {
        sources: vec![PathBuf::from("pages/index.tsx")],
        output_root: tmp2.path().to_path_buf(),
        ..CssPipelineConfig::default()
    };
    let p2 = CssPipeline::new(
        TailwindSubprocessEngine::new(
            TailwindSubprocessConfig::default().with_mock_output(mock),
        ),
        cfg2,
    );
    let emitted = p2.build_emitter().expect("build_emitter");

    assert_eq!(emitted.bytes, built.css.as_bytes());
}

#[test]
fn build_emitter_still_writes_class_map_jsons_when_configured() {
    // Class-map JSONs are *not* asset-graph nodes; the bundler reads
    // them at the next stage. The bytes-only path must keep emitting
    // them so a project using CSS Modules does not break when the
    // prod orchestrator switches to `build_emitter`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let pages = root.join("pages");
    std::fs::create_dir_all(&pages).unwrap();
    let module = pages.join("button.module.css");
    std::fs::write(&module, ".btn { color: red; }\n").unwrap();

    let class_map_dir = root.join("dist").join("css-modules");
    let cfg = CssPipelineConfig {
        sources: vec![],
        css_modules: vec![module.clone()],
        output_root: root.join("dist"),
        base_url: "/".to_string(),
        auto_discover_modules: false,
        class_map_dir: Some(class_map_dir.clone()),
        ..CssPipelineConfig::default()
    };
    let engine = TailwindSubprocessEngine::new(
        TailwindSubprocessConfig::default().with_mock_output(""),
    );
    let pipeline = CssPipeline::new(engine, cfg);
    let _ = pipeline.build_emitter().expect("build_emitter");

    let entries: Vec<_> = std::fs::read_dir(&class_map_dir)
        .expect("class_map_dir exists")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries.iter().any(|n| n.ends_with("button.module.css.classes.json")),
        "expected button class-map json, got entries: {entries:?}"
    );

    // And the hashed asset is still NOT written.
    assert!(!root.join("dist").join("assets").exists());
}
