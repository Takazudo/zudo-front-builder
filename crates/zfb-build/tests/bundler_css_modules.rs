//! End-to-end integration test for the build-time CSS Modules JSX
//! rewrite (`zfb-build`'s `BundlerInput::css_module_class_maps`).
//!
//! The bundler treats a `.module.css` import as a JS module exporting
//! the scoped class-name map. This test feeds a page that does
//! `import styles from "./x.module.css"; <div className={styles.foo}>`
//! plus a class map (`{ "foo": "abc12345_foo" }`) and asserts the
//! emitted ESM bundle carries the **scoped** class string, not the
//! original `foo`.
//!
//! ## Esbuild binary discovery
//!
//! Mirrors `bundler_integration.rs`: resolve the binary the same way
//! [`zfb_build::bundler::bundle`] does, with a `which esbuild` PATH
//! fallback, and skip the test (with a printed note) when none is
//! available.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Construct a `BundlerInput` for the project at `root` with the given
/// CSS Modules class maps. Keeps the test focused on the one field
/// under test; everything else is the minimal viable config.
fn make_input(
    root: &std::path::Path,
    esbuild: PathBuf,
    class_maps: HashMap<PathBuf, HashMap<String, String>>,
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
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: Vec::new(),
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
        external_links: None,
        cjk_friendly: true,
        hard_breaks: false,
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: class_maps,
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
    }
}

#[test]
fn css_module_import_resolves_to_scoped_class_name() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_css_modules] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("pages")).unwrap();

    // The `.module.css` file — its raw CSS bytes never reach the
    // bundle; the scoped CSS is shipped separately. The bundler
    // rewrites this file in the shadow tree to a JS class-map module.
    let module_css = root.join("pages/hero.module.css");
    fs::write(&module_css, ".foo { color: red; }\n.bar { color: blue; }\n").unwrap();

    // The page imports the module and reads `styles.foo`.
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import styles from "./hero.module.css";
            export default function Page() {
                return <div className={styles.foo} data-extra={styles.bar}>hi</div>;
            }
        "#,
    )
    .unwrap();

    // The class map the CSS pipeline would produce. Keyed by the
    // ABSOLUTE `.module.css` path, exactly as `zfb-css` emits it.
    let mut names: HashMap<String, String> = HashMap::new();
    names.insert("foo".into(), "abc12345_foo".into());
    names.insert("bar".into(), "abc12345_bar".into());
    let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    class_maps.insert(module_css.clone(), names);

    let out = bundle(make_input(&root, esbuild.clone(), class_maps))
        .expect("bundle with CSS Modules should succeed");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // The scoped class names MUST appear in the bundle — the user's
    // `styles.foo` member access now resolves to the scoped string.
    assert!(
        body.contains("abc12345_foo"),
        "scoped class `abc12345_foo` should appear in the bundle.\n--- bundle ---\n{body}"
    );
    assert!(
        body.contains("abc12345_bar"),
        "scoped class `abc12345_bar` should appear in the bundle.\n--- bundle ---\n{body}"
    );

    // Re-parse with esbuild to prove the bundle is well-formed JS —
    // the `.module.css` → JS rewrite must not produce broken syntax.
    let parse = Command::new(&esbuild)
        .arg(&out.bundle_path)
        .arg("--bundle=false")
        .arg("--log-level=warning")
        .output()
        .expect("re-parse via esbuild");
    assert!(
        parse.status.success(),
        "bundle is not parseable by esbuild: stderr={}",
        String::from_utf8_lossy(&parse.stderr)
    );
}

#[test]
fn unmapped_css_module_degrades_to_empty_object_not_crash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_css_modules] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("pages")).unwrap();

    // A `.module.css` file with NO class-map entry. The bundler must
    // still rewrite it to valid JS (`export default {}`) so the
    // build does not crash on the `--loader:.module.css=js` flag.
    fs::write(
        root.join("pages/orphan.module.css"),
        ".whatever { color: green; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import styles from "./orphan.module.css";
            export default function Page() {
                return <div className={styles.whatever ?? "fallback"}>hi</div>;
            }
        "#,
    )
    .unwrap();

    // Empty class map — the .module.css file is not in it.
    let out = bundle(make_input(&root, esbuild.clone(), HashMap::new()))
        .expect("bundle with unmapped CSS Module should still succeed");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // The raw CSS must NOT have leaked into the JS bundle as a parse error.
    assert!(
        !body.contains("color: green"),
        "raw CSS bytes must not survive in the JS bundle"
    );

    let parse = Command::new(&esbuild)
        .arg(&out.bundle_path)
        .arg("--bundle=false")
        .arg("--log-level=warning")
        .output()
        .expect("re-parse via esbuild");
    assert!(
        parse.status.success(),
        "unmapped-module bundle is not parseable: stderr={}",
        String::from_utf8_lossy(&parse.stderr)
    );
}
