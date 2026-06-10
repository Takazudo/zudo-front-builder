//! Regression test for zfb#532 — SSR catch-all survives `worker_only_routes` filter.
//!
//! Before the fix, `RouteEntry::entry_key` was stored in bracket-syntax
//! (`/api/admin/[...adminPath]`) while `worker_only_routes` is populated
//! in Hono-syntax (`/api/admin/:adminPath{.+}`). The string-equality
//! check at the filter site (`filter.contains(&r.entry_key)`) never matched
//! a catch-all, so the SSR catch-all was tree-shaken out of the runtime
//! bundle. After the fix `entry_key` is stored via `bracket_to_hono`, so
//! both sides are Hono-form and the filter matches.
//!
//! This test calls `bundle()` directly with `worker_only_routes` populated
//! and asserts:
//!   1. `RouteEntry::entry_key` on the catch-all entry equals the Hono form.
//!   2. The emitted bundle JS contains the catch-all Hono template string,
//!      proving the route survived the filter and reached esbuild's output.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Regression test: SSR catch-all route survives `worker_only_routes` filter.
///
/// Constructs a minimal project with:
///  - A static SSR route (`/api/hello`).
///  - An SSR catch-all route (`/api/admin/[...adminPath]`).
///
/// Sets `worker_only_routes` to the Hono-form templates for both, then
/// calls `bundle()` and asserts:
///  - `entry_key` on the catch-all is Hono-form (`/api/admin/:adminPath{.+}`).
///  - The emitted bundle contains the catch-all Hono template, confirming
///    the route was NOT tree-shaken.
#[test]
fn ssr_catchall_survives_worker_only_routes_filter() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[worker_only_routes_filter] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();

    // Create pages directory layout:
    //   pages/api/hello.tsx          → route /api/hello
    //   pages/api/admin/[...adminPath].tsx  → route /api/admin/[...adminPath]
    fs::create_dir_all(root.join("pages/api/admin")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    // Minimal page module: the bundler just needs a default export.
    fs::write(
        root.join("pages/api/hello.tsx"),
        r#"
            export default function Hello() {
              return "hello";
            }
        "#,
    )
    .unwrap();

    fs::write(
        root.join("pages/api/admin/[...adminPath].tsx"),
        r#"
            export default function AdminCatchAll({ params }) {
              return "admin path: " + params.adminPath;
            }
        "#,
    )
    .unwrap();

    // `worker_only_routes` is Hono-form — this is what the build command
    // populates from `route.template()` / `d.template` (zfb#532).
    let mut worker_only: BTreeSet<String> = BTreeSet::new();
    worker_only.insert("/api/hello".into());
    worker_only.insert("/api/admin/:adminPath{.+}".into());

    let input = BundlerInput {
        main_fields: Vec::new(),
        project_root: root.clone(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        // Mark every bare specifier the synthetic entry.mjs imports as
        // external so this test doesn't need a node_modules tree.
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
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: Some(worker_only),
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
    };

    let out = bundle(input).expect("bundle should succeed");

    // 1. Assert entry_key on the catch-all is Hono-form.
    //    (manifest.routes contains ALL routes regardless of filter — this
    //    assertion checks that the assignment sites now produce Hono-form.)
    let catchall_entry = out
        .manifest
        .routes
        .iter()
        .find(|r| r.route == "/api/admin/[...adminPath]")
        .expect("catch-all route should be in the manifest");
    assert_eq!(
        catchall_entry.entry_key, "/api/admin/:adminPath{.+}",
        "entry_key must be Hono-form so worker_only_routes filter can match catch-alls"
    );

    // 2. Assert the emitted bundle body contains the catch-all Hono template.
    //    The `write_entry_module` function emits `__zfb_pages` with
    //    `bracket_to_hono(&route.route)` — if the catch-all was filtered OUT
    //    this string would be absent from the bundle body.
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(!body.is_empty(), "bundle body should be non-empty");
    assert!(
        body.contains(":adminPath{.+}"),
        "bundle must contain the catch-all Hono route template `:adminPath{{.+}}` — \
         if absent the catch-all was tree-shaken (filter mismatch bug from zfb#532).\n\
         --- bundle head ---\n{}",
        snippet(&body)
    );
}

/// Truncate a long string for assert messages.
fn snippet(s: &str) -> String {
    if s.len() <= 800 {
        return s.to_string();
    }
    format!("{}…[{} bytes truncated]", &s[..800], s.len() - 800)
}
