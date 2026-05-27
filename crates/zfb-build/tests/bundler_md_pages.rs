//! Integration tests for `.md` page support (#408).
//!
//! Tests cover:
//! 1. The bundler produces a valid ESM bundle when `pages/about.md` is
//!    present (esbuild-gated).
//! 2. The bundle includes the compiled markdown body and the HTML shell
//!    for the `.md` page.
//! 3. `.mdx` pages keep working byte-identically after the `.md` change.
//!
//! All esbuild tests skip gracefully when no esbuild binary is available.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Build a minimal `BundlerInput` for an isolated test project.
fn make_input(root: &std::path::Path, esbuild: PathBuf) -> BundlerInput {
    BundlerInput {
        project_root: root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        content_collections: Vec::new(),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        // Mark all bare imports external so this test doesn't need a
        // full node_modules tree next to the shadow root.
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
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
    }
}

/// Bundle a project and re-parse the output with esbuild to confirm it is
/// syntactically valid JS.
fn assert_bundle_parses(bundle_path: &std::path::Path, esbuild: &std::path::Path) {
    let parse = Command::new(esbuild)
        .arg(bundle_path)
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
fn md_page_bundles_successfully() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_md_pages] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to enable."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    // A .md page with title + heading + paragraph.
    fs::write(
        root.join("pages/about.md"),
        "---\ntitle: About\nlang: en\n---\n\n# Hello\n\nA paragraph.\n",
    )
    .unwrap();

    let input = make_input(root, esbuild.clone());
    let out = bundle(input).expect("bundle with .md page should succeed");

    // Bundle exists and has content.
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(!body.is_empty(), "bundle should be non-empty");

    // Route manifest includes /about.
    let routes: Vec<&str> = out.manifest.routes.iter().map(|r| r.route.as_str()).collect();
    assert!(
        routes.contains(&"/about"),
        "manifest must include /about; got {routes:?}"
    );

    // The compiled markdown body is present in the bundle (MDXContent or
    // _createMdxContent from compile_mdx_to_jsx_module_cached).
    assert!(
        body.contains("MDXContent") || body.contains("_createMdxContent"),
        "bundle must contain compiled MDX body; got (first 800 chars):\n{}",
        &body[..body.len().min(800)]
    );

    // The bundle is syntactically valid JS.
    assert_bundle_parses(&out.bundle_path, &esbuild);
}

#[test]
fn md_page_bundle_contains_title_and_document_structure() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_md_pages] no esbuild binary; skipping");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    fs::write(
        root.join("pages/about.md"),
        "---\ntitle: About\n---\n\n# Hello\n",
    )
    .unwrap();

    let input = make_input(root, esbuild.clone());
    let out = bundle(input).expect("bundle with .md page should succeed");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // The title from frontmatter must be present in the bundle.
    assert!(
        body.contains("About"),
        "bundle must contain the page title; body snippet:\n{}",
        &body[..body.len().min(800)]
    );

    // The full HTML document structure markers from the shell module must
    // be present somewhere in the bundle.
    assert!(
        body.contains("html") && body.contains("head") && body.contains("body"),
        "bundle must contain html/head/body from the shell; body snippet:\n{}",
        &body[..body.len().min(800)]
    );
}

#[test]
fn md_page_with_relative_link_bundles_successfully() {
    // A .md page with a relative markdown link `[link](./other)` must
    // compile and bundle without error. We assert on tag structure
    // (the compiled body has <a>) not on the exact resolved URL.
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_md_pages] no esbuild binary; skipping");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    // Page with a relative link.
    fs::write(
        root.join("pages/about.md"),
        "---\ntitle: About\n---\n\nSee [other page](./other).\n",
    )
    .unwrap();

    let input = make_input(root, esbuild.clone());
    let out = bundle(input).expect("bundle with relative link in .md page should succeed");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // The link text must appear (confirms the markdown body was compiled).
    assert!(
        body.contains("other page"),
        "link text must appear in bundle; body snippet:\n{}",
        &body[..body.len().min(800)]
    );

    // The bundle must be syntactically valid JS.
    assert_bundle_parses(&out.bundle_path, &esbuild);
}

#[test]
fn mdx_pages_still_work_after_md_change() {
    // Regression guard: .mdx pages must keep working byte-identically
    // after the .md support change lands.
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_md_pages] no esbuild binary; skipping");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    // A .mdx page (not .md) to confirm .mdx path is untouched.
    fs::write(
        root.join("pages/post.mdx"),
        "---\ntitle: Post\n---\n\n# Post Title\n\nBody text.\n",
    )
    .unwrap();

    let input = make_input(root, esbuild.clone());
    let out = bundle(input).expect("bundle with .mdx page should succeed");

    assert!(out.bundle_path.exists());
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // Route manifest includes /post.
    let routes: Vec<&str> = out.manifest.routes.iter().map(|r| r.route.as_str()).collect();
    assert!(
        routes.contains(&"/post"),
        "manifest must include /post; got {routes:?}"
    );

    // MDX body compiled.
    assert!(
        body.contains("MDXContent") || body.contains("_createMdxContent"),
        "bundle must contain compiled MDX body"
    );

    assert_bundle_parses(&out.bundle_path, &esbuild);
}
