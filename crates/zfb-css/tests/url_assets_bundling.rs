use std::fs;

use zfb_css::{
    bundle_authored_css_with_assets, scan_css_urls, AuthoredCssBundle, AuthoredCssEngine,
    CssInputDependencyKind, CssPipeline, CssPipelineConfig,
};

fn put(path: &std::path::Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn package(root: &std::path::Path, name: &str, style: &str) -> std::path::PathBuf {
    let dir = root.join("node_modules").join(name);
    put(
        &dir.join("package.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
    );
    put(&dir.join("style.css"), style);
    dir
}

fn bundled(root: &std::path::Path, authored: &str) -> anyhow::Result<AuthoredCssBundle> {
    let entry = root.join("styles/global.css");
    put(&entry, authored);
    bundle_authored_css_with_assets(&entry, root, authored)
}

fn urls(css: &str) -> Vec<String> {
    scan_css_urls(css)
        .into_iter()
        .map(|url| url.decoded)
        .collect()
}

#[test]
fn package_font_from_import_reaches_emitter_as_companion() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let entry = root.join("styles/global.css");
    let package = root.join("node_modules/@acme/fonts");
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::create_dir_all(package.join("files")).unwrap();
    fs::write(&entry, "@import '@acme/fonts/style.css';").unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"@acme/fonts","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(
        package.join("style.css"),
        "@font-face{src:url('./files/a.woff2')}",
    )
    .unwrap();
    fs::write(package.join("files/a.woff2"), b"font bytes").unwrap();

    let bundled =
        bundle_authored_css_with_assets(&entry, root, &fs::read_to_string(&entry).unwrap())
            .unwrap();
    let emitted = CssPipeline::new(
        AuthoredCssEngine::with_bundle(bundled),
        CssPipelineConfig {
            auto_discover_modules: false,
            ..CssPipelineConfig::default()
        },
    )
    .build_emitter()
    .unwrap();
    let css = String::from_utf8(emitted.bytes).unwrap();
    let references = scan_css_urls(&css);
    assert_eq!(
        emitted.companions.len(),
        1,
        "CSS: {css}; references: {references:?}"
    );
    assert_eq!(references.len(), 1);
    assert!(references[0].decoded.starts_with("./a-"));
    assert!(references[0].decoded.ends_with(".woff2"));
    assert_eq!(emitted.companions[0].bytes, b"font bytes");
}

#[test]
fn project_stylesheet_relative_reference_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    put(
        &tmp.path().join("styles/local.css"),
        ".local{background:url('./own.png')}",
    );
    let out = bundled(
        tmp.path(),
        "@import './local.css';.entry{background:url('./entry.png')}",
    )
    .unwrap();
    assert_eq!(urls(&out.css), ["./own.png", "./entry.png"]);
    assert!(out.companions.is_empty());
    assert_eq!(
        out.input_dependencies
            .iter()
            .filter(|d| d.kind == CssInputDependencyKind::Stylesheet)
            .count(),
        2
    );
}

#[test]
fn package_stylesheet_emits_font_and_image() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(
        tmp.path(),
        "images",
        "@font-face{src:url('./a.woff2')}.x{background:url('./b.png')}",
    );
    put(&dir.join("a.woff2"), b"font");
    put(&dir.join("b.png"), b"image");
    let out = bundled(tmp.path(), "@import 'images/style.css';").unwrap();
    let refs = urls(&out.css);
    assert_eq!(refs.len(), 2);
    assert!(refs[0].starts_with("./a-") && refs[0].ends_with(".woff2"));
    assert!(refs[1].starts_with("./b-") && refs[1].ends_with(".png"));
    assert_eq!(out.companions.len(), 2);
    assert_eq!(
        out.input_dependencies
            .iter()
            .filter(|d| d.kind == CssInputDependencyKind::Asset)
            .count(),
        2
    );
}

#[test]
fn nested_package_import_resolves_against_nested_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "nested", "@import './deep/child.css';");
    put(
        &dir.join("deep/child.css"),
        ".x{background:url('./leaf.png')}",
    );
    put(&dir.join("deep/leaf.png"), b"nested leaf");
    let out = bundled(tmp.path(), "@import 'nested/style.css';").unwrap();
    assert_eq!(out.companions.len(), 1);
    assert_eq!(out.companions[0].bytes, b"nested leaf");
    assert!(urls(&out.css)[0].starts_with("./leaf-"));
    assert_eq!(
        out.input_dependencies
            .iter()
            .filter(|d| d.kind == CssInputDependencyKind::Stylesheet)
            .count(),
        3
    );
}

#[test]
fn package_url_import_remains_an_import_not_an_asset() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "urlimport", "@import url('./child.css');");
    put(&dir.join("child.css"), ".child{color:red}");
    let out = bundled(tmp.path(), "@import 'urlimport/style.css';").unwrap();
    assert!(out.css.contains(".child"), "{}", out.css);
    assert!(out.companions.is_empty());
    assert_eq!(
        out.input_dependencies
            .iter()
            .filter(|d| d.kind == CssInputDependencyKind::Stylesheet)
            .count(),
        3
    );
}

#[test]
fn entry_uses_caller_supplied_css_with_entry_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(
        tmp.path(),
        "entry",
        ".disk{background:url('./missing.png')}",
    );
    put(&dir.join("real.png"), b"supplied");
    let entry = dir.join("style.css");
    let out = bundle_authored_css_with_assets(
        &entry,
        tmp.path(),
        ".supplied{background:url('./real.png')}",
    )
    .unwrap();
    assert!(out.css.contains(".supplied"));
    assert!(!out.css.contains(".disk"));
    assert_eq!(out.companions.len(), 1);
    assert_eq!(out.companions[0].bytes, b"supplied");
}

