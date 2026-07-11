//! End-to-end routing + rendering integration test for `zfb-build`.
//!
//! Exercises the full pipeline from bundler through the in-process
//! embedded V8 host across every routing pattern the framework supports:
//!
//!   - static index (`/`)
//!   - static named (`/about`)
//!   - static sub-index (`/blog`)
//!   - single-segment dynamic (`/blog/[slug]`)
//!   - pagination dynamic (`/blog/page/[page]`)
//!   - nested dynamic (`/[lang]/[slug]`)
//!   - catchall (`/docs/[...slug]`)
//!   - optional catchall (`/manual/[[...slug]]` — bare `/manual` + nested; #812)
//!
//! Fixture source lives at
//! `crates/zfb-render/tests/fixtures/routing-rendering/`. The test bundles
//! it with Preact via real esbuild, renders every concrete URL through an
//! in-process `Backend::EmbeddedV8` host, and compares (or, under
//! `INSANE_UPDATE_SNAPSHOTS=1`, rewrites) HTML snapshots under
//! `tests/snapshots/e2e_routing_rendering/`. All routes are SSG.
//!
//! The whole file is gated on the `embed_v8` feature: `Backend::EmbeddedV8`
//! and the thread-pinned host adapter below do not exist on the V8-off path.
//!
//! ## Skip conditions
//!
//! The test skips (prints a note, returns early) when:
//!
//! - No esbuild binary is available (resolves via `ZFB_ESBUILD_BIN`,
//!   `crates/zfb/binaries/esbuild/esbuild`, or the pnpm store).
//! - The pnpm store is missing the runtime deps the bundle needs
//!   (`preact`, `preact-render-to-string`, `hono`) — run `pnpm install`
//!   at the repo root.
//!
//! ## Snapshot bootstrap
//!
//! Run with `INSANE_UPDATE_SNAPSHOTS=1` to (re-)write the snapshots.
//! Without it, the test compares rendered HTML against the stored
//! snapshots and fails if they differ.
#![cfg(feature = "embed_v8")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

    // --- /manual/[[...slug]] — optional catchall (#812): the bare
    // directory URL (zero segments) plus one nested page ---
    entries.push(RouteUniverseEntry {
        url_path: "/manual".into(),
        output_path: PathBuf::from("manual/index.html"),
        route_key: "/manual/[[...slug]]".into(),
        static_html: false,
        source_path: None,
    });
    entries.push(RouteUniverseEntry {
        url_path: "/manual/setup/quick".into(),
        output_path: PathBuf::from("manual/setup/quick/index.html"),
        route_key: "/manual/[[...slug]]".into(),
        static_html: false,
        source_path: None,
    });

    entries
}

// ---------------------------------------------------------------------------
// Bundler helper
// ---------------------------------------------------------------------------

/// Locate a `node_modules/.pnpm/node_modules` directory that contains the
/// runtime deps (`preact`, `hono`, …) the bundle needs.
///
/// First tries the worktree root (where `pnpm install` drops it). If that
/// path is missing — common in a fresh `/x-wt-teams` worktree that has not
/// been `pnpm install`-ed because the manager session shares the main
/// repo's `node_modules` — walks upwards looking for a sibling main-repo
/// checkout. Returns `None` when no candidate exists; the test skips in
/// that case. Mirrors `embedded_v8_snapshot_e2e::locate_pnpm_node_modules`.
fn locate_pnpm_node_modules() -> Option<PathBuf> {
    let primary = workspace_root().join("node_modules/.pnpm/node_modules");
    if primary.exists() {
        return Some(primary);
    }
    let mut cursor = workspace_root();
    for _ in 0..4 {
        let candidate = cursor.join("node_modules/.pnpm/node_modules");
        if candidate.exists() {
            return Some(candidate);
        }
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent.to_path_buf();
    }
    None
}

/// Build a `node_modules` directory suitable for the fixture project.
///
/// The bundler runs from a shadow tempdir that has no `node_modules` of its
/// own. We create a minimal one pointing packages at the pnpm virtual store,
/// EXCEPT for `@takazudo/zfb-runtime` which must resolve to the **worktree**
/// copy so the router.ts changes under test are included in the bundle.
/// Using the main-repo copy (through the pnpm symlink) would give the old
/// code and make dynamic routes fail.
///
/// Returns `None` (graceful skip) when a required pnpm-store package is
/// missing, so a store that was never `pnpm install`-ed skips the test
/// instead of failing opaquely inside `bundle()`.
fn make_test_node_modules() -> Option<tempfile::TempDir> {
    let worktree_root = workspace_root();
    let pnpm_store = locate_pnpm_node_modules()?;

    let tmp = tempfile::tempdir().expect("tempdir for test node_modules");
    let nm = tmp.path();

    // Packages we need from the pnpm virtual store.
    let from_store: &[&str] = &["preact", "preact-render-to-string", "hono"];
    for pkg in from_store {
        let src = pnpm_store.join(pkg);
        if !src.exists() {
            eprintln!(
                "[e2e_routing_rendering] missing runtime dep `{pkg}` at {} — \
                 skipping test (run `pnpm install` at the repo root).",
                src.display(),
            );
            return None;
        }
        let dst = nm.join(pkg);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &dst)
            .unwrap_or_else(|e| panic!("symlink {}: {e}", src.display()));
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

    Some(tmp)
}

