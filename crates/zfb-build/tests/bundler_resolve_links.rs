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
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: Some(ResolveMarkdownLinksSpec {
            routes: vec![ResolveMarkdownLinksRoute {
                docs_dir: PathBuf::from("content/docs"),
                // Route prefix for the docs collection.
                route_prefix: "/docs/".to_string(),
            }],
            on_broken_links,
        }),
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
    }
}

fn make_input_without_resolve(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
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
        pipeline_spec: zfb_content::PipelineSpec::default(),
        // Feature disabled: links pass through unchanged.
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
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
        eprintln!("[bundler_resolve_links] no esbuild binary available; skipping.");
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
        eprintln!("[bundler_resolve_links] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input_with_resolve(&root, &esbuild, "dist-c", OnBrokenLinks::Error);
    let err = bundle(input)
        .expect_err("bundle should fail with onBrokenLinks: error when broken links exist");
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
        eprintln!("[bundler_resolve_links] no esbuild binary available; skipping.");
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

/// Fixture for the directory-relative link repro (zfb#1004):
///
/// - `content/docs/section/index.mdx` — links `[pkg](other-page/)`.
/// - `content/docs/section/other-page.mdx` — the sibling target.
fn write_dir_link_fixture_project(root: &std::path::Path) {
    for d in ["pages", "content/docs/section", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Home\n---\n\nWelcome.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/docs/section/other-page.mdx"),
        "---\ntitle: Other Page\n---\n\nTarget body.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/docs/section/index.mdx"),
        "---\ntitle: Section Index\n---\n\n[pkg](other-page/)\n",
    )
    .unwrap();
}

/// (f) zfb#1004 — a directory-relative link (`other-page/`) from a category
///     index page must resolve to the sibling page's route URL, and must
///     NOT count as a broken link.
///
/// `linkValidation` is armed with `failOnBroken: true` — the configuration
/// whose warning motivated the issue. Pre-fix, the href passed through
/// verbatim, linkValidation classified it as an on-disk `FilePath`, the
/// existence check failed, and the build errored. Post-fix the resolver
/// rewrites it to a site-absolute URL, which linkValidation skips as
/// URL-space — so a successful bundle proves the diagnostic is gone
/// end-to-end.
#[test]
fn resolve_links_dir_relative_link_rewrites_to_route_url() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_resolve_links] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_dir_link_fixture_project(&root);

    let mut input = make_input_with_resolve(&root, &esbuild, "dist-f", OnBrokenLinks::Error);
    input.pipeline_spec = zfb_content::PipelineSpec {
        features: Some(zfb_content::MarkdownFeaturesConfig {
            link_validation: Some(zfb_content::LinkValidationConfig {
                fail_on_broken: Some(true),
            }),
            ..Default::default()
        }),
        build_context_roots: Some((root.clone(), root.join("public"))),
        ..zfb_content::PipelineSpec::default()
    };
    let out = bundle(input).expect("bundle must succeed — dir-relative link is resolvable");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("/docs/section/other-page/"),
        "dir-relative link must be rewritten to /docs/section/other-page/. \
         Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
    assert!(
        !body.contains("\"other-page/\""),
        "the raw dir-relative href must not survive as a verbatim string \
         literal in the bundle. Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
}

/// Fixture for the URL-space fallback repro (zfb#1030):
///
/// - `content/docs/section/article.mdx` — a NON-index page linking
///   `[sibling](../other-article/)` — written against its rendered URL
///   `/docs/section/article/`, one directory deeper than the file.
/// - `content/docs/section/other-article.mdx` — the sibling target.
fn write_url_space_fixture_project(root: &std::path::Path) {
    for d in ["pages", "content/docs/section", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Home\n---\n\nWelcome.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/docs/section/other-article.mdx"),
        "---\ntitle: Other Article\n---\n\nTarget body.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/docs/section/article.mdx"),
        "---\ntitle: Article\n---\n\n[sibling](../other-article/)\n",
    )
    .unwrap();
}

/// (g) zfb#1030 — a URL-space directory link (`../other-article/`) from a
///     NON-index page must resolve to the sibling page's route URL via the
///     URL-space fallback, and must NOT count as a broken link.
///
/// Same `linkValidation` + `failOnBroken` arming as the #1004 test (f):
/// pre-fix, the file-space probe resolves `../other-article/` one
/// directory too high, the href passes through verbatim, linkValidation's
/// on-disk existence check fails, and the build errors — the 164-warning
/// zcss shape from the issue. Post-fix the fallback reads the href
/// against the page's route directory and rewrites it, so a successful
/// bundle proves the diagnostic is gone end-to-end.
#[test]
fn resolve_links_url_space_link_from_non_index_page_rewrites_to_route_url() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_resolve_links] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_url_space_fixture_project(&root);

    let mut input = make_input_with_resolve(&root, &esbuild, "dist-g", OnBrokenLinks::Error);
    input.pipeline_spec = zfb_content::PipelineSpec {
        features: Some(zfb_content::MarkdownFeaturesConfig {
            link_validation: Some(zfb_content::LinkValidationConfig {
                fail_on_broken: Some(true),
            }),
            ..Default::default()
        }),
        build_context_roots: Some((root.clone(), root.join("public"))),
        ..zfb_content::PipelineSpec::default()
    };
    let out = bundle(input)
        .expect("bundle must succeed — URL-space dir link resolves via the #1030 fallback");

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("/docs/section/other-article/"),
        "URL-space dir link must be rewritten to /docs/section/other-article/. \
         Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
    assert!(
        !body.contains("\"../other-article/\""),
        "the raw URL-space href must not survive as a verbatim string \
         literal in the bundle. Bundle excerpt: {}",
        &body[..body.len().min(800)]
    );
}

/// (e) zfb#939 — error mode must STILL fail when every MDX compile is a
///     process-global cache hit.
///
/// Resolve-links pipelines join the MDX compile cache as of zfb#939
/// (pre-#939 they bypassed it). Broken-link diagnostics are drained from
/// the pipeline AFTER each compile — on a hit the plugin never runs, so
/// the cached entry's stored diagnostics must be replayed into the
/// pipeline. Without replay, the first bundle fails but every warm
/// rebuild of the same tree (the dev-loop shape) silently succeeds.
#[test]
fn resolve_links_error_mode_still_fails_when_compiles_hit_the_cache() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_resolve_links] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    // Cold bundle: populates the process-global compile cache, then
    // fails on the broken link (policy applies after the walk).
    let cold = bundle(make_input_with_resolve(
        &root,
        &esbuild,
        "dist-e1",
        OnBrokenLinks::Error,
    ));
    assert!(cold.is_err(), "cold bundle must fail in error mode");

    // Warm bundle: identical tree + config → identical cache keys →
    // every compile is a hit. The failure must survive the hits.
    let warm_err = bundle(make_input_with_resolve(
        &root,
        &esbuild,
        "dist-e2",
        OnBrokenLinks::Error,
    ))
    .expect_err("warm bundle (all compiles cache hits) must still fail in error mode");
    let msg = format!("{warm_err:#}");
    assert!(
        msg.contains("missing.mdx"),
        "replayed diagnostics must still name the broken link URL; got: {msg}"
    );
}
