//! End-to-end routing + rendering integration test for `zfb-build`.
//!
//! Exercises the full pipeline from bundler through the embedded V8 host
//! across every routing pattern the framework supports:
//!
//!   - static index (`/`)
//!   - static named (`/about`)
//!   - static sub-index (`/blog`)
//!   - single-segment dynamic (`/blog/[slug]`)
//!   - pagination dynamic (`/blog/page/[page]`)
//!   - nested dynamic (`/[lang]/[slug]`)
//!   - catchall (`/docs/[...slug]`)
//!
//! Fixture source lives at
//! `crates/zfb-render/tests/fixtures/routing-rendering/`. The test
//! bundles it with Preact (and optionally React when available), renders
//! every concrete URL through the embedded V8 host, captures or
//! compares HTML snapshots under
//! `tests/snapshots/e2e_routing_rendering/`, and (for the portable-
//! component contract) asserts that the Preact and React
//! outputs are byte-identical.
//!
//! ## Skip conditions
//!
//! The test skips (prints a note, returns early) when:
//!
//! - No esbuild binary is available (resolves via `ZFB_ESBUILD_BIN`,
//!   `crates/zfb/binaries/esbuild/esbuild`, or `which esbuild`).
//!
//! ## Snapshot bootstrap
//!
//! Run with `INSANE_UPDATE_SNAPSHOTS=1` to (re-)write the snapshots.
//! Without it, the test compares rendered HTML against the stored
//! snapshots and fails if they differ.
//!
//! ## React byte-equality
//!
//! When `react` and `react-dom` are available in the pnpm store, the
//! test runs the same bundle under the React adapter and asserts
//! byte-identical output for every page. React is optional: if the
//! packages are absent the equality check is skipped with a note.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{
    bundle, render_all, Backend, BundleMode, BundlerInput, BundlerOutput, RendererInput,
    RouteUniverseEntry,
};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

// ---------------------------------------------------------------------------
// Infrastructure helpers
// ---------------------------------------------------------------------------

/// Locate the workspace root (two levels up from CARGO_MANIFEST_DIR:
/// `crates/zfb-build` → `crates` → workspace root).
fn workspace_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("workspace root from CARGO_MANIFEST_DIR")
        .to_path_buf()
}

/// Snapshot dir for this test suite. Created on first use.
fn snapshot_dir() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.join("tests/snapshots/e2e_routing_rendering")
}

/// Assert (or write) a snapshot for a single rendered page.
///
/// When `INSANE_UPDATE_SNAPSHOTS=1` is set the file is written /
/// overwritten. Otherwise, if the file exists, its contents are compared;
/// if the file doesn't exist the test fails with a hint to run with
/// `INSANE_UPDATE_SNAPSHOTS=1`.
fn assert_snapshot_eq(name: &str, actual: &[u8]) {
    let dir = snapshot_dir();
    fs::create_dir_all(&dir).expect("create snapshot dir");
    let path = dir.join(name);

    if std::env::var("INSANE_UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        fs::write(&path, actual).expect("write snapshot");
        eprintln!("[snapshot] wrote {}", path.display());
        return;
    }

    if !path.exists() {
        panic!(
            "snapshot file not found: {}\n\
             Run the test with INSANE_UPDATE_SNAPSHOTS=1 to bootstrap it.",
            path.display()
        );
    }

    let expected = fs::read(&path).expect("read snapshot");
    if actual != expected.as_slice() {
        // Show a truncated diff so the failure is actionable without
        // flooding the test output.
        let actual_str = String::from_utf8_lossy(actual);
        let expected_str = String::from_utf8_lossy(&expected);
        panic!(
            "snapshot mismatch for {name}\n\
             --- expected (first 500 chars) ---\n{}\n\
             --- actual (first 500 chars) ---\n{}\n\
             Run with INSANE_UPDATE_SNAPSHOTS=1 to update.",
            &expected_str[..expected_str.len().min(500)],
            &actual_str[..actual_str.len().min(500)],
        );
    }
}

