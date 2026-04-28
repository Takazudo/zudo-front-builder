//! End-to-end routing + rendering integration test for `zfb-build`.
//!
//! Exercises the full pipeline from bundler through miniflare renderer
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
//! every concrete URL through a real miniflare subprocess, captures or
//! compares HTML snapshots under
//! `tests/snapshots/e2e_routing_rendering/`, and (for the portable-
//! component contract — ADR-002) asserts that the Preact and React
//! outputs are byte-identical.
//!
//! ## Skip conditions
//!
//! The test skips (prints a note, returns early) when:
//!
//! - No esbuild binary is available (resolves via `ZFB_ESBUILD_BIN`,
//!   `crates/zfb/binaries/esbuild/esbuild`, or `which esbuild`).
//! - `node` is not on PATH.
//! - `node_modules/miniflare` is not in the workspace root.
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
use std::process::Command;

use zfb_build::{
    bundle, render_all, Backend, BundleMode, BundlerInput, BundlerOutput, RendererInput,
    RouteUniverseEntry,
};
use zfb_render::adapters::Framework;

// ---------------------------------------------------------------------------
// Infrastructure helpers
// ---------------------------------------------------------------------------

/// Resolve the esbuild binary. Same order as `bundler_integration.rs`.
fn locate_esbuild() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
        if slot.exists() {
            return Some(slot);
        }
    }
    if let Ok(out) = Command::new("which").arg("esbuild").output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Walk up from `CARGO_MANIFEST_DIR` until we find a directory that
/// contains `node_modules/miniflare`. This handles both the normal
/// workspace layout (`crates/zfb-build` → `crates` → workspace root)
/// and the worktree layout (`.../worktrees/i63-e2e-test/crates/zfb-build`
/// → … → `.../zfb/`) where the pnpm store lives in the git root rather
/// than the worktree's parent.
///
/// Returns `None` when no such directory is found.
fn find_miniflare_workspace() -> Option<PathBuf> {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Walk-up search starting from the crate directory.
    // Collect ancestors into a Vec so we own each PathBuf.
    let ancestors: Vec<PathBuf> = {
        let mut v = Vec::new();
        let mut p: Option<&Path> = Some(here.as_path());
        while let Some(q) = p {
            v.push(q.to_path_buf());
            p = q.parent();
        }
        v
    };
    for dir in &ancestors {
        if dir.join("node_modules").join("miniflare").exists() {
            return Some(dir.clone());
        }
    }
    None
}

/// Return a workspace root that is known to have `node_modules/miniflare`.
/// Returns `None` when not found (test will skip).
fn workspace_root() -> Option<PathBuf> {
    find_miniflare_workspace()
}

/// Check whether miniflare is available in the workspace root.
/// When `workspace` is `None`, returns `false`.
fn miniflare_available(workspace: Option<&Path>) -> bool {
    workspace
        .map(|w| w.join("node_modules").join("miniflare").exists())
        .unwrap_or(false)
}

