use std::fs;
use std::path::{Path, PathBuf};

use zfb_css::{
    bundle_authored_css, bundle_authored_css_with_assets, resolve_css_imports,
    CssInputDependencyKind,
};

fn package(root: &Path, name: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = root.join("node_modules").join(name);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("package.json"), manifest).unwrap();
    for (name, contents) in files {
        let file = root.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, contents).unwrap();
    }
    root
}

fn assert_import(root: &Path, spec: &str, expected: Option<&Path>) {
    let entry = root.join("entry.css");
    let source = format!("@import \"{spec}\";\n");
    fs::write(&entry, &source).unwrap();
    let actual = resolve_css_imports(&entry, root);
    match expected {
        Some(path) => {
            assert_eq!(actual, vec![fs::canonicalize(path).unwrap()]);
            let bundled = bundle_authored_css(&entry, root, &source).unwrap();
            assert!(bundled.contains(".selected"), "{bundled}");
        }
        None => {
            assert!(actual.is_empty(), "unexpected import: {actual:?}");
            assert!(bundle_authored_css(&entry, root, &source).is_err());
        }
    }
}

fn authored_import_error(root: &Path, spec: &str) -> String {
    let entry = root.join("entry.css");
    let source = format!("@import \"{spec}\";\n");
    fs::write(&entry, &source).unwrap();
    bundle_authored_css(&entry, root, &source)
        .unwrap_err()
        .to_string()
}

#[test]
fn exact_unscoped_export_maps_to_css_file() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":"./dist/widget.css"}}"#,
        &[("dist/widget.css", ".selected { color: red; }")],
    );
    assert_import(
        tmp.path(),
        "widget-css/styles.css",
        Some(&pkg.join("dist/widget.css")),
    );
}

#[test]
fn exact_scoped_export_maps_to_css_file() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "@scope/widget-css",
        r#"{"exports":{"./styles.css":"./dist/widget.css"}}"#,
        &[("dist/widget.css", ".selected { color: red; }")],
    );
    assert_import(
        tmp.path(),
        "@scope/widget-css/styles.css",
        Some(&pkg.join("dist/widget.css")),
    );
}

