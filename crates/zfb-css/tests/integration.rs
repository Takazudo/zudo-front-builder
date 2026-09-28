//! Integration tests for the CSS pipeline.

use std::path::{Path, PathBuf};

use zfb_css::{
    link_href, scan_css_module_imports, AuthoredCssEngine, CssDiagnostic, CssDiagnosticOrigin,
    CssDiagnosticSeverity, CssEngineId, CssEngineOutput, CssInputDependency,
    CssInputDependencyKind, CssModulesOutput, CssModulesProcessor, CssPipeline, CssPipelineConfig,
    StubCssEngine,
};

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

    let engine = StubCssEngine::new(".u-text-red { color: red; }\n");
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
        let engine = StubCssEngine::new(output);
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
// Cross-package CSS Modules and structured engine-output fixture.

#[test]
fn acceptance_css_modules_with_framework_package_sources() {
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
    std::fs::write(&user_global_css_file, ":root { --color-brand: #123456; }\n").unwrap();

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
    std::fs::write(&fw_module, ".shell { padding: 1rem; }\n").unwrap();

    // The stub output simulates the real binary scanning
    // both @source globs and emitting utilities for both pages —
    // including the framework-only `prose-zfb-only` class.
    let mock_utility_css =
        ".flex{display:flex}.items-center{align-items:center}.prose-zfb-only{max-width:65ch}\n";

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
    let pipeline = CssPipeline::new(StubCssEngine::new(mock_utility_css), cfg);
    let out = pipeline.build().expect("acceptance pipeline build");

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
    // stylesheet.
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
        assert!(
            json_path.exists(),
            "json {} must exist",
            json_path.display()
        );
        let body = std::fs::read_to_string(json_path).unwrap();
        let parsed: std::collections::BTreeMap<String, String> = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("invalid JSON for {}: {e}\n{body}", module_path.display()));
        // Each map must contain at least one binding.
        assert!(!parsed.is_empty());
    }

    // Sanity: the asset file is on disk and link_href works.
    assert!(out.asset_path.exists());
    let href = link_href("/", &out.asset_path);
    assert_eq!(href, format!("/assets/styles-{}.css", out.hash));
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
        scan.modules.iter().any(|p| p.ends_with("local.module.css")),
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
fn build_emitter_threads_engine_metadata() {
    let mut result = CssEngineOutput::new(".x{}", CssEngineId::new("canned", Some("1".into())));
    result.input_dependencies.push(CssInputDependency {
        path: "styles/base.css".into(),
        kind: CssInputDependencyKind::Stylesheet,
    });
    result.diagnostics.push(CssDiagnostic {
        severity: CssDiagnosticSeverity::Warning,
        code: "W001".into(),
        message: "example".into(),
        origin: CssDiagnosticOrigin::default(),
        candidate: None,
    });
    let pipeline = CssPipeline::new(
        StubCssEngine::with_output(result),
        CssPipelineConfig::default(),
    );
    let emitted = pipeline.build_emitter().unwrap();
    assert!(String::from_utf8(emitted.bytes).unwrap().contains(".x{}"));
    assert_eq!(
        emitted.input_dependencies[0].path,
        PathBuf::from("styles/base.css")
    );
    assert_eq!(emitted.diagnostics[0].code, "W001");
    assert_eq!(emitted.engine.name, "canned");
    assert_eq!(emitted.engine.version.as_deref(), Some("1"));
}

#[test]
fn build_emitter_returns_bytes_and_stable_url_without_writing_asset() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output_root = tmp.path().to_path_buf();

    let engine = StubCssEngine::new(".u-text-red { color: red; }\n");
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
    let p1 = CssPipeline::new(StubCssEngine::new(mock), cfg1);
    let built = p1.build().expect("build");

    let tmp2 = tempfile::tempdir().expect("tempdir");
    let cfg2 = CssPipelineConfig {
        sources: vec![PathBuf::from("pages/index.tsx")],
        output_root: tmp2.path().to_path_buf(),
        ..CssPipelineConfig::default()
    };
    let p2 = CssPipeline::new(StubCssEngine::new(mock), cfg2);
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
    let engine = StubCssEngine::new("");
    let pipeline = CssPipeline::new(engine, cfg);
    let _ = pipeline.build_emitter().expect("build_emitter");

    let entries: Vec<_> = std::fs::read_dir(&class_map_dir)
        .expect("class_map_dir exists")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries
            .iter()
            .any(|n| n.ends_with("button.module.css.classes.json")),
        "expected button class-map json, got entries: {entries:?}"
    );

    // And the hashed asset is still NOT written.
    assert!(!root.join("dist").join("assets").exists());
}

// --- Issue #1280: external @import hoisting through the full pipeline -------
//
// The bug has two independent manifestations, one per engine. Both produce a
// combined stylesheet where a consumer's font @import trails the style rules
// (spec-invalid → silently dropped by browsers). The pipeline must hoist it
// above the first style rule regardless of which engine produced the bytes.

fn first_rule_offset(css: &str) -> usize {
    css.find(".x").expect("a style rule must be present")
}

fn import_offset(css: &str) -> usize {
    css.find("@import").expect("an @import must be present")
}

#[test]
fn acceptance_hoists_font_import_authored_engine_half() {
    // Authored half (wind = false): global.css is passed verbatim
    // with no subprocess to inline anything, so a font @import sitting below
    // other rules reaches combine() in its authored position — a second,
    // independent manifestation of the same bug.
    let authored = ".x { color: red }\n.y { color: blue }\n\
                    @import url(\"https://fonts.googleapis.com/css2?family=Noto+Sans+JP\");\n";
    let engine = AuthoredCssEngine::new(authored);
    let cfg = CssPipelineConfig {
        // No sources scan needed for the authored engine.
        auto_discover_modules: false,
        ..CssPipelineConfig::default()
    };
    let combined = CssPipeline::new(engine, cfg)
        .build_emitter()
        .expect("build_emitter");
    let css = String::from_utf8(combined.bytes).expect("utf8");
    assert!(
        import_offset(&css) < first_rule_offset(&css),
        "Authored-half: font @import must be hoisted above the first style rule:\n{css}"
    );
}
