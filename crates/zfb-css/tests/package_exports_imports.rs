use std::fs;
use std::path::{Path, PathBuf};

use zfb_css::{bundle_authored_css, resolve_css_imports};

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