fn build_bundle(
    fixture_root: &Path,
    framework: Framework,
    esbuild: &Path,
    dist: &Path,
    node_modules: &Path,
) -> BundlerOutput {
    let input = BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: fixture_root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root: None,
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework,
        define_vars: std::collections::BTreeMap::new(),
        public_env_vars: Default::default(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![],
        outdir: dist.to_path_buf(),
        mode: BundleMode::Development,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: Some(node_modules.to_path_buf()),
        node_modules_preserve_symlinks: true,
        content_collections: Vec::new(),
        pipeline_spec: zfb_content::PipelineSpec::default(),
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
    };

    bundle(input).expect("bundle should succeed for fixture project")
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

/// End-to-end routing + rendering test using the in-process embedded V8 host.
///
/// Bundles the routing-rendering fixture with Preact (real esbuild) and
/// renders every route through an in-process `Backend::EmbeddedV8` host,
/// constructed by the thread-pinned `TestThreadedHost` adapter below. All
/// routes are SSG. Kick with:
///
///     cargo test -p zfb-build --test integration_e2e_routing_rendering
///
/// or, to (re-)bootstrap the snapshots:
///
///     INSANE_UPDATE_SNAPSHOTS=1 \
///       cargo test -p zfb-build --test integration_e2e_routing_rendering
#[test]
fn e2e_routing_rendering_with_embedded_host() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[e2e_routing_rendering] no esbuild binary; \
             set ZFB_ESBUILD_BIN or place at crates/zfb/binaries/esbuild/esbuild. Skipping."
        );
        return;
    };

    let fixture = fixture_root();
    assert!(
        fixture.exists(),
        "routing-rendering fixture not found at {}",
        fixture.display()
    );

    let universe = route_universe();

    // --- Preact pass ---
    eprintln!("[e2e_routing_rendering] bundling with Preact…");
    let Some(node_modules) = make_test_node_modules() else {
        eprintln!(
            "[e2e_routing_rendering] missing runtime deps in pnpm store; \
             skipping test (run `pnpm install` at the repo root)."
        );
        return;
    };
    let dist_preact = tempfile::tempdir().expect("tempdir");
    let bundle_preact = build_bundle(
        &fixture,
        Framework::Preact,
        &esbuild,
        dist_preact.path(),
        node_modules.path(),
    );

    eprintln!("[e2e_routing_rendering] rendering all routes with the embedded V8 host…");
    let renderer_out = render_all(RendererInput {
        bundle_path: bundle_preact.bundle_path.clone(),
        sourcemap_path: bundle_preact.sourcemap_path.clone(),
        manifest: bundle_preact.manifest.clone(),
        dist_dir: dist_preact.path().join("html"),
        route_universe: universe.clone(),
        prerender_map: BTreeMap::new(), // all SSG
        backend: Backend::EmbeddedV8 {
            host_factory: Arc::new(test_v8_host::TestThreadedHost::new),
        },
        request_timeout: None,
        prod_head_assets: None,
        project_root: PathBuf::new(),
    })
    .expect("render_all with the embedded V8 host should succeed");

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

    // Manual optional catchall (#812) — bare directory URL + nested path
    has("/manual", "Manual home");
    has("/manual/setup/quick", "Quick setup");
}

// ---------------------------------------------------------------------------
// Test-local thread-pinned embedded-V8 host adapter
// ---------------------------------------------------------------------------
//
// `Backend::EmbeddedV8` needs a `Box<dyn EmbeddedV8Host + Send>`, but
// `zfb_render::EmbeddedV8RenderHost` is `!Send` — it owns a V8 isolate, which
// deno_core pins to its creating thread. The production adapter that bridges
// this gap (`ThreadedV8Host` in `crates/zfb/src/v8_host_adapter.rs`) lives in
// the downstream `zfb` bin crate, so depending on it from `zfb-build` would
// form a dependency cycle (documented in `embedded_v8_snapshot_e2e.rs`).
//
// This is a minimal replica of `v8_host_adapter.rs:125-311`: it parks the
// isolate on a dedicated OS thread with its own current-thread tokio runtime
// and forwards each `dispatch_fetch` over an mpsc rendezvous channel (bound 0
// so the loop can never get more than one request ahead). It drops everything
// the production version carries that this SSG-only test does not need:
// plugin-registry hooks, the `DrainConsoleLogs` request kind, and the
// full-fidelity `dispatch_fetch_full` override (its trait default forwards to
// `dispatch_fetch`, which is all SSG GETs use).
mod test_v8_host {
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;