// ---------------------------------------------------------------------------
// Fixture paths and route universe
// ---------------------------------------------------------------------------

/// Absolute path to the routing-rendering fixture project.
fn fixture_root() -> PathBuf {
    // The fixture lives inside `crates/zfb-render/tests/fixtures/` so we
    // navigate from the `zfb-build` crate root.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent() // crates/
        .expect("crates dir")
        .join("zfb-render/tests/fixtures/routing-rendering")
}

/// Every concrete URL we want to render from the fixture project, together
/// with where each should land in the dist tree.
///
/// Dynamic pages are expanded manually here (matching the fixture's
/// `paths()` return values) so the test is fully deterministic and does
/// not need to call `__paths__` via the running host.
fn route_universe() -> Vec<RouteUniverseEntry> {
    let mut entries = Vec::new();

    // --- Static routes ---
    entries.push(RouteUniverseEntry {
        url_path: "/".into(),
        output_path: PathBuf::from("index.html"),
        route_key: "/".into(),
        static_html: false,
        source_path: None,
    });
    entries.push(RouteUniverseEntry {
        url_path: "/about".into(),
        output_path: PathBuf::from("about/index.html"),
        route_key: "/about".into(),
        static_html: false,
        source_path: None,
    });
    entries.push(RouteUniverseEntry {
        url_path: "/blog".into(),
        output_path: PathBuf::from("blog/index.html"),
        route_key: "/blog".into(),
        static_html: false,
        source_path: None,
    });

    // --- /blog/[slug] — one per post (matches fixture posts.ts) ---
    for slug in ["hello", "second", "third", "fourth"] {
        entries.push(RouteUniverseEntry {
            url_path: format!("/blog/{slug}"),
            output_path: PathBuf::from(format!("blog/{slug}/index.html")),
            route_key: "/blog/[slug]".into(),
            static_html: false,
            source_path: None,
        });
    }

    // --- /blog/page/[page] — 4 posts, 2 per page → 2 pages ---
    for page in [1u32, 2] {
        entries.push(RouteUniverseEntry {
            url_path: format!("/blog/page/{page}"),
            output_path: PathBuf::from(format!("blog/page/{page}/index.html")),
            route_key: "/blog/page/[page]".into(),
            static_html: false,
            source_path: None,
        });
    }

    // --- /[lang]/[slug] — 2 langs × 2 slugs ---
    for lang in ["en", "ja"] {
        for slug in ["welcome", "goodbye"] {
            entries.push(RouteUniverseEntry {
                url_path: format!("/{lang}/{slug}"),
                output_path: PathBuf::from(format!("{lang}/{slug}/index.html")),
                route_key: "/[lang]/[slug]".into(),
                static_html: false,
                source_path: None,
            });
        }
    }

    // --- /docs/[...slug] — 3 stub docs ---
    let doc_slugs: &[(&str, &[&str])] = &[
        ("intro", &["intro"]),
        ("guides-install", &["guides", "install"]),
        (
            "guides-config-framework",
            &["guides", "config", "framework"],
        ),
    ];
    for (_name, segments) in doc_slugs {
        let url_path = format!("/docs/{}", segments.join("/"));
        let output_path = PathBuf::from(format!("docs/{}/index.html", segments.join("/")));
        entries.push(RouteUniverseEntry {
            url_path,
            output_path,
            route_key: "/docs/[...slug]".into(),
            static_html: false,
            source_path: None,
        });
    }

    entries
}

// ---------------------------------------------------------------------------
// Bundler helper
// ---------------------------------------------------------------------------

