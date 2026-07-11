//! Wave 2 regression tests for plugin alias / virtual-module
//! exact-match resolution in the main bundler (#269).
//!
//! The contract being pinned:
//!
//! 1. `addAlias("@/foo", "/abs/foo.tsx")` resolves `@/foo` exactly —
//!    `@/foo/bar` is NOT silently rewritten to `<target>/bar` the way
//!    esbuild's prefix-with-slash `--alias` flag would have done.
//! 2. `addVirtualModule("virtual:foo", ...)` behaves the same way —
//!    `virtual:foo/bar` is unresolved.
//! 3. A project that sets `BundlerInput::tsconfig_paths` AND uses
//!    plugin aliases has BOTH honored — plugin entries are merged on
//!    top, user-supplied entries win on key collision.
//!
//! All three tests need a real esbuild binary: the unit-test path
//! (`mock_subprocess_output`) bypasses `run_esbuild` entirely, so it
//! cannot exercise esbuild's path-mapping pipeline. Esbuild gating
//! follows the precedence used by sibling integration tests
//! (`bundler_strip_md_ext.rs`): `ZFB_ESBUILD_BIN` env var →
//! `crates/zfb/binaries/esbuild/esbuild` slot → `which esbuild` PATH
//! fallback. Skips with a printed note when no binary is present.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{
    bundle, bundle_with_session, BundleMode, BundlerInput, ContentCollectionSpec, ShadowSession,
};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Lay down a minimal user-project tree: one TSX page that imports a
/// single specifier the caller picks. `import_specifier` is what the
/// page's `import` statement asks for; `import_target_path` is the
/// real file the alias resolves to. The two are decoupled so the
/// caller can register `@/foo` and then make the page import either
/// `@/foo` (exact match — should succeed) or `@/foo/bar` (prefix —
/// should fail).
fn write_fixture_project(
    root: &std::path::Path,
    import_specifier: &str,
    aliased_target_filename: &str,
) {
    for d in ["pages", "content", "components", "layouts", "src"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join(format!("src/{aliased_target_filename}")),
        "export default function Foo() { return null; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        format!(
            "import Foo from {spec};\n\
             export default function Page() {{ return <Foo />; }}\n",
            spec = serde_json::to_string(import_specifier).unwrap()
        ),
    )
    .unwrap();
}

fn write_value_page(root: &std::path::Path, import_specifier: &str) {
    fs::write(
        root.join("pages/index.tsx"),
        format!(
            "import value from {spec};\n\
             export default function Page() {{ return <div>{{value}}</div>; }}\n",
            spec = serde_json::to_string(import_specifier).unwrap()
        ),
    )
    .unwrap();
}

fn write_hidden_imports_package(root: &std::path::Path, directory: &str, marker: &str) -> PathBuf {
    let package = root.join(directory);
    fs::create_dir_all(package.join("dist")).unwrap();
    fs::create_dir_all(package.join(".internal")).unwrap();
    fs::write(
        package.join("package.json"),
        r##"{"type":"module","main":"./dist/entry.js","imports":{"#internal":"./.internal/value.js"}}"##,
    )
    .unwrap();
    fs::write(
        package.join("dist/entry.js"),
        "import value from '#internal'; export default value;\n",
    )
    .unwrap();
    fs::write(
        package.join(".internal/value.js"),
        format!("export default {marker:?};\n"),
    )
    .unwrap();
    package
}

fn make_input(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
    tsconfig_paths: BTreeMap<String, Vec<String>>,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
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
        tsconfig_paths,
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
        content_collections: vec![],
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries,
        plugin_virtual_modules,
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
    }
}

/// **Exact-match regression test (alias).**
///
/// Register `@/foo` → `<root>/src/foo.tsx`. The page imports
/// `@/foo/bar` (NOT the registered specifier). esbuild must NOT
/// rewrite this to `<root>/src/foo.tsx/bar` the way `--alias` would
/// have — bundling must fail with an unresolved-import error
/// referencing the bare `@/foo/bar` string.
#[test]
fn plugin_alias_does_not_match_prefix_with_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "@/foo/bar", "foo.tsx");

    let plugin_aliases = vec![(
        "@/foo".to_string(),
        root.join("src/foo.tsx").to_string_lossy().into_owned(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-alias",
        BTreeMap::new(),
        plugin_aliases,
        vec![],
    );
    let err = bundle(input).expect_err(
        "Wave 2 contract: addAlias(\"@/foo\", ...) must NOT match `@/foo/bar`; \
         bundling must fail with an unresolved-import error.",
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("@/foo/bar"),
        "esbuild error should mention the unresolved specifier `@/foo/bar`. \
         Got: {msg}"
    );
}

/// **Exact-match regression test — happy path (alias).**
///
/// Same setup as above, but the page imports `@/foo` (the registered
/// specifier). Bundling must succeed and the bundled output must
/// reference the aliased file's exported identifier `Foo`.
#[test]
fn plugin_alias_matches_exact_specifier() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "@/foo", "foo.tsx");

    let plugin_aliases = vec![(
        "@/foo".to_string(),
        root.join("src/foo.tsx").to_string_lossy().into_owned(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-alias-ok",
        BTreeMap::new(),
        plugin_aliases,
        vec![],
    );
    let out = bundle(input).expect(
        "exact-match alias `@/foo` must resolve when the import string is the \
         registered specifier",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // The aliased target's exported `Foo` function survives into the
    // bundle (modulo esbuild's identifier rename — `function Foo` /
    // `Foo()` / `Foo.displayName` style references are all stable
    // because we don't minify).
    assert!(
        body.contains("Foo"),
        "exact-match alias bundle should contain the aliased component's \
         identifier `Foo`."
    );
}

#[test]
fn project_plugin_alias_is_preprocessed_inside_the_ssr_shadow() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:preprocessed", "unused.tsx");
    let alias = root.join("plugin-preprocessed.ts");
    let worker = root.join("plugin-preprocessed.worker.ts");
    fs::write(
        &alias,
        "import payload from './plugin-preprocessed.txt?raw';\n\
         export default function Foo() {\n\
           new Worker(new URL('./plugin-preprocessed.worker.ts', import.meta.url), { type: 'module' });\n\
           return payload;\n\
         }\n",
    )
    .unwrap();
    fs::write(
        root.join("plugin-preprocessed.txt"),
        "ZFB_SSR_PLUGIN_ALIAS_RAW",
    )
    .unwrap();
    fs::write(&worker, "self.postMessage('worker');\n").unwrap();
    let worker_filename = zfb_types::module_worker_filename(&root, &worker).unwrap();
    let input = make_input(
        &root,
        &esbuild,
        "dist-alias-preprocessed",
        BTreeMap::new(),
        vec![(
            "plugin:preprocessed".to_string(),
            alias.to_string_lossy().into_owned(),
        )],
        vec![],
    );

    let output = bundle(input).expect("SSR plugin alias preprocessing must bundle");
    let body = fs::read_to_string(output.bundle_path).unwrap();
    assert!(body.contains("ZFB_SSR_PLUGIN_ALIAS_RAW"), "{body}");
    assert!(body.contains(&worker_filename), "{body}");
    assert!(!body.contains("?raw"), "{body}");
}

#[test]
fn imported_excluded_plugin_alias_never_falls_back_to_real_source() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:excluded-root", "unused.tsx");
    fs::create_dir_all(root.join("content/blog")).unwrap();
    let alias = root.join("content/blog/excluded-plugin.ts");
    fs::write(
        &alias,
        "export default function ExcludedRealSourceMarker() { return null; }\n",
    )
    .unwrap();
    // Materialise a separate, unexcluded plugin target at the exact project
    // path used by the former fixed sentinel. An excluded alias must not be
    // able to resolve this file through a project-controlled shadow collision.
    let former_sentinel_collision =
        root.join(".zfb-excluded-plugin-alias/content/blog/excluded-plugin.ts");
    fs::create_dir_all(former_sentinel_collision.parent().unwrap()).unwrap();
    fs::write(
        &former_sentinel_collision,
        "export default function FormerFixedSentinelMarker() { return null; }\n",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-alias-root",
        BTreeMap::new(),
        vec![
            (
                "plugin:excluded-root".to_string(),
                alias.to_string_lossy().into_owned(),
            ),
            (
                "plugin:former-sentinel-collision".to_string(),
                former_sentinel_collision.to_string_lossy().into_owned(),
            ),
        ],
        vec![],
    );
    // Exercise both collection-side exclusion and the resolver guard: the
    // collection must not materialise the source, and the plugin alias must
    // still point at the guaranteed-absent sentinel instead of the real file.
    input.content_collections = vec![ContentCollectionSpec::new(
        "blog",
        root.join("content/blog"),
    )];
    input.bundle_exclude = vec!["content/blog/excluded-plugin.ts".to_string()];

    let error = bundle(input).expect_err(
        "an imported plugin alias excluded from the shadow must not resolve from the real tree",
    );
    let message = format!("{error:#}");
    assert!(message.contains("plugin:excluded-root"), "{message}");
    let bundle_path = root.join("dist-excluded-alias-root/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("ExcludedRealSourceMarker"), "{body}");
        assert!(!body.contains("FormerFixedSentinelMarker"), "{body}");
    }
}