    use zfb_build::renderer::{EmbeddedV8Host, HttpResponseLike, RendererError};
    use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike};

    /// A single SSG GET forwarded to the pinned V8 thread, plus a one-shot
    /// reply channel the thread answers on.
    struct DispatchRequest {
        url_path: String,
        reply: mpsc::SyncSender<Result<HttpResponseLike, RendererError>>,
    }

    /// Minimal thread-pinned [`EmbeddedV8Host`] used only by this test.
    ///
    /// `tx`/`thread` are wrapped in `Option` so `Drop` can `take()` them:
    /// dropping the sender closes the channel, which breaks the V8 thread's
    /// `for req in rx` loop, after which the join completes.
    pub struct TestThreadedHost {
        tx: Option<mpsc::SyncSender<DispatchRequest>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl TestThreadedHost {
        /// Boot a V8 host for the bundle at `bundle_path` on a dedicated
        /// thread and load the bundle. Returns a boxed trait object so it
        /// slots straight into `Backend::EmbeddedV8`'s factory signature.
        ///
        /// Blocks until the host signals boot success or failure over a
        /// rendezvous boot channel.
        #[allow(clippy::new_ret_no_self)]
        pub fn new(bundle_path: &Path) -> Result<Box<dyn EmbeddedV8Host>, RendererError> {
            let (tx, rx) = mpsc::sync_channel::<DispatchRequest>(0);
            let (boot_tx, boot_rx) = mpsc::sync_channel::<Result<(), String>>(0);
            let bundle_path = bundle_path.to_path_buf();

            let thread = thread::Builder::new()
                .name("zfb-build-test-v8-host".into())
                .spawn(move || {
                    // A current-thread runtime keeps the isolate on this OS
                    // thread — deno_core's `JsRuntime` must never migrate.
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            let _ = boot_tx.send(Err(format!("tokio runtime build failed: {e}")));
                            return;
                        }
                    };
                    rt.block_on(async move {
                        use zfb_render::RenderHost as _;
                        let mut host = match EmbeddedV8RenderHost::new() {
                            Ok(h) => h,
                            Err(e) => {
                                let _ = boot_tx.send(Err(format!("V8 host init failed: {e}")));
                                return;
                            }
                        };
                        let src = match std::fs::read_to_string(&bundle_path) {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = boot_tx.send(Err(format!(
                                    "could not read bundle {}: {e}",
                                    bundle_path.display()
                                )));
                                return;
                            }
                        };
                        let name = bundle_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("bundle.mjs");
                        if let Err(e) = host.execute_module(name, &src).await {
                            // Surface any worker console output produced
                            // before the throw — the host dies with this
                            // thread, so embedding it in the boot error is
                            // the only way it reaches the caller.
                            let logs = host.drain_console_logs();
                            let msg = if logs.trim().is_empty() {
                                format!("bundle load failed: {e}")
                            } else {
                                format!("bundle load failed: {e}\nworker console output:\n{logs}")
                            };
                            let _ = boot_tx.send(Err(msg));
                            return;
                        }
                        let _ = boot_tx.send(Ok(()));
                        drop(boot_tx);

                        // Request loop: serve one SSG GET at a time.
                        for req in rx {
                            let http_req =
                                HttpRequestLike::get(format!("http://localhost{}", req.url_path));
                            let result = host
                                .dispatch_fetch(http_req)
                                .await
                                .map(|resp| {
                                    let content_type = resp
                                        .headers
                                        .get("content-type")
                                        .cloned()
                                        .unwrap_or_default();
                                    HttpResponseLike {
                                        status: resp.status,
                                        content_type,
                                        headers: resp.headers.into_iter().collect(),
                                        body: resp.body,
                                    }
                                })
                                .map_err(|e| RendererError::EmbeddedV8(e.to_string()));
                            // The caller may have already gone away; ignore
                            // send errors.
                            let _ = req.reply.send(result);
                        }
                    });
                })
                .map_err(|e| {
                    RendererError::EmbeddedV8(format!("could not spawn V8 host thread: {e}"))
                })?;

            match boot_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(msg)) => {
                    let _ = thread.join();
                    return Err(RendererError::EmbeddedV8(msg));
                }
                Err(_) => {
                    let _ = thread.join();
                    return Err(RendererError::EmbeddedV8(
                        "V8 host thread exited during boot without signalling".into(),
                    ));
                }
            }

            Ok(Box::new(TestThreadedHost {
                tx: Some(tx),
                thread: Some(thread),
            }))
        }
    }

    impl EmbeddedV8Host for TestThreadedHost {
        fn dispatch_fetch(&mut self, url_path: &str) -> Result<HttpResponseLike, RendererError> {
            let tx = self
                .tx
                .as_ref()
                .ok_or_else(|| RendererError::EmbeddedV8("V8 host already shut down".into()))?;
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            tx.send(DispatchRequest {
                url_path: url_path.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| RendererError::EmbeddedV8("V8 host thread exited unexpectedly".into()))?;
            reply_rx.recv().map_err(|_| {
                RendererError::EmbeddedV8("V8 host thread closed reply channel".into())
            })?
        }
    }

    impl Drop for TestThreadedHost {
        fn drop(&mut self) {
            // Close the channel first so the V8 thread's receive loop exits,
            // then join it.
            drop(self.tx.take());
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }
}