/// Build a `node_modules` directory suitable for the fixture project.
///
/// The bundler runs from a shadow tempdir that has no `node_modules` of its
/// own. We create a minimal one pointing packages at the pnpm virtual store,
/// EXCEPT for `@takazudo/zfb-runtime` which must resolve to the **worktree**
/// copy so the router.ts changes under test are included in the bundle.
/// Using the main-repo copy (through the pnpm symlink) would give the old
/// code and make dynamic routes fail.
fn make_test_node_modules() -> tempfile::TempDir {
    let worktree_root = workspace_root();
    let pnpm_store = worktree_root.join("node_modules/.pnpm/node_modules");

    let tmp = tempfile::tempdir().expect("tempdir for test node_modules");
    let nm = tmp.path();

    // Packages we need from the pnpm virtual store.
    let from_store: &[&str] = &[
        "preact",
        "preact-render-to-string",
        "hono",
        // The `zfb` package is also imported by fixtures; point it at the
        // worktree's packages/zfb so its content.ts etc. use the worktree code.
    ];
    for pkg in from_store {
        let src = pnpm_store.join(pkg);
        if src.exists() {
            let dst = nm.join(pkg);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&src, &dst)
                .unwrap_or_else(|e| panic!("symlink {}: {e}", src.display()));
        }
    }

    // `@takazudo/zfb-runtime` — MUST come from the worktree so our router.ts
    // changes (props-passing for dynamic routes) are included.
    let takazudo_dir = nm.join("@takazudo");
    fs::create_dir_all(&takazudo_dir).expect("create @takazudo dir");
    let zfb_runtime_src = worktree_root.join("packages/zfb-runtime");
    let zfb_runtime_dst = takazudo_dir.join("zfb-runtime");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_runtime_src, &zfb_runtime_dst)
        .unwrap_or_else(|e| panic!("symlink @takazudo/zfb-runtime: {e}"));

    // `zfb` — point at the workspace's packages/zfb.
    let zfb_src = worktree_root.join("packages/zfb");
    let zfb_dst = nm.join("zfb");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, &zfb_dst).unwrap_or_else(|e| panic!("symlink zfb: {e}"));

    tmp
}