/// Check whether `node` is available on PATH.
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
/// not need to spin up a first miniflare just to call `__paths__`.
fn route_universe() -> Vec<RouteUniverseEntry> {
    let mut entries = Vec::new();

    // --- Static routes ---
    entries.push(RouteUniverseEntry {
        url_path: "/".into(),
        output_path: PathBuf::from("index.html"),
        route_key: "/".into(),
    });
    entries.push(RouteUniverseEntry {
        url_path: "/about".into(),
        output_path: PathBuf::from("about/index.html"),
        route_key: "/about".into(),
    });
    entries.push(RouteUniverseEntry {
        url_path: "/blog".into(),
        output_path: PathBuf::from("blog/index.html"),
        route_key: "/blog".into(),
    });

    // --- /blog/[slug] — one per post (matches fixture posts.ts) ---
    for slug in ["hello", "second", "third", "fourth"] {
        entries.push(RouteUniverseEntry {
            url_path: format!("/blog/{slug}"),
            output_path: PathBuf::from(format!("blog/{slug}/index.html")),
            route_key: "/blog/[slug]".into(),
        });
    }

    // --- /blog/page/[page] — 4 posts, 2 per page → 2 pages ---
    for page in [1u32, 2] {
        entries.push(RouteUniverseEntry {
            url_path: format!("/blog/page/{page}"),
            output_path: PathBuf::from(format!("blog/page/{page}/index.html")),
            route_key: "/blog/page/[page]".into(),
        });
    }

    // --- /[lang]/[slug] — 2 langs × 2 slugs ---
    for lang in ["en", "ja"] {
        for slug in ["welcome", "goodbye"] {
            entries.push(RouteUniverseEntry {
                url_path: format!("/{lang}/{slug}"),
                output_path: PathBuf::from(format!("{lang}/{slug}/index.html")),
                route_key: "/[lang]/[slug]".into(),
            });
        }
    }

    // --- /docs/[...slug] — 3 stub docs ---
    let doc_slugs: &[(&str, &[&str])] = &[
        ("intro", &["intro"]),
        ("guides-install", &["guides", "install"]),
        ("guides-config-framework", &["guides", "config", "framework"]),
    ];
    for (_name, segments) in doc_slugs {
        let url_path = format!("/docs/{}", segments.join("/"));
        let output_path = PathBuf::from(format!("docs/{}/index.html", segments.join("/")));
        entries.push(RouteUniverseEntry {
            url_path,
            output_path,
            route_key: "/docs/[...slug]".into(),
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
fn make_test_node_modules(workspace: &Path) -> tempfile::TempDir {
    // Infer the worktree root (two levels up from CARGO_MANIFEST_DIR:
    // crates/zfb-build → crates → worktree).
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let worktree_root = here
        .parent() // crates/
        .and_then(|p| p.parent()) // worktree root (e.g. .../i63-e2e-test/)
        .expect("worktree root from CARGO_MANIFEST_DIR")
        .to_path_buf();

    let pnpm_store = workspace.join("node_modules/.pnpm/node_modules");

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

    // `zfb` — point at the worktree's packages/zfb.
    let zfb_src = worktree_root.join("packages/zfb");
    let zfb_dst = nm.join("zfb");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, &zfb_dst)
        .unwrap_or_else(|e| panic!("symlink zfb: {e}"));

    tmp
}

fn build_bundle(
    fixture_root: &Path,
    framework: Framework,
    workspace: &Path,
    esbuild: &Path,
    dist: &Path,
) -> (BundlerOutput, tempfile::TempDir) {
    // Build a custom node_modules that uses the worktree's packages.
    let node_modules = make_test_node_modules(workspace);

    let input = BundlerInput {
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
    };

    let output = bundle(input).expect("bundle should succeed for fixture project");
    // Return the tempdir so the caller can keep it alive (dropping it would
    // delete the node_modules symlink while the bundle is still referencing it).
    (output, node_modules)
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

#[test]
fn e2e_routing_rendering_with_real_miniflare() {
    // --- Prerequisite checks: skip gracefully if tooling is absent ---
    let Some(workspace) = workspace_root() else {
        eprintln!(
            "[e2e_routing_rendering] node_modules/miniflare not found at any ancestor of \
             CARGO_MANIFEST_DIR. Skipping."
        );
        return;
    };
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[e2e_routing_rendering] no esbuild binary; \
             set ZFB_ESBUILD_BIN or place at crates/zfb/binaries/esbuild/esbuild. Skipping."
        );
        return;
    };
    if !node_available() {
        eprintln!("[e2e_routing_rendering] `node` not on PATH. Skipping.");
        return;
    }
    if !miniflare_available(Some(&workspace)) {
        eprintln!(
            "[e2e_routing_rendering] node_modules/miniflare not found at workspace root. Skipping."
        );
        return;
    }

    let fixture = fixture_root();
    assert!(
        fixture.exists(),
        "routing-rendering fixture not found at {}",
        fixture.display()
    );

    let universe = route_universe();

    // --- Preact pass ---
    eprintln!("[e2e_routing_rendering] bundling with Preact…");
    // Place the bundle inside the workspace so `resolve_subprocess_cwd`
    // (which walks up from bundle_path looking for node_modules/miniflare)
    // can find the workspace-root's node_modules. A tempdir under /tmp
    // has no ancestors with miniflare.
    let dist_preact = tempfile::Builder::new()
        .prefix("zfb-e2e-preact-")
        .tempdir_in(&workspace)
        .expect("tempdir inside workspace");
    let (bundle_preact, _nm_preact) = build_bundle(
        &fixture,
        Framework::Preact,
        &workspace,
        &esbuild,
        dist_preact.path(),
    );

    eprintln!("[e2e_routing_rendering] rendering all routes with Preact…");
    let renderer_out = render_all(RendererInput {
        bundle_path: bundle_preact.bundle_path.clone(),
        sourcemap_path: bundle_preact.sourcemap_path.clone(),
        manifest: bundle_preact.manifest.clone(),
        dist_dir: dist_preact.path().join("html"),
        route_universe: universe.clone(),
        prerender_map: BTreeMap::new(), // all SSG
        backend: Backend::SpawnMiniflare,
        request_timeout: None,
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

    // Collect rendered HTML bytes, keyed by url_path, for snapshot comparison
    // and optional React byte-equality check.
    let mut preact_html: Vec<(String, Vec<u8>)> = Vec::new();
    let dist_html = dist_preact.path().join("html");
    for entry in &universe {
        let dest = dist_html.join(&entry.output_path);
        let body = fs::read(&dest)
            .unwrap_or_else(|e| panic!("reading {}: {e}", dest.display()));
        preact_html.push((entry.url_path.clone(), body));
    }

    // --- Snapshot assertions ---
    eprintln!("[e2e_routing_rendering] asserting snapshots…");
    for (url_path, html) in &preact_html {
        // Convert URL path to a safe snapshot filename:
        //   /  → index.html
        //   /blog/hello  → blog-hello.html
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

    // --- Optional React byte-equality pass ---
    let pnpm_virtual_store = workspace.join("node_modules/.pnpm/node_modules");
    let react_available = pnpm_virtual_store.join("react").exists()
        && pnpm_virtual_store.join("react-dom").exists();

    if react_available {
        eprintln!("[e2e_routing_rendering] React available; bundling for byte-equality check…");
        let dist_react = tempfile::Builder::new()
            .prefix("zfb-e2e-react-")
            .tempdir_in(&workspace)
            .expect("tempdir inside workspace for react");
        let (bundle_react, _nm_react) = build_bundle(
            &fixture,
            Framework::React,
            &workspace,
            &esbuild,
            dist_react.path(),
        );

        let react_renderer_out = render_all(RendererInput {
            bundle_path: bundle_react.bundle_path.clone(),
            sourcemap_path: bundle_react.sourcemap_path.clone(),
            manifest: bundle_react.manifest.clone(),
            dist_dir: dist_react.path().join("html"),
            route_universe: universe.clone(),
            prerender_map: BTreeMap::new(),
            backend: Backend::SpawnMiniflare,
            request_timeout: None,
        })
        .expect("render_all with React should succeed");

        assert_eq!(
            react_renderer_out.ssg_files_written.len(),
            universe.len(),
            "React pass: expected {} files written",
            universe.len(),
        );

        let react_dist_html = dist_react.path().join("html");
        for (i, entry) in universe.iter().enumerate() {
            let dest = react_dist_html.join(&entry.output_path);
            let react_body = fs::read(&dest)
                .unwrap_or_else(|e| panic!("react: reading {}: {e}", dest.display()));
            let preact_body = &preact_html[i].1;
            assert_eq!(
                react_body, *preact_body,
                "ADR-002 byte-equality violated for {} \
                 (Preact and React outputs differ)",
                entry.url_path,
            );
        }
        eprintln!("[e2e_routing_rendering] React byte-equality: PASS");
    } else {
        eprintln!(
            "[e2e_routing_rendering] React not installed (react/react-dom absent from pnpm store); \
             skipping byte-equality check."
        );
    }

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