#[test]
fn unused_excluded_plugin_alias_registration_is_harmless() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:unused-excluded", "unused.tsx");
    fs::write(
        root.join("pages/index.tsx"),
        "export default function Page() { return null; }\n",
    )
    .unwrap();
    let alias = root.join("excluded-unused-plugin.ts");
    fs::write(
        &alias,
        "import value from './missing.txt?raw'; export default value;\n",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-unused-excluded-alias",
        BTreeMap::new(),
        vec![(
            "plugin:unused-excluded".to_string(),
            alias.to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.bundle_exclude = vec!["excluded-unused-plugin.ts".to_string()];

    bundle(input).expect("an unused excluded alias registration must not fail the bundle");
}

#[test]
fn plugin_alias_preprocessing_honours_excluded_nested_raw_target() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:excluded-raw", "unused.tsx");
    fs::create_dir_all(root.join("content/blog")).unwrap();
    let alias = root.join("content/blog/plugin-excluded-raw.ts");
    fs::write(
        &alias,
        "import payload from './plugin-secret.txt?raw';\n\
         export default function Foo() { return payload; }\n",
    )
    .unwrap();
    fs::write(
        root.join("content/blog/plugin-secret.txt"),
        "EXCLUDED_PLUGIN_RAW_SECRET",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-plugin-raw",
        BTreeMap::new(),
        vec![(
            "plugin:excluded-raw".to_string(),
            alias.to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.content_collections = vec![ContentCollectionSpec::new(
        "blog",
        root.join("content/blog"),
    )];
    input.bundle_exclude = vec!["content/blog/plugin-secret.txt".to_string()];

    let error = bundle(input).expect_err("an excluded required ?raw target must fail by name");
    let message = format!("{error:#}");
    assert!(message.contains("bundle.exclude"), "{message}");
    assert!(message.contains("plugin-secret.txt"), "{message}");
    let bundle_path = root.join("dist-excluded-plugin-raw/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_PLUGIN_RAW_SECRET"), "{body}");
    }
}

#[test]
fn plugin_alias_preprocessing_honours_excluded_nested_module() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:excluded-module", "unused.tsx");
    fs::create_dir_all(root.join("content/blog")).unwrap();
    let alias = root.join("content/blog/plugin-excluded-module.ts");
    fs::write(
        &alias,
        "import marker from './plugin-secret.ts';\n\
         export default function Foo() { return marker; }\n",
    )
    .unwrap();
    fs::write(
        root.join("content/blog/plugin-secret.ts"),
        "export default 'EXCLUDED_PLUGIN_MODULE_SECRET';\n",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-plugin-module",
        BTreeMap::new(),
        vec![(
            "plugin:excluded-module".to_string(),
            alias.to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.content_collections = vec![ContentCollectionSpec::new(
        "blog",
        root.join("content/blog"),
    )];
    input.bundle_exclude = vec!["content/blog/plugin-secret.ts".to_string()];

    let error = bundle(input)
        .expect_err("an excluded nested module must remain absent from the SSR shadow");
    let message = format!("{error:#}");
    assert!(message.contains("plugin-secret"), "{message}");
    let bundle_path = root.join("dist-excluded-plugin-module/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_PLUGIN_MODULE_SECRET"), "{body}");
    }
}

#[test]
fn plugin_alias_preprocessing_honours_excluded_exact_tsconfig_target() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:excluded-tsconfig", "unused.tsx");
    fs::create_dir_all(root.join("content/blog")).unwrap();
    let alias = root.join("content/blog/plugin-excluded-tsconfig.ts");
    let secret = root.join("content/blog/plugin-tsconfig-secret.ts");
    fs::write(
        &alias,
        "import marker from 'project:secret';\n\
         export default function Foo() { return marker; }\n",
    )
    .unwrap();
    fs::write(
        &secret,
        "export default 'EXCLUDED_TSCONFIG_TARGET_SECRET';\n",
    )
    .unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{
          "compilerOptions": {
            "baseUrl": ".",
            "paths": {
              "project:secret": ["content/blog/plugin-tsconfig-secret.ts"]
            }
          }
        }"#,
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-plugin-tsconfig",
        BTreeMap::from([(
            "project:secret".to_string(),
            vec![secret.to_string_lossy().into_owned()],
        )]),
        vec![(
            "plugin:excluded-tsconfig".to_string(),
            alias.to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.content_collections = vec![ContentCollectionSpec::new(
        "blog",
        root.join("content/blog"),
    )];
    input.bundle_exclude = vec!["content/blog/plugin-tsconfig-secret.ts".to_string()];

    let error = bundle(input)
        .expect_err("an excluded exact tsconfig target must not use its live-real fallback");
    let message = format!("{error:#}");
    assert!(message.contains("project:secret"), "{message}");
    let bundle_path = root.join("dist-excluded-plugin-tsconfig/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_TSCONFIG_TARGET_SECRET"), "{body}");
    }
}

#[test]
fn excluded_exact_tsconfig_directory_cannot_fallback_to_index_json() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:excluded-json-directory", "unused.tsx");
    fs::create_dir_all(root.join("content/blog/secret-json-dir")).unwrap();
    let alias = root.join("content/blog/plugin-excluded-json-directory.ts");
    let secret_dir = root.join("content/blog/secret-json-dir");
    fs::write(
        &alias,
        "import payload from 'project:secret-json-directory';\n\
         export default function Foo() { return payload.secret; }\n",
    )
    .unwrap();
    fs::write(
        secret_dir.join("index.json"),
        r#"{"secret":"EXCLUDED_INDEX_JSON_SECRET"}"#,
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-index-json",
        BTreeMap::from([(
            "project:secret-json-directory".to_string(),
            vec![secret_dir.to_string_lossy().into_owned()],
        )]),
        vec![(
            "plugin:excluded-json-directory".to_string(),
            alias.to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.content_collections = vec![ContentCollectionSpec::new(
        "blog",
        root.join("content/blog"),
    )];
    input.bundle_exclude = vec!["content/blog/secret-json-dir/index.json".to_string()];

    let error = bundle(input)
        .expect_err("an excluded index.json must not resolve through an exact directory fallback");
    let message = format!("{error:#}");
    assert!(
        message.contains("project:secret-json-directory"),
        "{message}"
    );
    let bundle_path = root.join("dist-excluded-index-json/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_INDEX_JSON_SECRET"), "{body}");
    }
}

#[test]
fn excluded_nontrailing_css_sibling_is_removed_before_esbuild_selects_directory_main() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:choice", "unused.tsx");
    write_value_page(&root, "project:choice");
    let choice = root.join("src/choice");
    fs::create_dir_all(choice.join("dist")).unwrap();
    fs::write(
        choice.join("package.json"),
        r#"{"main":"./dist/allowed.js"}"#,
    )
    .unwrap();
    fs::write(
        choice.join("dist/allowed.js"),
        "export default 'ALLOWED_DIRECTORY_MAIN';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/choice.mjs"),
        "export default 'ALLOWED_MJS_SIBLING';\n",
    )
    .unwrap();
    fs::write(root.join("src/choice.css"), ".EXCLUDED_CSS_SIBLING {}\n").unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-css-sibling",
        BTreeMap::from([(
            "project:choice".to_string(),
            vec![choice.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec!["src/choice.css".to_string()];

    bundle(input).expect("the isolated view must remove CSS and retain the allowed directory main");
    let body = fs::read_to_string(root.join("dist-excluded-css-sibling/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_DIRECTORY_MAIN"), "{body}");
    assert!(!body.contains("EXCLUDED_CSS_SIBLING"), "{body}");
}

#[test]
fn trailing_tsconfig_target_preserves_directory_only_resolution() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:directory-only", "unused.tsx");
    write_value_page(&root, "project:directory-only");
    let choice = root.join("src/directory-only");
    fs::create_dir_all(choice.join("dist")).unwrap();
    fs::write(choice.join("package.json"), r#"{"main":"./dist/entry.js"}"#).unwrap();
    fs::write(
        choice.join("dist/entry.js"),
        "export default 'TRAILING_DIRECTORY_MAIN';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/directory-only.js"),
        "export default 'WRONG_NONTRAILING_SIBLING';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/directory-only.css"),
        ".EXCLUDED_DIRECTORY_SIBLING {}\n",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-trailing-directory",
        BTreeMap::from([(
            "project:directory-only".to_string(),
            vec![format!("{}/", choice.display())],
        )]),
        vec![],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec!["src/directory-only.css".to_string()];

    bundle(input).expect("trailing target must select the allowed directory main");
    let body = fs::read_to_string(root.join("dist-trailing-directory/bundle.mjs")).unwrap();
    assert!(body.contains("TRAILING_DIRECTORY_MAIN"), "{body}");
    assert!(!body.contains("WRONG_NONTRAILING_SIBLING"), "{body}");
}

#[test]
fn trailing_tsconfig_target_removes_excluded_basename_child_before_main_fallback() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:trailing-child", "unused.tsx");
    write_value_page(&root, "project:trailing-child");
    let target = root.join("src/trailing-child");
    fs::create_dir_all(target.join("dist")).unwrap();
    fs::write(target.join("package.json"), r#"{"main":"./dist/entry.js"}"#).unwrap();
    fs::write(
        target.join("dist/entry.js"),
        "export default 'ALLOWED_TRAILING_MAIN';\n",
    )
    .unwrap();
    fs::write(
        target.join("trailing-child.js"),
        "export default 'EXCLUDED_TRAILING_CHILD';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/trailing-child.js"),
        "export default 'OUTSIDE_TRAILING_DECOY';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-trailing-child-excluded",
        BTreeMap::from([(
            "project:trailing-child".to_string(),
            vec![format!("{}/", target.display())],
        )]),
        vec![],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec!["src/trailing-child/trailing-child.js".to_string()];

    bundle(input).expect("excluded trailing basename child must not leak");
    let body = fs::read_to_string(root.join("dist-trailing-child-excluded/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_TRAILING_MAIN"), "{body}");
    assert!(!body.contains("EXCLUDED_TRAILING_CHILD"), "{body}");
    assert!(!body.contains("OUTSIDE_TRAILING_DECOY"), "{body}");
}

#[test]
fn trailing_tsconfig_target_keeps_allowed_basename_child_when_main_is_excluded() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:trailing-child", "unused.tsx");
    write_value_page(&root, "project:trailing-child");
    let target = root.join("src/trailing-child");
    fs::create_dir_all(target.join("dist")).unwrap();
    fs::write(target.join("package.json"), r#"{"main":"./dist/entry.js"}"#).unwrap();
    fs::write(
        target.join("dist/entry.js"),
        "export default 'EXCLUDED_TRAILING_MAIN';\n",
    )
    .unwrap();
    fs::write(
        target.join("trailing-child.js"),
        "export default 'ALLOWED_TRAILING_CHILD';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-trailing-child-allowed",
        BTreeMap::from([(
            "project:trailing-child".to_string(),
            vec![format!("{}/", target.display())],
        )]),
        vec![],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec!["src/trailing-child/dist/entry.js".to_string()];

    bundle(input).expect("allowed trailing basename child must remain resolvable");
    let body = fs::read_to_string(root.join("dist-trailing-child-allowed/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_TRAILING_CHILD"), "{body}");
    assert!(!body.contains("EXCLUDED_TRAILING_MAIN"), "{body}");
}

#[test]
fn node_modules_exact_target_removes_preferred_excluded_js_and_uses_ts() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:node-choice", "unused.tsx");
    write_value_page(&root, "project:node-choice");
    let package = root.join("node_modules/context-order-probe");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        package.join("value.js"),
        "export default 'EXCLUDED_NODE_JS';\n",
    )
    .unwrap();
    fs::write(
        package.join("value.ts"),
        "export default 'ALLOWED_NODE_TS';\n",
    )
    .unwrap();
    let target = package.join("value");
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-node-js-excluded",
        BTreeMap::from([(
            "project:node-choice".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec!["node_modules/context-order-probe/value.js".to_string()];

    bundle(input).expect("isolated node_modules target must retain allowed TS fallback");
    let body = fs::read_to_string(root.join("dist-node-js-excluded/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_NODE_TS"), "{body}");
    assert!(!body.contains("EXCLUDED_NODE_JS"), "{body}");
}

#[test]
fn node_modules_exact_target_prefers_allowed_js_over_excluded_invalid_ts() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:node-choice", "unused.tsx");
    write_value_page(&root, "project:node-choice");
    let package = root.join("node_modules/context-order-probe");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        package.join("value.js"),
        "export default 'ALLOWED_NODE_JS';\n",
    )
    .unwrap();
    fs::write(
        package.join("value.ts"),
        "export default <; // invalid alternate\n",
    )
    .unwrap();
    let target = package.join("value");
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-node-ts-excluded",
        BTreeMap::from([(
            "project:node-choice".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec!["node_modules/context-order-probe/value.ts".to_string()];

    bundle(input).expect("node_modules JS order must ignore the excluded invalid TS alternate");
    let body = fs::read_to_string(root.join("dist-node-ts-excluded/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_NODE_JS"), "{body}");
}

#[test]
fn node_modules_exact_target_cannot_climb_to_live_excluded_bare_dependency() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:node-bare", "unused.tsx");
    write_value_page(&root, "project:node-bare");
    let package = root.join("node_modules/context-order-probe");
    let dependency = root.join("node_modules/excluded-dependency");
    fs::create_dir_all(&package).unwrap();
    fs::create_dir_all(&dependency).unwrap();
    fs::write(package.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        package.join("value.js"),
        "import value from 'excluded-dependency'; export default value;\n",
    )
    .unwrap();
    fs::write(
        dependency.join("package.json"),
        r#"{"type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("index.js"),
        "export default 'LIVE_EXCLUDED_BARE_DEPENDENCY';\n",
    )
    .unwrap();
    let target = package.join("value");
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-node-bare-excluded",
        BTreeMap::from([(
            "project:node-bare".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec!["node_modules/excluded-dependency/index.js".to_string()];

    let error = bundle(input)
        .expect_err("isolated package must not climb to the live node_modules symlink");
    let message = format!("{error:#}");
    assert!(message.contains("excluded-dependency"), "{message}");
    let bundle_path = root.join("dist-node-bare-excluded/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("LIVE_EXCLUDED_BARE_DEPENDENCY"), "{body}");
    }
}

#[test]
fn node_modules_exact_target_stages_allowed_bare_dependency_closure() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:node-bare", "unused.tsx");
    write_value_page(&root, "project:node-bare");
    let package = root.join("node_modules/context-order-probe");
    let dependency = root.join("node_modules/allowed-dependency");
    fs::create_dir_all(&package).unwrap();
    fs::create_dir_all(&dependency).unwrap();
    fs::write(
        package.join("package.json"),
        r##"{"type":"module","imports":{"#dep":"allowed-dependency"}}"##,
    )
    .unwrap();
    fs::write(
        package.join("value.js"),
        "import value from '#dep'; export default value;\n",
    )
    .unwrap();
    fs::write(
        dependency.join("package.json"),
        r#"{"type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("index.js"),
        "export default 'ALLOWED_ISOLATED_BARE_DEPENDENCY';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/unrelated-secret.ts"),
        "export default 'secret';\n",
    )
    .unwrap();
    let target = package.join("value");
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-node-bare-allowed",
        BTreeMap::from([(
            "project:node-bare".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];

    bundle(input).expect("allowed bare dependency closure must be staged in isolation");
    let body = fs::read_to_string(root.join("dist-node-bare-allowed/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_ISOLATED_BARE_DEPENDENCY"), "{body}");
}

#[cfg(unix)]
#[test]
fn node_modules_symlinked_package_stages_non_hoisted_canonical_dependency_only_without_preserve() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:pnpm-package", "unused.tsx");
    write_value_page(&root, "project:pnpm-package");
    let virtual_store = root.join("node_modules/.pnpm/context@1/node_modules");
    let physical_package = virtual_store.join("context-order-probe");
    let dependency = virtual_store.join("non-hoisted-dependency");
    fs::create_dir_all(&physical_package).unwrap();
    fs::create_dir_all(&dependency).unwrap();
    fs::write(
        physical_package.join("package.json"),
        r#"{"type":"module"}"#,
    )
    .unwrap();
    fs::write(
        physical_package.join("value.js"),
        "import value from 'non-hoisted-dependency'; export default value;\n",
    )
    .unwrap();
    fs::write(
        dependency.join("package.json"),
        r#"{"type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("index.js"),
        "export default 'ALLOWED_PNPM_CANONICAL_DEPENDENCY';\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        &physical_package,
        root.join("node_modules/context-order-probe"),
    )
    .unwrap();
    fs::write(
        root.join("src/unrelated-secret.ts"),
        "export default 'secret';\n",
    )
    .unwrap();
    let target = root.join("node_modules/context-order-probe/value");

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-pnpm-canonical",
        BTreeMap::from([(
            "project:pnpm-package".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];
    bundle(input).expect("non-preserving resolution must stage the canonical pnpm dependency");
    let body = fs::read_to_string(root.join("dist-pnpm-canonical/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_PNPM_CANONICAL_DEPENDENCY"), "{body}");

    let mut preserve_input = make_input(
        &root,
        &esbuild,
        "dist-pnpm-preserve",
        BTreeMap::from([(
            "project:pnpm-package".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    preserve_input.node_modules_dir = Some(root.join("node_modules"));
    preserve_input.node_modules_preserve_symlinks = true;
    preserve_input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];
    let error = bundle(preserve_input)
        .expect_err("preserve-symlinks must retain lexical non-hoisted resolution failure");
    assert!(
        format!("{error:#}").contains("non-hoisted-dependency"),
        "{error:#}"
    );
}

// --- #1555 wave-1: allowed-closure POSITIVES for first-party closure seeding ---
//
// These guard against OVER-suppression: a non-excluded bare dependency reachable
// from a staged FIRST-PARTY package (a `package.json` under the project, outside
// `node_modules`) must resolve from the staged dependency view — across every
// layout (project-local `node_modules`, an EXTERNAL vendored `node_modules_dir`,
// and a pnpm virtual store) and in BOTH `preserve_symlinks` modes. They are
// ADDITIVE prep: the live `<shadow>/node_modules` symlink still backs resolution
// in wave 1, so each stays green whether or not the new explicit staging fires;
// the NEGATIVE tests that prove the staged view is the SOLE resolver land with
// the wave-2 symlink removal. esbuild stays the only resolver here — the Rust
// change only widens the candidate SET staged into the shadow, never predicts a
// winner.

/// Lay down a first-party package `<root>/<dir>` (a `package.json` outside any
/// `node_modules`) whose `entry.js` imports a single bare package specifier.
/// Returns the package directory.
fn write_first_party_bare_importer(root: &std::path::Path, dir: &str, bare_dep: &str) -> PathBuf {
    let package = root.join(dir);
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        package.join("entry.js"),
        format!(
            "import value from {dep}; export default value;\n",
            dep = serde_json::to_string(bare_dep).unwrap()
        ),
    )
    .unwrap();
    package
}

/// Lay down a bare dependency package `<node_modules>/<name>` whose default
/// export is the string `marker`. Returns the (physical) package directory.
fn write_bare_package_in(node_modules: &std::path::Path, name: &str, marker: &str) -> PathBuf {
    let dependency = node_modules.join(name);
    fs::create_dir_all(&dependency).unwrap();
    fs::write(
        dependency.join("package.json"),
        r#"{"type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("index.js"),
        format!("export default {marker:?};\n"),
    )
    .unwrap();
    dependency
}

#[test]
fn first_party_package_stages_project_local_bare_dependency_closure() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    for preserve in [false, true] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        write_fixture_project(&root, "plugin:fp-local", "unused.tsx");
        write_value_page(&root, "plugin:fp-local");
        let package = write_first_party_bare_importer(&root, "first-party-pkg", "local-dependency");
        write_bare_package_in(
            &root.join("node_modules"),
            "local-dependency",
            "ALLOWED_FIRST_PARTY_PROJECT_LOCAL_DEP",
        );
        fs::write(
            root.join("src/unrelated-secret.ts"),
            "export default 'secret';\n",
        )
        .unwrap();
        let mut input = make_input(
            &root,
            &esbuild,
            &format!("dist-fp-local-{preserve}"),
            BTreeMap::new(),
            vec![(
                "plugin:fp-local".to_string(),
                package.join("entry.js").to_string_lossy().into_owned(),
            )],
            vec![],
        );
        input.node_modules_dir = Some(root.join("node_modules"));
        input.node_modules_preserve_symlinks = preserve;
        input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];
        bundle(input)
            .expect("first-party package's project-local bare dependency must stage into the view");
        let body =
            fs::read_to_string(root.join(format!("dist-fp-local-{preserve}/bundle.mjs"))).unwrap();
        assert!(
            body.contains("ALLOWED_FIRST_PARTY_PROJECT_LOCAL_DEP"),
            "preserve_symlinks={preserve}: {body}"
        );
    }
}

#[test]
fn first_party_package_stages_external_vendored_bare_dependency_closure() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    for preserve in [false, true] {
        let project = tempfile::tempdir().expect("project tempdir");
        let vendor = tempfile::tempdir().expect("vendor tempdir");
        let root = project.path().to_path_buf();
        let vendor_node_modules = vendor.path().join("node_modules");
        write_fixture_project(&root, "plugin:fp-vendor", "unused.tsx");
        write_value_page(&root, "plugin:fp-vendor");
        let package =
            write_first_party_bare_importer(&root, "first-party-pkg", "vendored-dependency");
        write_bare_package_in(
            &vendor_node_modules,
            "vendored-dependency",
            "ALLOWED_FIRST_PARTY_EXTERNAL_VENDOR_DEP",
        );
        fs::write(
            root.join("src/unrelated-secret.ts"),
            "export default 'secret';\n",
        )
        .unwrap();
        let mut input = make_input(
            &root,
            &esbuild,
            &format!("dist-fp-vendor-{preserve}"),
            BTreeMap::new(),
            vec![(
                "plugin:fp-vendor".to_string(),
                package.join("entry.js").to_string_lossy().into_owned(),
            )],
            vec![],
        );
        input.node_modules_dir = Some(vendor_node_modules.clone());
        input.node_modules_preserve_symlinks = preserve;
        input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];
        bundle(input).expect(
            "first-party package's external vendored bare dependency must stage into the view",
        );
        let body =
            fs::read_to_string(root.join(format!("dist-fp-vendor-{preserve}/bundle.mjs"))).unwrap();
        assert!(
            body.contains("ALLOWED_FIRST_PARTY_EXTERNAL_VENDOR_DEP"),
            "preserve_symlinks={preserve}: {body}"
        );
    }
}

#[cfg(unix)]
#[test]
fn first_party_package_stages_pnpm_symlinked_bare_dependency_closure() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    for preserve in [false, true] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        write_fixture_project(&root, "plugin:fp-pnpm", "unused.tsx");
        write_value_page(&root, "plugin:fp-pnpm");
        let package = write_first_party_bare_importer(&root, "first-party-pkg", "pnpm-dependency");
        // pnpm layout: the logical `node_modules/pnpm-dependency` is a symlink
        // into the non-hoisted virtual store where the package physically lives.
        let virtual_store = root.join("node_modules/.pnpm/pnpm-dependency@1/node_modules");
        let physical_dependency = write_bare_package_in(
            &virtual_store,
            "pnpm-dependency",
            "ALLOWED_FIRST_PARTY_PNPM_DEP",
        );
        fs::create_dir_all(root.join("node_modules")).unwrap();
        std::os::unix::fs::symlink(
            &physical_dependency,
            root.join("node_modules/pnpm-dependency"),
        )
        .unwrap();
        fs::write(
            root.join("src/unrelated-secret.ts"),
            "export default 'secret';\n",
        )
        .unwrap();
        let mut input = make_input(
            &root,
            &esbuild,
            &format!("dist-fp-pnpm-{preserve}"),
            BTreeMap::new(),
            vec![(
                "plugin:fp-pnpm".to_string(),
                package.join("entry.js").to_string_lossy().into_owned(),
            )],
            vec![],
        );
        input.node_modules_dir = Some(root.join("node_modules"));
        input.node_modules_preserve_symlinks = preserve;
        input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];
        bundle(input).expect(
            "first-party package's pnpm-symlinked bare dependency must stage into the view",
        );
        let body =
            fs::read_to_string(root.join(format!("dist-fp-pnpm-{preserve}/bundle.mjs"))).unwrap();
        assert!(
            body.contains("ALLOWED_FIRST_PARTY_PNPM_DEP"),
            "preserve_symlinks={preserve}: {body}"
        );
    }
}

#[test]
fn first_party_package_imports_external_stages_allowed_bare_dependency_closure() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:fp-imports", "unused.tsx");
    write_value_page(&root, "plugin:fp-imports");
    // A first-party package whose bare dependency is reached only through a
    // package-`imports` `#dep` EXTERNAL branch — the closure source added by
    // `package_external_import_names`.
    let package = root.join("first-party-imports-pkg");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("package.json"),
        r##"{"type":"module","imports":{"#dep":"imports-dependency"}}"##,
    )
    .unwrap();
    fs::write(
        package.join("entry.js"),
        "import value from '#dep'; export default value;\n",
    )
    .unwrap();
    write_bare_package_in(
        &root.join("node_modules"),
        "imports-dependency",
        "ALLOWED_FIRST_PARTY_PACKAGE_IMPORTS_DEP",
    );
    fs::write(
        root.join("src/unrelated-secret.ts"),
        "export default 'secret';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-fp-imports",
        BTreeMap::new(),
        vec![(
            "plugin:fp-imports".to_string(),
            package.join("entry.js").to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];
    bundle(input)
        .expect("first-party package's #dep external imports target must stage into the view");
    let body = fs::read_to_string(root.join("dist-fp-imports/bundle.mjs")).unwrap();
    assert!(
        body.contains("ALLOWED_FIRST_PARTY_PACKAGE_IMPORTS_DEP"),
        "{body}"
    );
}

#[test]
fn css_import_context_does_not_use_allowed_invalid_ts_when_css_is_excluded() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:unused", "unused.tsx");
    fs::write(
        root.join("pages/index.tsx"),
        "import './styles.css'; export default function Page() { return null; }\n",
    )
    .unwrap();
    fs::write(root.join("pages/styles.css"), "@import 'project:theme';\n").unwrap();
    fs::write(
        root.join("src/theme.css"),
        ".EXCLUDED_CONTEXT_CSS { color: red; }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/theme.ts"),
        "export default <; // invalid unused alternate\n",
    )
    .unwrap();
    let target = root.join("src/theme");
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-css-context-excluded",
        BTreeMap::from([(
            "project:theme".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.extra_loader_args = vec!["--loader:.css=css".to_string()];
    input.bundle_exclude = vec!["src/theme.css".to_string()];

    let error = bundle(input).expect_err("CSS context must not fall through to live excluded CSS");
    let message = format!("{error:#}");
    assert!(message.contains("project:theme"), "{message}");
    assert!(!message.contains("failed to parse"), "{message}");
}

#[test]
fn css_import_context_keeps_css_when_invalid_ts_alternate_is_excluded() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:unused", "unused.tsx");
    fs::write(
        root.join("pages/index.tsx"),
        "import './styles.css'; export default function Page() { return null; }\n",
    )
    .unwrap();
    fs::write(root.join("pages/styles.css"), "@import 'project:theme';\n").unwrap();
    fs::write(
        root.join("src/theme.css"),
        ".ALLOWED_CONTEXT_CSS { color: green; }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/theme.ts"),
        "export default <; // excluded invalid alternate\n",
    )
    .unwrap();
    let target = root.join("src/theme");
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-css-context-allowed",
        BTreeMap::from([(
            "project:theme".to_string(),
            vec![target.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.extra_loader_args = vec!["--loader:.css=css".to_string()];
    input.bundle_exclude = vec!["src/theme.ts".to_string()];

    bundle(input).expect("CSS context must resolve the allowed CSS candidate");
    let css = fs::read_to_string(root.join("dist-css-context-allowed/bundle.css")).unwrap();
    assert!(css.contains("ALLOWED_CONTEXT_CSS"), "{css}");
}

#[test]
fn ordinary_wildcard_target_cannot_fallback_to_excluded_real_file() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:secret", "unused.tsx");
    write_value_page(&root, "project:secret");
    fs::write(
        root.join("src/secret.ts"),
        "export default 'EXCLUDED_WILDCARD_SECRET';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-wildcard",
        BTreeMap::from([(
            "project:*".to_string(),
            vec![format!("{}/src/*", root.display())],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec!["src/secret.ts".to_string()];

    let error = bundle(input).expect_err("wildcard fallback must not resurrect excluded source");
    let message = format!("{error:#}");
    assert!(message.contains("project:secret"), "{message}");
}

#[test]
fn unrelated_wildcard_loses_ignored_directory_real_fallback_under_exclusions() {
    // FLIP (#1557): the old "provably disjoint wildcard keeps its real fallback"
    // heuristic is DELETED. THE SWITCH makes exclusion mean exactly "absence from
    // the fully staged shadow tree", uniformly: with ANY `bundle.exclude` active,
    // every under-root wildcard is shadow-only, with no live-real fallback — even
    // one whose prefix is provably disjoint from the excluded pattern. Here the
    // `ignored/` directory is gitignored, so it is never mirrored into the shadow;
    // without the suppressed real fallback the alias target is genuinely absent
    // and the build must FAIL, naming the unresolved import rather than
    // resurrecting the file from the live tree.
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "ignored:allowed", "unused.tsx");
    write_value_page(&root, "ignored:allowed");
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    fs::create_dir_all(root.join("ignored")).unwrap();
    fs::write(
        root.join("ignored/allowed.ts"),
        "export default 'UNRELATED_REAL_FALLBACK_ALLOWED';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/secret.ts"),
        "export default 'UNRELATED_EXCLUDED_FILE';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-unrelated-wildcard",
        BTreeMap::from([(
            "ignored:*".to_string(),
            vec![format!("{}/ignored/*", root.display())],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec!["src/secret.ts".to_string()];

    let error = bundle(input)
        .expect_err("a shadow-only wildcard has no live fallback: the gitignored target is absent");
    let message = format!("{error:#}");
    assert!(message.contains("ignored:allowed"), "{message}");
    let bundle_path = root.join("dist-unrelated-wildcard/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("UNRELATED_REAL_FALLBACK_ALLOWED"), "{body}");
        assert!(!body.contains("UNRELATED_EXCLUDED_FILE"), "{body}");
    }
}

#[test]
fn package_parent_main_ts_rewrite_is_removed_before_allowed_index_fallback() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:parent-main", "unused.tsx");
    write_value_page(&root, "project:parent-main");
    let package = root.join("src/parent-main-package");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"main":"../parent-secret.js"}"#,
    )
    .unwrap();
    fs::write(
        package.join("index.js"),
        "export default 'ALLOWED_PACKAGE_INDEX';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/parent-secret.ts"),
        "export default 'EXCLUDED_PARENT_MAIN_SECRET';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-parent-main",
        BTreeMap::from([(
            "project:parent-main".to_string(),
            vec![package.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec!["src/parent-secret.ts".to_string()];

    bundle(input).expect("excluded parent-main rewrite must be absent from the isolated view");
    let body = fs::read_to_string(root.join("dist-parent-main/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_PACKAGE_INDEX"), "{body}");
    assert!(!body.contains("EXCLUDED_PARENT_MAIN_SECRET"), "{body}");
}

#[test]
fn rooted_package_main_remains_package_local_in_isolated_shadow() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:rooted-main", "unused.tsx");
    write_value_page(&root, "project:rooted-main");
    let package = root.join("src/rooted-main");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), r#"{"main":"/secret.js"}"#).unwrap();
    fs::write(
        package.join("secret.js"),
        "export default 'ALLOWED_PACKAGE_LOCAL_ROOTED_MAIN';\n",
    )
    .unwrap();
    fs::write(
        root.join("secret.js"),
        "export default 'EXCLUDED_PROJECT_ROOT_DECOY';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-rooted-main",
        BTreeMap::from([(
            "project:rooted-main".to_string(),
            vec![package.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec!["secret.js".to_string()];

    bundle(input).expect("rooted main field must stay inside its package");
    let body = fs::read_to_string(root.join("dist-rooted-main/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_PACKAGE_LOCAL_ROOTED_MAIN"), "{body}");
    assert!(!body.contains("EXCLUDED_PROJECT_ROOT_DECOY"), "{body}");
}

#[test]
fn allowed_exact_js_wins_before_excluded_ts_rewrite() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:exact-js", "unused.tsx");
    write_value_page(&root, "project:exact-js");
    let exact_js = root.join("src/exact.js");
    fs::write(&exact_js, "export default 'ALLOWED_EXACT_JS';\n").unwrap();
    fs::write(
        root.join("src/exact.ts"),
        "export default 'EXCLUDED_TS_REWRITE';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-allowed-exact-js",
        BTreeMap::from([(
            "project:exact-js".to_string(),
            vec![exact_js.to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec!["src/exact.ts".to_string()];

    bundle(input).expect("allowed exact .js must not be guarded because .ts is excluded");
    let body = fs::read_to_string(root.join("dist-allowed-exact-js/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_EXACT_JS"), "{body}");
    assert!(!body.contains("EXCLUDED_TS_REWRITE"), "{body}");
}

#[test]
fn hidden_plugin_directory_uses_shadow_entry_and_blocks_excluded_nested_import() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:hidden-directory", "unused.tsx");
    write_value_page(&root, "plugin:hidden-directory");
    let hidden = root.join(".plugin-hidden-directory");
    fs::create_dir_all(hidden.join("dist")).unwrap();
    fs::write(hidden.join("package.json"), r#"{"main":"./dist/entry.js"}"#).unwrap();
    fs::write(
        hidden.join("dist/entry.js"),
        "import secret from '../secret.js';\nexport default secret;\n",
    )
    .unwrap();
    fs::write(
        hidden.join("secret.js"),
        "export default 'EXCLUDED_HIDDEN_PLUGIN_SECRET';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-hidden-plugin-directory",
        BTreeMap::new(),
        vec![(
            "plugin:hidden-directory".to_string(),
            format!("{}/", hidden.display()),
        )],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec![".plugin-hidden-directory/secret.js".to_string()];

    let error = bundle(input)
        .expect_err("hidden directory alias must resolve through its shadow entry closure");
    let message = format!("{error:#}");
    assert!(message.contains("secret.js"), "{message}");
    let bundle_path = root.join("dist-hidden-plugin-directory/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_HIDDEN_PLUGIN_SECRET"), "{body}");
    }
}

#[test]
fn hidden_plugin_directory_preserves_package_imports_into_dotdir() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:hidden-imports", "unused.tsx");
    write_value_page(&root, "plugin:hidden-imports");
    let package = write_hidden_imports_package(
        &root,
        ".plugin-hidden-imports",
        "ALLOWED_HIDDEN_PACKAGE_IMPORT",
    );
    fs::write(
        package.join(".internal/value.js"),
        "import payload from './payload.txt?raw'; export default payload;\n",
    )
    .unwrap();
    fs::write(
        package.join(".internal/payload.txt"),
        "ALLOWED_HIDDEN_PACKAGE_IMPORT_RAW",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-hidden-imports-directory",
        BTreeMap::new(),
        vec![(
            "plugin:hidden-imports".to_string(),
            format!("{}/", package.display()),
        )],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];

    bundle(input).expect("hidden package imports target in a dotdir must be staged");
    let body = fs::read_to_string(root.join("dist-hidden-imports-directory/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_HIDDEN_PACKAGE_IMPORT_RAW"), "{body}");
    assert!(!body.contains("?raw"), "{body}");
}

#[test]
fn hidden_plugin_directory_omits_excluded_package_import_target() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:hidden-imports", "unused.tsx");
    write_value_page(&root, "plugin:hidden-imports");
    let package = write_hidden_imports_package(
        &root,
        ".plugin-hidden-imports",
        "EXCLUDED_HIDDEN_PACKAGE_IMPORT",
    );
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-hidden-imports-excluded",
        BTreeMap::new(),
        vec![(
            "plugin:hidden-imports".to_string(),
            format!("{}/", package.display()),
        )],
        vec![],
    );
    input.main_fields = vec!["main".to_string()];
    input.bundle_exclude = vec![".plugin-hidden-imports/.internal/value.js".to_string()];

    let error = bundle(input).expect_err("excluded package import target must remain absent");
    let message = format!("{error:#}");
    assert!(
        message.contains("#internal") || message.contains("value.js"),
        "{message}"
    );
    let bundle_path = root.join("dist-hidden-imports-excluded/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_HIDDEN_PACKAGE_IMPORT"), "{body}");
    }
}

#[test]
fn hidden_plugin_file_alias_stages_containing_package_for_imports() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:hidden-file-imports", "unused.tsx");
    write_value_page(&root, "plugin:hidden-file-imports");
    let package = write_hidden_imports_package(
        &root,
        ".plugin-hidden-file-imports",
        "ALLOWED_HIDDEN_FILE_PACKAGE_IMPORT",
    );
    let input = make_input(
        &root,
        &esbuild,
        "dist-hidden-imports-file",
        BTreeMap::new(),
        vec![(
            "plugin:hidden-file-imports".to_string(),
            package.join("dist/entry.js").to_string_lossy().into_owned(),
        )],
        vec![],
    );

    bundle(input).expect("direct hidden package file must retain its package imports scope");
    let body = fs::read_to_string(root.join("dist-hidden-imports-file/bundle.mjs")).unwrap();
    assert!(
        body.contains("ALLOWED_HIDDEN_FILE_PACKAGE_IMPORT"),
        "{body}"
    );
}

#[test]
fn project_root_file_alias_stages_nested_root_package_imports_scope() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:root-package", "unused.tsx");
    write_value_page(&root, "plugin:root-package");
    fs::create_dir_all(root.join(".root-internal/nested")).unwrap();
    fs::write(
        root.join("package.json"),
        r##"{"type":"module","imports":{"#root-internal/*":"./.root-internal/*.js"}}"##,
    )
    .unwrap();
    fs::write(
        root.join(".root-entry.js"),
        "import value from '#root-internal/nested/value'; export default value;\n",
    )
    .unwrap();
    fs::write(
        root.join(".root-internal/nested/value.js"),
        "export default 'ALLOWED_ROOT_PACKAGE_IMPORT';\n",
    )
    .unwrap();
    let input = make_input(
        &root,
        &esbuild,
        "dist-root-package-imports",
        BTreeMap::new(),
        vec![(
            "plugin:root-package".to_string(),
            root.join(".root-entry.js").to_string_lossy().into_owned(),
        )],
        vec![],
    );

    bundle(input).expect("root package metadata/import target must share the shadow scope");
    let body = fs::read_to_string(root.join("dist-root-package-imports/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_ROOT_PACKAGE_IMPORT"), "{body}");
}

#[test]
fn hidden_user_exact_file_cannot_import_excluded_relative_dependency() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:hidden-exact", "unused.tsx");
    write_value_page(&root, "project:hidden-exact");
    let hidden = root.join(".hidden-user-exact");
    fs::create_dir_all(&hidden).unwrap();
    fs::write(
        hidden.join("entry.js"),
        "import value from './secret.js'; export default value;\n",
    )
    .unwrap();
    fs::write(
        hidden.join("secret.js"),
        "export default 'EXCLUDED_HIDDEN_USER_SECRET';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-hidden-user-excluded",
        BTreeMap::from([(
            "project:hidden-exact".to_string(),
            vec![hidden.join("entry.js").to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec![".hidden-user-exact/secret.js".to_string()];

    let error = bundle(input).expect_err("exact user target must not use a live relative fallback");
    let message = format!("{error:#}");
    assert!(message.contains("secret.js"), "{message}");
    let bundle_path = root.join("dist-hidden-user-excluded/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("EXCLUDED_HIDDEN_USER_SECRET"), "{body}");
    }
}

#[test]
fn hidden_user_exact_file_stages_allowed_relative_closure_under_unrelated_exclusion() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "project:hidden-exact", "unused.tsx");
    write_value_page(&root, "project:hidden-exact");
    let hidden = root.join(".hidden-user-exact");
    fs::create_dir_all(&hidden).unwrap();
    fs::write(
        hidden.join("entry.js"),
        "import value from './allowed.js'; export default value;\n",
    )
    .unwrap();
    fs::write(
        hidden.join("allowed.js"),
        "export default 'ALLOWED_HIDDEN_USER_CLOSURE';\n",
    )
    .unwrap();
    fs::write(
        root.join("src/unrelated-secret.ts"),
        "export default 'secret';\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-hidden-user-allowed",
        BTreeMap::from([(
            "project:hidden-exact".to_string(),
            vec![hidden.join("entry.js").to_string_lossy().into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.bundle_exclude = vec!["src/unrelated-secret.ts".to_string()];

    bundle(input).expect("isolated user exact target must retain its allowed closure");
    let body = fs::read_to_string(root.join("dist-hidden-user-allowed/bundle.mjs")).unwrap();
    assert!(body.contains("ALLOWED_HIDDEN_USER_CLOSURE"), "{body}");
}

#[test]
fn excluded_mdx_components_override_is_neither_preflighted_nor_imported() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:unused", "unused.tsx");
    fs::write(
        root.join("pages/index.tsx"),
        "export default function Page() { return <main>ok</main>; }\n",
    )
    .unwrap();
    let override_file = root.join("mdx-components.tsx");
    fs::write(
        &override_file,
        "import forbidden from './missing-theme.css?raw';\n\
         export default { marker: 'EXCLUDED_MDX_COMPONENTS_OVERRIDE', forbidden };\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-mdx-components",
        BTreeMap::new(),
        vec![],
        vec![],
    );
    input.mdx_components_file = Some(override_file);
    input.bundle_exclude = vec!["mdx-components.tsx".to_string()];

    bundle(input).expect("excluded mdx-components override must be fully omitted");
    let body = fs::read_to_string(root.join("dist-excluded-mdx-components/bundle.mjs")).unwrap();
    assert!(!body.contains("EXCLUDED_MDX_COMPONENTS_OVERRIDE"), "{body}");
    assert!(!body.contains("missing-theme.css"), "{body}");
}

#[test]
fn excluded_collection_markdown_is_rejected_for_snapshot_parity() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:unused", "unused.tsx");
    fs::create_dir_all(root.join("content/blog")).unwrap();
    fs::write(
        root.join("content/blog/excluded.mdx"),
        "# EXCLUDED_COLLECTION_MARKDOWN\n",
    )
    .unwrap();
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-excluded-collection-markdown",
        BTreeMap::new(),
        vec![],
        vec![],
    );
    input.content_collections = vec![ContentCollectionSpec::new(
        "blog",
        root.join("content/blog"),
    )];
    input.bundle_exclude = vec!["content/blog/excluded.mdx".to_string()];

    let error = bundle(input)
        .expect_err("collection Markdown exclusion must fail instead of desynchronizing snapshot");
    let message = format!("{error:#}");
    assert!(message.contains("snapshot"), "{message}");
    assert!(message.contains("excluded.mdx"), "{message}");
}

#[test]
fn explicit_defines_override_public_env_per_exact_ssr_expression() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:unused", "unused.tsx");
    fs::write(
        root.join("pages/index.tsx"),
        "const processValue = process.env.PUBLIC_COLLISION;\n\
         const importMetaValue = import.meta.env.PUBLIC_COLLISION;\n\
         export default function Page() { return processValue + importMetaValue; }\n",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-public-define-precedence",
        BTreeMap::new(),
        vec![],
        vec![],
    );
    input.public_env_vars = HashMap::from([(
        "PUBLIC_COLLISION".to_string(),
        "PUBLIC_ENV_PAYLOAD_MUST_NOT_WIN".to_string(),
    )]);
    input.define_vars = BTreeMap::from([
        (
            "process.env.PUBLIC_COLLISION".to_string(),
            "\"EXPLICIT_PROCESS_DEFINE\"".to_string(),
        ),
        (
            "import.meta.env.PUBLIC_COLLISION".to_string(),
            "\"EXPLICIT_IMPORT_META_DEFINE\"".to_string(),
        ),
    ]);

    let output = bundle(input).expect("explicit defines and PUBLIC env must bundle");
    let body = fs::read_to_string(output.bundle_path).unwrap();
    assert!(body.contains("EXPLICIT_PROCESS_DEFINE"), "{body}");
    assert!(body.contains("EXPLICIT_IMPORT_META_DEFINE"), "{body}");
    assert!(!body.contains("PUBLIC_ENV_PAYLOAD_MUST_NOT_WIN"), "{body}");
}

#[test]
fn virtual_only_ssr_entry_rejects_unshadowable_preprocessing_syntax() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "virtual:ssr-raw", "unused.tsx");
    let input = make_input(
        &root,
        &esbuild,
        "dist-virtual-preprocess-error",
        BTreeMap::new(),
        vec![],
        vec![(
            "virtual:ssr-raw".to_string(),
            "import value from './payload.txt?raw'; export default value;".to_string(),
        )],
    );

    let error = bundle(input).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains(
            "query-bearing import \"./payload.txt?raw\" inside plugin virtual module \"virtual:ssr-raw\" is unsupported"
        ),
        "{message}"
    );
}

#[test]
fn user_claimed_virtual_skips_losing_plugin_preprocessing_source() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "virtual:claimed", "user-virtual.tsx");
    fs::write(
        root.join("src/user-virtual.tsx"),
        "export default function UserClaimedVirtualMarker() { return null; }\n",
    )
    .unwrap();
    let user_paths = BTreeMap::from([(
        "virtual:claimed".to_string(),
        vec!["src/user-virtual.tsx".to_string()],
    )]);
    let input = make_input(
        &root,
        &esbuild,
        "dist-user-claimed-virtual",
        user_paths,
        vec![],
        vec![(
            "virtual:claimed".to_string(),
            "import type Broken from './payload.txt?raw'; new Worker(new URL('./bad.worker.ts', import.meta.url), { type: 'module' }); export default Broken;".to_string(),
        )],
    );

    let output = bundle(input)
        .expect("user tsconfig mapping must suppress the losing plugin virtual source");
    let body = fs::read_to_string(output.bundle_path).unwrap();
    assert!(body.contains("UserClaimedVirtualMarker"), "{body}");
    assert!(!body.contains("bad.worker"), "{body}");
}

#[test]
fn user_claimed_virtual_stays_suppressed_inside_plugin_alias_root() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:entry", "user-virtual.tsx");
    fs::write(
        root.join("src/user-virtual.tsx"),
        "export default function NestedUserVirtualMarker() { return null; }\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("nested-alias")).unwrap();
    fs::write(
        root.join("nested-alias/tsconfig.json"),
        r#"{"compilerOptions":{}}"#,
    )
    .unwrap();
    let alias_target = root.join("nested-alias/entry.tsx");
    fs::write(
        &alias_target,
        "import Claimed from 'virtual:claimed'; export default Claimed;\n",
    )
    .unwrap();
    let user_paths = BTreeMap::from([(
        "virtual:claimed".to_string(),
        vec!["src/user-virtual.tsx".to_string()],
    )]);
    let input = make_input(
        &root,
        &esbuild,
        "dist-user-claimed-virtual-alias-root",
        user_paths,
        vec![(
            "plugin:entry".to_string(),
            alias_target.to_string_lossy().into_owned(),
        )],
        vec![(
            "virtual:claimed".to_string(),
            "import type Broken from './payload.txt?raw'; export default Broken;".to_string(),
        )],
    );

    let output = bundle(input)
        .expect("authoritative SSR user paths must suppress a losing virtual inside an alias root");
    let body = fs::read_to_string(output.bundle_path).unwrap();
    assert!(body.contains("NestedUserVirtualMarker"), "{body}");
}

/// **Virtual-module exact-match regression test.**
///
/// Register `virtual:foo` with a source string. The page imports
/// `virtual:foo/bar`. Bundling must fail; esbuild must NOT silently
/// rewrite this to `<temp-mjs>/bar`.
#[test]
fn plugin_virtual_module_does_not_match_prefix_with_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    // The page imports `virtual:foo/bar` — wrong by one slash.
    write_fixture_project(&root, "virtual:foo/bar", "foo.tsx");

    let plugin_vms = vec![(
        "virtual:foo".to_string(),
        "export default function Foo() { return null; }\n".to_string(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-vm",
        BTreeMap::new(),
        vec![],
        plugin_vms,
    );
    let err = bundle(input).expect_err(
        "Wave 2 contract: addVirtualModule(\"virtual:foo\", ...) must NOT match \
         `virtual:foo/bar`; bundling must fail with an unresolved-import error.",
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("virtual:foo/bar"),
        "esbuild error should mention the unresolved specifier `virtual:foo/bar`. \
         Got: {msg}"
    );
}

/// **Virtual-module exact-match — happy path.**
///
/// Same registration; the page imports `virtual:foo` (the registered
/// specifier). Bundling must succeed.
#[test]
fn plugin_virtual_module_matches_exact_specifier() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "virtual:foo", "foo.tsx");

    let plugin_vms = vec![(
        "virtual:foo".to_string(),
        "export default function Foo() { return null; }\n".to_string(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-vm-ok",
        BTreeMap::new(),
        vec![],
        plugin_vms,
    );
    let out = bundle(input).expect(
        "exact-match virtual module `virtual:foo` must resolve when the import \
         string is the registered specifier",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("Foo"),
        "exact-match virtual-module bundle should contain the loader's \
         exported identifier `Foo`."
    );
}

/// **Pre-existing `tsconfig_paths` preservation test.**
///
/// A project that sets `BundlerInput::tsconfig_paths` AND uses plugin
/// aliases must have BOTH honored. The user explicitly maps
/// `@/components/*` in their tsconfig; the plugin separately registers
/// `@/data`. Both must resolve. The user's entry must not be erased
/// or shadowed by the plugin merge (collision policy: user wins).
#[test]
fn user_tsconfig_paths_coexist_with_plugin_aliases() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    // Build a project tree with TWO aliased targets — one resolved
    // via the user's tsconfig paths entry, one via a plugin alias.
    for d in [
        "pages",
        "content",
        "components",
        "layouts",
        "src",
        "src/components",
    ] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    // User-tsconfig-mapped target: `@/components/*` → `src/components/*`.
    fs::write(
        root.join("src/components/widget.tsx"),
        "export default function Widget() { return null; }\n",
    )
    .unwrap();
    // Plugin-alias target: `@/data` → `<root>/src/data.ts` (absolute).
    fs::write(root.join("src/data.ts"), "export default { ok: true };\n").unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import Widget from \"@/components/widget\";\n\
         import Data from \"@/data\";\n\
         export default function Page() {\n\
             const _ = [Widget, Data];\n\
             return <Widget />;\n\
         }\n",
    )
    .unwrap();

    let mut user_paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Wildcard entry — user's tsconfig style.
    user_paths.insert(
        "@/components/*".to_string(),
        vec!["src/components/*".to_string()],
    );

    let plugin_aliases = vec![(
        "@/data".to_string(),
        root.join("src/data.ts").to_string_lossy().into_owned(),
    )];

    let input = make_input(
        &root,
        &esbuild,
        "dist-coexist",
        user_paths,
        plugin_aliases,
        vec![],
    );
    let out = bundle(input).expect(
        "User-supplied tsconfig_paths AND plugin aliases must coexist — both \
         resolve in the same bundle.",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // Both targets' identifiers (`Widget`, `Data`) survive into the
    // bundle — esbuild does not minify here.
    assert!(
        body.contains("Widget"),
        "user tsconfig_paths entry (`@/components/*`) must resolve and the \
         target's `Widget` symbol must reach the bundle."
    );
    assert!(
        body.contains("ok: true") || body.contains("ok:true"),
        "plugin alias (`@/data`) must resolve and its source content must \
         reach the bundle."
    );
}

/// **Collision policy test: user wins.**
///
/// User maps `@/x` → `src/user-x.tsx` via `tsconfig_paths`; a plugin
/// also tries to register `@/x` → `<root>/src/plugin-x.tsx`. The
/// merged tsconfig must keep the user's entry. Bundling resolves
/// `@/x` to the user's file, not the plugin's.
#[test]
fn user_tsconfig_paths_win_over_plugin_alias_on_collision() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    for d in ["pages", "content", "components", "layouts", "src"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    // The two candidate targets carry distinct identifier names so
    // we can tell which one esbuild actually pulled into the bundle.
    fs::write(
        root.join("src/user-x.tsx"),
        "export default function UserVersionMarker() { return null; }\n",
    )
    .unwrap();
    fs::write(
        root.join("losing-plugin-x.tsx"),
        "import type Broken from './payload.txt?raw';\n\
         export default function PluginVersionMarker() { return null; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import X from \"@/x\";\n\
         export default function Page() { return <X />; }\n",
    )
    .unwrap();

    let mut user_paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    user_paths.insert(
        "@/x".to_string(),
        // tsconfig paths values are joined against baseUrl (the shadow
        // root, which mirrors `<root>`), so the relative `src/user-x.tsx`
        // here resolves to the user's file under the project tree.
        vec!["src/user-x.tsx".to_string()],
    );
    let plugin_aliases = vec![(
        "@/x".to_string(),
        root.join("losing-plugin-x.tsx")
            .to_string_lossy()
            .into_owned(),
    )];

    let input = make_input(
        &root,
        &esbuild,
        "dist-collide",
        user_paths,
        plugin_aliases,
        vec![],
    );
    let out = bundle(input)
        .expect("bundling must succeed without preprocessing the losing plugin alias target");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("UserVersionMarker"),
        "collision policy: user-supplied tsconfig_paths entry must win — \
         the user's marker symbol `UserVersionMarker` must reach the bundle."
    );
    assert!(
        !body.contains("PluginVersionMarker"),
        "collision policy: plugin entry must NOT shadow the user's entry — \
         the plugin's marker symbol `PluginVersionMarker` should not reach \
         the bundle."
    );
}

// ── THE SWITCH (#1557): F1 + F2 negative regression tests ───────────────────
//
// These land HERE because only THE SWITCH (shadow-only tsconfig targets + no
// live `<shadow>/node_modules` symlink under exclusions) makes them pass. Each
// pins a live-tree fallback that the old 3-state resolver prediction could not
// close: an excluded alias target must be UNREACHABLE, and the build must FAIL
// visibly, naming the unresolved import. The wave-1 allowed-closure positives
// above prove the same switch does NOT over-suppress non-excluded dependencies.

/// Lay down a bare dependency package `<root>/node_modules/<name>` whose default
/// export is `marker`. Returns the physical package directory.
fn write_root_bare_package(root: &std::path::Path, name: &str, marker: &str) -> PathBuf {
    write_bare_package_in(&root.join("node_modules"), name, marker)
}

// Finding 1 — a blocked bare-shaped alias key must not fall back to a
// same-named live bare package.
#[test]
fn excluded_exact_bare_alias_cannot_fall_back_to_same_named_package() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "collision-alias", "unused.tsx");
    write_value_page(&root, "collision-alias");
    fs::write(
        root.join("src/excluded-collision.ts"),
        "export default 'EXCLUDED_EXACT_ALIAS';\n",
    )
    .unwrap();
    write_root_bare_package(&root, "collision-alias", "LIVE_BARE_FALLBACK");

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-exact-bare-collision",
        BTreeMap::from([(
            "collision-alias".to_string(),
            vec![root
                .join("src/excluded-collision.ts")
                .to_string_lossy()
                .into_owned()],
        )]),
        vec![],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec!["src/excluded-collision.ts".to_string()];

    let error = bundle(input)
        .expect_err("an excluded exact alias must not fall back to a same-named live bare package");
    let message = format!("{error:#}");
    assert!(message.contains("collision-alias"), "{message}");
    let bundle_path = root.join("dist-exact-bare-collision/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("LIVE_BARE_FALLBACK"), "{body}");
        assert!(!body.contains("EXCLUDED_EXACT_ALIAS"), "{body}");
    }
}

// Finding 1 — an overlapping wildcard miss must not fall back to a same-named
// live bare package subpath.
#[test]
fn overlapping_wildcard_cannot_fall_back_to_same_named_package_subpath() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "collision-wildcard/secret", "unused.tsx");
    write_value_page(&root, "collision-wildcard/secret");
    fs::create_dir_all(root.join("src/wildcard")).unwrap();
    fs::write(
        root.join("src/wildcard/secret.ts"),
        "export default 'EXCLUDED_WILDCARD_ALIAS';\n",
    )
    .unwrap();
    let package = write_root_bare_package(&root, "collision-wildcard", "unused-root");
    fs::write(
        package.join("secret.js"),
        "export default 'LIVE_WILDCARD_BARE_FALLBACK';\n",
    )
    .unwrap();

    let mut input = make_input(
        &root,
        &esbuild,
        "dist-wildcard-bare-collision",
        BTreeMap::from([(
            "collision-wildcard/*".to_string(),
            vec![format!("{}/src/wildcard/*", root.display())],
        )]),
        vec![],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec!["src/wildcard/secret.ts".to_string()];

    let error = bundle(input)
        .expect_err("an overlapping wildcard miss must resolve to absence, not a bare subpath");
    let message = format!("{error:#}");
    assert!(message.contains("collision-wildcard/secret"), "{message}");
    let bundle_path = root.join("dist-wildcard-bare-collision/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(!body.contains("LIVE_WILDCARD_BARE_FALLBACK"), "{body}");
        assert!(!body.contains("EXCLUDED_WILDCARD_ALIAS"), "{body}");
    }
}

// Finding 2 — a first-party exact target must resolve inside the filtered
// dependency view; it must not climb to an excluded live bare dependency.
#[test]
fn first_party_root_exact_target_cannot_climb_to_excluded_bare_dependency() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:root-bare", "unused.tsx");
    write_value_page(&root, "plugin:root-bare");
    fs::write(
        root.join(".root-bare-entry.js"),
        "import value from 'excluded-root-dependency'; export default value;\n",
    )
    .unwrap();
    let dependency = write_root_bare_package(
        &root,
        "excluded-root-dependency",
        "LIVE_EXCLUDED_ROOT_BARE_DEPENDENCY",
    );
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-root-bare-excluded",
        BTreeMap::new(),
        vec![(
            "plugin:root-bare".to_string(),
            root.join(".root-bare-entry.js")
                .to_string_lossy()
                .into_owned(),
        )],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec![dependency
        .join("index.js")
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned()];

    let error = bundle(input).expect_err(
        "a first-party exact target must not climb to an excluded live bare dependency",
    );
    let message = format!("{error:#}");
    assert!(message.contains("excluded-root-dependency"), "{message}");
    let bundle_path = root.join("dist-root-bare-excluded/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(
            !body.contains("LIVE_EXCLUDED_ROOT_BARE_DEPENDENCY"),
            "{body}"
        );
    }
}

// Finding 2 — a first-party package resolving an external via package `imports`
// must use the filtered dependency view, not climb to the excluded dependency.
#[test]
fn first_party_package_imports_external_cannot_climb_to_excluded_dependency() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "plugin:hidden-external-import", "unused.tsx");
    write_value_page(&root, "plugin:hidden-external-import");
    let package = root.join(".hidden-external-import");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("package.json"),
        r##"{"type":"module","imports":{"#dep":"excluded-imports-dependency"}}"##,
    )
    .unwrap();
    fs::write(
        package.join("entry.js"),
        "import value from '#dep'; export default value;\n",
    )
    .unwrap();
    let dependency = write_root_bare_package(
        &root,
        "excluded-imports-dependency",
        "LIVE_EXCLUDED_PACKAGE_IMPORTS_DEPENDENCY",
    );
    let mut input = make_input(
        &root,
        &esbuild,
        "dist-hidden-external-import-excluded",
        BTreeMap::new(),
        vec![(
            "plugin:hidden-external-import".to_string(),
            package.join("entry.js").to_string_lossy().into_owned(),
        )],
        vec![],
    );
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = vec![dependency
        .join("index.js")
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned()];

    let error = bundle(input)
        .expect_err("package `imports` external targets must use the filtered dependency view");
    let message = format!("{error:#}");
    assert!(
        message.contains("excluded-imports-dependency") || message.contains("#dep"),
        "{message}"
    );
    let bundle_path = root.join("dist-hidden-external-import-excluded/bundle.mjs");
    if bundle_path.exists() {
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(
            !body.contains("LIVE_EXCLUDED_PACKAGE_IMPORTS_DEPENDENCY"),
            "{body}"
        );
    }
}

