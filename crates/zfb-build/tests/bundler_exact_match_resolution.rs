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

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec};
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
