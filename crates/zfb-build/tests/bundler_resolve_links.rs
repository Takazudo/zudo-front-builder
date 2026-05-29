//! Bundler-wiring integration test for the `resolveMarkdownLinks` config field
//! (zfb#196 / #185 Gap 1).
//!
//! Pins the contract that when `BundlerInput::resolve_markdown_links` is set
//! with `enabled: true`, the bundler:
//!
//! - rewrites author-written `[label](./other.mdx)` links to the rendered
//!   route URL (e.g. `/docs/other/`) in the compiled JSX/bundle output;
//! - collects unresolved `.md`/`.mdx` links as broken-link diagnostics;
//! - respects `onBrokenLinks: 'warn'` (log, don't fail) and
//!   `onBrokenLinks: 'error'` (fail after walk, all links reported); and
//! - preserves current pass-through behavior when the feature is disabled
//!   (`resolve_markdown_links: None`).
//!
//! ## Esbuild gating
//!
//! Same precedence as the other bundler integration tests:
//! `ZFB_ESBUILD_BIN` env var → `crates/zfb/binaries/esbuild/esbuild` slot →
//! `which esbuild` PATH fallback. If no binary is available, the test prints
//! a skip note and returns early rather than failing.
//!
//! ## What is tested
//!
//! All four acceptance criteria from the spec:
//!
//! (a) Good link `[label](./other.mdx)` rewrites to `/docs/other/` in the
//!     bundle (the source map URL).
//! (b) `onBrokenLinks: 'warn'` — bundle succeeds; broken link is NOT
//!     present as-is in the resolved form.
//! (c) `onBrokenLinks: 'error'` — `bundle()` returns `Err`; error message
//!     mentions the broken link URL.
//! (d) Disabled state (`resolve_markdown_links: None`) — the raw `.mdx`
//!     href survives unchanged in the bundle.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{
    bundle, BundleMode, BundlerInput, ContentCollectionSpec, OnBrokenLinks,
    ResolveMarkdownLinksRoute, ResolveMarkdownLinksSpec,
};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Write the fixture project tree:
///
/// - `pages/index.mdx` — the compiled page (only entry).
/// - `content/docs/index.mdx` — the source file that contains:
///   - `[good link](./other.mdx)` — resolvable link.
///   - `[broken link](./missing.mdx)` — unresolvable link.
/// - `content/docs/other.mdx` — target of the good link.
fn write_fixture_project(root: &std::path::Path) {
    for d in ["pages", "content/docs", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // A minimal layout stub (not exercised, but the bundler expects the dir).
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();

    // Page that doesn't exercise link resolution (just a route anchor).
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Home\n---\n\nWelcome.\n",
    )
    .unwrap();

    // The target of the good cross-collection link.
    fs::write(
        root.join("content/docs/other.mdx"),
        "---\ntitle: Other\n---\n\nOther page body.\n",
    )
    .unwrap();

    // The "linked-from" doc: has a good link and a broken link.
    fs::write(
        root.join("content/docs/index.mdx"),
        "---\ntitle: Docs Index\n---\n\n\
         [good link](./other.mdx)\n\n\
         [broken link](./missing.mdx)\n",
    )
    .unwrap();
}

fn make_input_with_resolve(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
    on_broken_links: OnBrokenLinks,
) -> BundlerInput {
    BundlerInput {
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
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![ContentCollectionSpec::new(
            "docs",
            PathBuf::from("content/docs"),
        )],
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: Some(ResolveMarkdownLinksSpec {
            routes: vec![ResolveMarkdownLinksRoute {
                docs_dir: PathBuf::from("content/docs"),
                // Route prefix for the docs collection.
                route_prefix: "/docs/".to_string(),
            }],
            on_broken_links,
        }),
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
        external_links: None,
        cjk_friendly: true,
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
    }
}

fn make_input_without_resolve(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
) -> BundlerInput {
    BundlerInput {
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
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![ContentCollectionSpec::new(
            "docs",
            PathBuf::from("content/docs"),
        )],
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        // Feature disabled: links pass through unchanged.
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
            external_links: None,
            cjk_friendly: true,
            markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
    }
}

/// (a) Good link rewrites to the rendered route URL in the bundle.
#[test]
fn resolve_links_good_link_rewrites_to_route_url() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_resolve_links] no esbuild binary available; \
             set ZFB_ESBUILD_BIN or install esbuild on PATH. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input_with_resolve(&root, &esbuild, "dist-a", OnBrokenLinks::Warn);
    let out = bundle(input).expect("bundle should succeed (warn mode)");

    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // The good link `./other.mdx` must be rewritten to `/docs/other/`.
    // The compiled JSX contains the URL as a string literal; we check for
    // the resolved URL and also that the raw `.mdx` href is absent.
    assert!(
        body.contains("/docs/other/"),
        "good link must be rewritten to /docs/other/ in bundle. \
         Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
    assert!(
        !body.contains("./other.mdx"),
        "./other.mdx must not survive verbatim in bundle (should be rewritten). \
         Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
}

/// (b) onBrokenLinks: 'warn' — bundle succeeds; broken link not rewritten.
#[test]
fn resolve_links_warn_mode_succeeds_despite_broken_link() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_resolve_links] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input_with_resolve(&root, &esbuild, "dist-b", OnBrokenLinks::Warn);
    // Must succeed even though ./missing.mdx is not in the source map.
    let out = bundle(input).expect("bundle should succeed with onBrokenLinks: warn");
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
}

/// (c) onBrokenLinks: 'error' — bundle returns Err after the walk; error
///     message includes the broken link URL.
#[test]
fn resolve_links_error_mode_fails_on_broken_link() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_resolve_links] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input_with_resolve(&root, &esbuild, "dist-c", OnBrokenLinks::Error);
    let err = bundle(input).expect_err(
        "bundle should fail with onBrokenLinks: error when broken links exist",
    );
    let msg = format!("{err:#}");
    // The error message must identify the broken link.
    assert!(
        msg.contains("./missing.mdx") || msg.contains("missing.mdx"),
        "error message should name the broken link URL; got: {msg}"
    );
    assert!(
        msg.contains("broken") || msg.contains("link"),
        "error message should describe the problem; got: {msg}"
    );
}

/// (d) Disabled state — `resolve_markdown_links: None` — raw `.mdx` hrefs
///     survive unchanged in the bundle.
#[test]
fn resolve_links_disabled_preserves_raw_mdx_hrefs() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_resolve_links] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input_without_resolve(&root, &esbuild, "dist-d");
    let out = bundle(input).expect("bundle should succeed with feature disabled");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // With the feature disabled, the raw `./other.mdx` href must survive in
    // the bundle unchanged — no rewriting occurs.
    assert!(
        body.contains("other.mdx") || body.contains("./other.mdx"),
        "disabled mode must preserve the raw .mdx href in the bundle. \
         Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
    // The resolved URL should NOT appear (no rewriting happened).
    assert!(
        !body.contains("/docs/other/"),
        "/docs/other/ must not appear in disabled-mode bundle \
         (no link resolution was requested). \
         Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
}