// Acceptance #6 — one persistent ShadowSession through empty → non-empty →
// empty `bundle.exclude`. The live `<shadow>/node_modules` symlink must be
// actively REMOVED on the empty→non-empty transition (so an excluded dep cannot
// be resurrected), and re-created when the session returns to an empty exclude.
// Behavioural proof: the same bare dependency resolves, then fails, then
// resolves again — across one stable session identity.
#[test]
fn session_transition_removes_and_restores_node_modules_symlink_under_exclusions() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };
    let esbuild = fs::canonicalize(esbuild).expect("absolute esbuild path");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "session-dep", "unused.tsx");
    write_value_page(&root, "session-dep");
    let dependency = write_root_bare_package(&root, "session-dep", "SESSION_DEP_MARKER");

    let make_tick = |exclude: Vec<String>, outdir: &str| {
        let mut input = make_input(&root, &esbuild, outdir, BTreeMap::new(), vec![], vec![]);
        input.mode = BundleMode::Development;
        input.node_modules_dir = Some(root.join("node_modules"));
        input.bundle_exclude = exclude;
        input
    };
    let excluded = vec![dependency
        .join("index.js")
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned()];

    let mut session = ShadowSession::new().unwrap();

    // Tick 1 (empty exclude): the live symlink resolves the bare dep.
    let first = bundle_with_session(make_tick(Vec::new(), "dist-session-a"), Some(&mut session))
        .expect("empty-exclude tick resolves the bare dep through the live symlink");
    let first_body = fs::read_to_string(&first.bundle_path).unwrap();
    assert!(
        first_body.contains("SESSION_DEP_MARKER"),
        "tick 1 must bundle the live bare dependency: {first_body}"
    );

    // Tick 2 (non-empty exclude): the symlink is REMOVED on the transition, so
    // the excluded dep is unreachable and the build fails, naming the import.
    let second = bundle_with_session(make_tick(excluded, "dist-session-b"), Some(&mut session))
        .expect_err("non-empty-exclude tick must remove the live symlink and fail to resolve");
    let second_msg = format!("{second:#}");
    assert!(second_msg.contains("session-dep"), "{second_msg}");

    // Tick 3 (empty exclude again): the symlink is re-created; the dep resolves.
    let third = bundle_with_session(make_tick(Vec::new(), "dist-session-c"), Some(&mut session))
        .expect("returning to an empty exclude must restore the live symlink");
    let third_body = fs::read_to_string(&third.bundle_path).unwrap();
    assert!(
        third_body.contains("SESSION_DEP_MARKER"),
        "tick 3 must resolve the bare dep again after symlink restoration: {third_body}"
    );
}