#[test]
fn wildcard_export_maps_to_css_file() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./*":"./dist/*"}}"#,
        &[("dist/widget.css", ".selected { color: red; }")],
    );
    assert_import(
        tmp.path(),
        "widget-css/widget.css",
        Some(&pkg.join("dist/widget.css")),
    );
}

#[test]
fn conditional_export_uses_package_key_order() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":{"default":"./a.css","style":"./b.css"}}}"#,
        &[
            ("a.css", ".selected { color: red; }"),
            ("b.css", ".wrong { color: blue; }"),
        ],
    );
    assert_import(
        tmp.path(),
        "widget-css/styles.css",
        Some(&pkg.join("a.css")),
    );
}

#[test]
fn nested_conditions_and_array_skip_invalid_target() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":{"style":{"default":["../outside.css","./dist/widget.css"]}}}}"#,
        &[("dist/widget.css", ".selected { color: red; }")],
    );
    assert_import(
        tmp.path(),
        "widget-css/styles.css",
        Some(&pkg.join("dist/widget.css")),
    );
}

#[test]
fn nested_style_condition_precedes_default_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./s.css":{"style":{"default":"./a.css"},"default":"./b.css"}}}"#,
        &[
            ("a.css", ".selected { color: red; }"),
            ("b.css", ".wrong { color: blue; }"),
        ],
    );
    assert_import(tmp.path(), "widget-css/s.css", Some(&pkg.join("a.css")));
}

#[test]
fn selected_non_css_target_fails_without_trying_another_condition() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":{"default":"./index.js","style":"./style.css"}}}"#,
        &[
            ("index.js", "export default 1;"),
            ("style.css", ".selected {}"),
        ],
    );
    assert_import(tmp.path(), "widget-css/styles.css", None);
}

#[test]
fn first_valid_array_target_fails_when_it_is_non_css() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":["./index.js","./style.css"]}}"#,
        &[
            ("index.js", "export default 1;"),
            ("style.css", ".selected {}"),
        ],
    );
    let error = authored_import_error(tmp.path(), "widget-css/styles.css");
    assert!(
        error.contains("export \"./styles.css\" of package \"widget-css\" resolved to non-CSS target \"./index.js\""),
        "{error}"
    );
}

#[test]
fn null_and_unlisted_exports_block_physical_files() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./blocked.css":null}}"#,
        &[
            ("blocked.css", ".selected {}"),
            ("unlisted.css", ".selected {}"),
        ],
    );
    assert_import(tmp.path(), "widget-css/blocked.css", None);
    assert_import(tmp.path(), "widget-css/unlisted.css", None);
    let blocked = authored_import_error(tmp.path(), "widget-css/blocked.css");
    assert!(
        blocked.contains("export \"./blocked.css\" of package \"widget-css\" is null"),
        "{blocked}"
    );
    let unlisted = authored_import_error(tmp.path(), "widget-css/unlisted.css");
    assert!(
        unlisted.contains("package \"widget-css\" exports does not expose \"./unlisted.css\""),
        "{unlisted}"
    );
}

#[test]
fn error_names_null_export_reason() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./blocked.css":null}}"#,
        &[("blocked.css", ".selected {}")],
    );
    let entry = tmp.path().join("entry.css");
    let source = "@import \"widget-css/blocked.css\";\n";
    fs::write(&entry, source).unwrap();
    let error = bundle_authored_css_with_assets(&entry, tmp.path(), source)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(
            "export \"./blocked.css\" of package \"widget-css\" is null (blocked by the package)"
        ),
        "{error}"
    );
}

#[test]
fn missing_export_target_names_the_target() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":"./dist/missing.css"}}"#,
        &[],
    );
    let error = authored_import_error(tmp.path(), "widget-css/styles.css");
    assert!(error.contains("./dist/missing.css"), "{error}");
}

#[test]
fn malformed_package_json_has_a_specific_error() {
    let tmp = tempfile::tempdir().unwrap();
    package(tmp.path(), "widget-css", "{", &[]);
    let error = authored_import_error(tmp.path(), "widget-css/styles.css");
    assert!(
        error.contains("package.json of \"widget-css\" is malformed"),
        "{error}"
    );
}

#[test]
fn escaping_export_target_has_a_specific_error() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":"../other/x.css"}}"#,
        &[],
    );
    let error = authored_import_error(tmp.path(), "widget-css/styles.css");
    assert!(
        error.contains("export target escapes the package"),
        "{error}"
    );
}

#[test]
fn package_without_exports_resolves_physical_subpath() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"name":"widget-css"}"#,
        &[("dist/x.css", ".selected { color: red; }")],
    );
    assert_import(
        tmp.path(),
        "widget-css/dist/x.css",
        Some(&pkg.join("dist/x.css")),
    );
}

#[test]
fn missing_physical_subpath_names_the_package_and_path() {
    let tmp = tempfile::tempdir().unwrap();
    package(tmp.path(), "widget-css", r#"{"name":"widget-css"}"#, &[]);
    let error = authored_import_error(tmp.path(), "widget-css/dist/missing.css");
    assert!(
        error.contains("physical path \"./dist/missing.css\" of package \"widget-css\" is missing (package has no exports)"),
        "{error}"
    );
}

#[test]
fn missing_package_has_a_specific_error() {
    let tmp = tempfile::tempdir().unwrap();
    let error = authored_import_error(tmp.path(), "missing-package/styles.css");
    assert!(
        error.contains("package \"missing-package\" is not installed"),
        "{error}"
    );
}

#[test]
fn bare_package_import_keeps_legacy_style_field_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"style":"dist/pkg.css"}"#,
        &[("dist/pkg.css", ".selected { color: red; }")],
    );
    assert_import(tmp.path(), "widget-css", Some(&pkg.join("dist/pkg.css")));
}

#[cfg(unix)]
#[test]
fn pnpm_style_package_symlink_returns_canonical_target() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("packages/widget-css");
    fs::create_dir_all(real.join("dist")).unwrap();
    fs::write(
        real.join("package.json"),
        r#"{"exports":{"./styles.css":"./dist/widget.css"}}"#,
    )
    .unwrap();
    fs::write(real.join("dist/widget.css"), ".selected { color: red; }").unwrap();
    fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
    std::os::unix::fs::symlink(&real, tmp.path().join("node_modules/widget-css")).unwrap();

    assert_import(
        tmp.path(),
        "widget-css/styles.css",
        Some(&real.join("dist/widget.css")),
    );
}

#[test]
fn exported_stylesheet_urls_are_attributed_to_the_real_target_file() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":"./dist/pkg.css"}}"#,
        &[("dist/pkg.css", "@font-face { src: url(./font.woff2); }")],
    );
    let asset = pkg.join("dist/font.woff2");
    fs::write(&asset, b"font bytes").unwrap();
    let entry = tmp.path().join("entry.css");
    let source = "@import \"widget-css/styles.css\";\n";
    fs::write(&entry, source).unwrap();

    let bundle = bundle_authored_css_with_assets(&entry, tmp.path(), source).unwrap();

    let real_asset = fs::canonicalize(asset).unwrap();
    assert!(
        bundle
            .input_dependencies
            .iter()
            .any(
                |dependency| dependency.kind == CssInputDependencyKind::Asset
                    && dependency.path == real_asset
            ),
        "input dependencies: {:?}",
        bundle.input_dependencies
    );
    assert_eq!(bundle.companions.len(), 1);
    assert_eq!(bundle.companions[0].bytes, b"font bytes");
}

#[test]
fn exported_import_keeps_cascade_layer_and_supports_wrappers() {
    let tmp = tempfile::tempdir().unwrap();
    package(
        tmp.path(),
        "widget-css",
        r#"{"exports":{"./styles.css":"./dist/widget.css"}}"#,
        &[("dist/widget.css", ".selected { color: red; }")],
    );
    let entry = tmp.path().join("entry.css");
    let source = "@import \"widget-css/styles.css\" layer(base) supports(display:grid);\n";
    fs::write(&entry, source).unwrap();

    let bundled = bundle_authored_css(&entry, tmp.path(), source).unwrap();

    assert!(bundled.contains(".selected"), "{bundled}");
    assert!(bundled.contains("@layer base"), "{bundled}");
    assert!(bundled.contains("@supports (display: grid)"), "{bundled}");
}

#[test]
fn nearest_package_blocks_ancestor_version() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    package(
        &project,
        "widget-css",
        r#"{"name":"widget-css"}"#,
        &[("other.css", ".wrong {}")],
    );
    package(
        tmp.path(),
        "widget-css",
        r#"{"name":"widget-css"}"#,
        &[("missing.css", ".selected {}")],
    );
    assert_import(&project, "widget-css/missing.css", None);
}