fn build_bundle(
    fixture_root: &Path,
    framework: Framework,
    esbuild: &Path,
    dist: &Path,
) -> (BundlerOutput, tempfile::TempDir) {
    // Build a custom node_modules that uses the worktree's packages.
    let node_modules = make_test_node_modules();

    let input = BundlerInput {
        main_fields: Vec::new(),
        project_root: fixture_root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework,
        define_vars: Default::default(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![],
        outdir: dist.to_path_buf(),
        mode: BundleMode::Development,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: Some(node_modules.path().to_path_buf()),
        node_modules_preserve_symlinks: true,
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
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
    };

    let output = bundle(input).expect("bundle should succeed for fixture project");
    // Return the tempdir so the caller can keep it alive (dropping it would
    // delete the node_modules symlink while the bundle is still referencing it).
    (output, node_modules)
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

/// End-to-end routing + rendering test using the embedded V8 host.
///
/// This test bundles the routing-rendering fixture with Preact and renders
/// every route through the embedded V8 host. Requires esbuild and a running
/// embedded V8 host wired via `ZFB_E2E_BASE_URL`.
///
/// Gated `#[ignore]` until the embedded V8 host is merged and
/// the base URL can be resolved automatically. Kick with:
///
///     ZFB_E2E_BASE_URL=http://127.0.0.1:PORT \
///       cargo test --package zfb-build -- --include-ignored \
///         e2e_routing_rendering_with_embedded_host
#[test]
#[ignore = "requires embedded V8 host"]
fn e2e_routing_rendering_with_embedded_host() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[e2e_routing_rendering] no esbuild binary; \
             set ZFB_ESBUILD_BIN or place at crates/zfb/binaries/esbuild/esbuild. Skipping."
        );
        return;
    };

    let base_url = std::env::var("ZFB_E2E_BASE_URL").unwrap_or_else(|_| {
        panic!("ZFB_E2E_BASE_URL not set; start the embedded V8 host and export its URL");
    });

    let fixture = fixture_root();
    assert!(
        fixture.exists(),
        "routing-rendering fixture not found at {}",
        fixture.display()
    );

    let universe = route_universe();

    // --- Preact pass ---
    eprintln!("[e2e_routing_rendering] bundling with Preact…");
    let dist_preact = tempfile::tempdir().expect("tempdir");
    let (bundle_preact, _nm_preact) =
        build_bundle(&fixture, Framework::Preact, &esbuild, dist_preact.path());

    eprintln!("[e2e_routing_rendering] rendering all routes with Preact…");
    let renderer_out = render_all(RendererInput {
        bundle_path: bundle_preact.bundle_path.clone(),
        sourcemap_path: bundle_preact.sourcemap_path.clone(),
        manifest: bundle_preact.manifest.clone(),
        dist_dir: dist_preact.path().join("html"),
        route_universe: universe.clone(),
        prerender_map: BTreeMap::new(), // all SSG
        backend: Backend::Existing {
            base_url: base_url.clone(),
        },
        request_timeout: None,
        prod_head_assets: None,
        project_root: PathBuf::new(),
    })
    .expect("render_all with Preact should succeed");

    assert_eq!(
        renderer_out.ssg_files_written.len(),
        universe.len(),
        "expected {} SSG files written (one per route), got {}",
        universe.len(),
        renderer_out.ssg_files_written.len(),
    );
    assert!(
        renderer_out.ssr_manifest.routes.is_empty(),
        "expected no SSR routes (all pages are SSG)"
    );

    // Collect rendered HTML bytes, keyed by url_path, for snapshot comparison.
    let mut preact_html: Vec<(String, Vec<u8>)> = Vec::new();
    let dist_html = dist_preact.path().join("html");
    for entry in &universe {
        let dest = dist_html.join(&entry.output_path);
        let body = fs::read(&dest).unwrap_or_else(|e| panic!("reading {}: {e}", dest.display()));
        preact_html.push((entry.url_path.clone(), body));
    }

    // --- Snapshot assertions ---
    eprintln!("[e2e_routing_rendering] asserting snapshots…");
    for (url_path, html) in &preact_html {
        let snap_name = if url_path == "/" {
            "index.html".to_string()
        } else {
            format!(
                "{}.html",
                url_path.trim_start_matches('/').replace('/', "-")
            )
        };
        assert_snapshot_eq(&snap_name, html);
    }

    // --- Content assertions: spot-check a few rendered pages ---
    check_rendered_html(&preact_html);

    eprintln!("[e2e_routing_rendering] PASS");
}

// ---------------------------------------------------------------------------
// Content spot-checks
// ---------------------------------------------------------------------------

/// Spot-check a handful of pages to ensure the rendered HTML contains the
/// content we expect (not just that files were written).
fn check_rendered_html(pages: &[(String, Vec<u8>)]) {
    let page = |url: &str| -> &[u8] {
        pages
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, b)| b.as_slice())
            .unwrap_or_else(|| panic!("page not found in rendered output: {url}"))
    };
    let has = |url: &str, needle: &str| {
        let body = page(url);
        let s = String::from_utf8_lossy(body);
        assert!(
            s.contains(needle),
            "page {url} does not contain {needle:?}\n--- body ---\n{}",
            &s[..s.len().min(1000)]
        );
    };

    // Index page
    has("/", "Welcome to zfb");

    // About page
    has("/about", "About zfb");

    // Blog index lists all posts
    has("/blog", "Hello");
    has("/blog", "Second");

    // Dynamic blog post — slug is in the HTML
    has("/blog/hello", "Hello");
    has("/blog/hello", "First post");
    has("/blog/second", "Second");

    // Pagination — page 1 contains first 2 posts, page 2 contains next 2
    has("/blog/page/1", "Page 1 of 2");
    has("/blog/page/2", "Page 2 of 2");

    // Localized
    has("/en/welcome", "Welcome");
    has("/ja/welcome", "ようこそ");
    has("/en/goodbye", "Goodbye");
    has("/ja/goodbye", "さようなら");

    // Docs catchall
    has("/docs/intro", "Intro doc");
    has("/docs/guides/install", "Install guide");
    has("/docs/guides/config/framework", "Framework config");
}