#[test]
fn forbidden_tailwind_import_errors_before_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    for source in [
        "@import 'tailwindcss/utilities';",
        r#"@import 'tailwind\63 ss/utilities';"#,
    ] {
        let err = bundled(tmp.path(), source).unwrap_err().to_string();
        assert!(
            err.contains("ZW009") && err.contains("tailwindcss/utilities"),
            "{err}"
        );
    }
}

#[test]
fn forbidden_import_in_nested_stylesheet_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "badimport", "@import 'tailwindcss';");
    let err = bundled(tmp.path(), "@import 'badimport/style.css';")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("ZW009") && err.contains(&dir.join("style.css").display().to_string()),
        "{err}"
    );
}

#[test]
fn ordinary_layer_import_still_bundles() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "layered", ".layered{color:red}");
    let out = bundled(tmp.path(), "@import 'layered/style.css' layer(components);").unwrap();
    assert!(out.css.contains("@layer components"), "{}", out.css);
    assert!(out.css.contains(".layered"));
    assert!(dir.join("style.css").exists());
}

#[test]
fn external_and_pathless_urls_are_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    package(tmp.path(), "external", ".x{a:url('https://x/a');b:url('//x/a');c:url('/a');d:url('data:image/png;base64,abc');e:url('#id');f:url('?q')}");
    let out = bundled(tmp.path(), "@import 'external/style.css';").unwrap();
    assert_eq!(
        urls(&out.css),
        [
            "https://x/a",
            "//x/a",
            "/a",
            "data:image/png;base64,abc",
            "#id",
            "?q"
        ]
    );
    assert!(out.companions.is_empty());
}

#[test]
fn identical_reference_text_uses_each_declaring_stylesheet() {
    let tmp = tempfile::tempdir().unwrap();
    let first = package(tmp.path(), "first", ".a{background:url('./same.png')}");
    let second = package(tmp.path(), "second", ".b{background:url('./same.png')}");
    put(&first.join("same.png"), b"first");
    put(&second.join("same.png"), b"second");
    let out = bundled(
        tmp.path(),
        "@import 'first/style.css';@import 'second/style.css';",
    )
    .unwrap();
    assert_eq!(out.companions.len(), 2);
    assert_ne!(urls(&out.css)[0], urls(&out.css)[1]);
    assert!(out.companions.iter().any(|c| c.bytes == b"first"));
    assert!(out.companions.iter().any(|c| c.bytes == b"second"));
}

#[test]
fn duplicate_references_share_one_companion() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(
        tmp.path(),
        "duplicate",
        ".a{a:url('./x.png');b:url('./x.png')}",
    );
    put(&dir.join("x.png"), b"one");
    let out = bundled(tmp.path(), "@import 'duplicate/style.css';").unwrap();
    assert_eq!(out.companions.len(), 1);
    assert_eq!(urls(&out.css)[0], urls(&out.css)[1]);
    assert_eq!(
        out.input_dependencies
            .iter()
            .filter(|d| d.kind == CssInputDependencyKind::Asset)
            .count(),
        1
    );
}

#[test]
fn query_and_fragment_suffixes_survive_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "suffix", ".a{a:url('./x.png?v=1#part')}");
    put(&dir.join("x.png"), b"one");
    let out = bundled(tmp.path(), "@import 'suffix/style.css';").unwrap();
    assert!(urls(&out.css)[0].starts_with("./x-"));
    assert!(urls(&out.css)[0].ends_with(".png?v=1#part"));
}

#[test]
fn missing_package_asset_is_a_hard_error() {
    let tmp = tempfile::tempdir().unwrap();
    package(tmp.path(), "missing", ".a{a:url('./missing.png')}");
    let err = bundled(tmp.path(), "@import 'missing/style.css';")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("missing@1.0.0") && err.contains("missing.png"),
        "{err}"
    );
}

#[test]
fn directory_asset_is_a_hard_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "directory", ".a{a:url('./folder')}");
    fs::create_dir_all(dir.join("folder")).unwrap();
    let err = bundled(tmp.path(), "@import 'directory/style.css';")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not a regular file") && err.contains("directory@1.0.0"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_a_hard_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = package(tmp.path(), "escape", ".a{a:url('./outside.png')}");
    let outside = tmp.path().join("outside.png");
    put(&outside, b"outside");
    std::os::unix::fs::symlink(&outside, dir.join("outside.png")).unwrap();
    let err = bundled(tmp.path(), "@import 'escape/style.css';")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("outside the package directory") && err.contains("escape@1.0.0"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn workspace_linked_stylesheet_is_authored() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("packages/linked");
    put(
        &workspace.join("package.json"),
        br#"{"name":"linked","version":"1.0.0"}"#,
    );
    put(&workspace.join("style.css"), ".a{a:url('./local.png')}");
    fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
    std::os::unix::fs::symlink(&workspace, tmp.path().join("node_modules/linked")).unwrap();
    let out = bundled(tmp.path(), "@import 'linked/style.css';").unwrap();
    assert_eq!(urls(&out.css), ["./local.png"]);
    assert!(out.companions.is_empty());
    assert!(out.input_dependencies.iter().any(|d| d.path
        == fs::canonicalize(workspace.join("style.css")).unwrap()
        && d.kind == CssInputDependencyKind::Stylesheet));
}
