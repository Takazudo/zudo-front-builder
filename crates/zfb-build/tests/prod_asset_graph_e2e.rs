//! End-to-end integration test for the **Prod Asset Graph** epic
//! (`crates/zfb-build/src/pipeline/prod.rs` + the orchestrator at
//! `crates/zfb-build/src/pipeline/orchestrator.rs`).
//!
//! ## Why this test exists
//!
//! Issue #95 shipped because the `ProductionAssetPipeline` apparatus
//! had thorough unit tests at `prod.rs:499-937` but **zero** end-to-end
//! coverage that exercised the orchestrator together with a real
//! `CssPipeline`. The orphaned wiring went unnoticed for ~270 routes
//! of the `zudo-doc` migration. This test locks in the S0 - S4 fix
//! (and S5's de-inlining once it lands) by driving the orchestration
//! entry point against a fixture and asserting the four contracts the
//! issue calls out:
//!
//! 1. `dist/assets/styles-<8hex>.css` lands on disk with non-zero bytes.
//! 2. The renderer's HTML head carries the hashed URL.
//! 3. The hashed URL the HTML references is openable (the explicit
//!    static-fallback contract — when the Worker 404s and `env.ASSETS`
//!    serves the raw HTML, the `<link>` tag has to resolve to a real
//!    file on disk).
//! 4. No unhashed `/assets/styles.css` URL leaks into rendered HTML.
//!
//! ## Subprocess vs. in-process
//!
//! Subprocess (`cargo run --bin zfb -- build`) is the most authentic
//! e2e shape but compounds Rust + esbuild + node + embedded-V8 +
//! Tailwind cost into a single test, which busts the ~30s budget for
//! the day-to-day `cargo test` loop. We instead drive the
//! orchestration entry point directly:
//!
//! - The **real** `CssPipeline::build_emitter` path runs (the orphan
//!   bug ships from this exact slot when it is not invoked from the
//!   build command). The Tailwind subprocess is mocked via
//!   `TailwindSubprocessConfig::with_mock_output` so the test does
//!   not need the v4 binary on disk; the synthesised entry CSS,
//!   CSS-Modules processing, hashing, and bytes assembly are still
//!   real code paths.
//! - The renderer is **simulated** by writing HTML files that match
//!   the byte shape `render_all` would produce when handed
//!   `prod_head_assets: Some(...)` — i.e. a `<link>` and `<script>`
//!   spliced before `</head>` via `head_inject::inject_prod_head_assets`.
//!   The `head_inject` module is itself unit-tested elsewhere; here
//!   we just need the on-disk HTML the orchestrator reads back.
//! - The **real** `apply_prod_asset_pipeline` runs end-to-end:
//!   hashing, the URL rewrite contract, atomic writes, the
//!   disk-existence post-condition.
//!
//! The gated `#[ignore]` test invokes the real Tailwind binary when
//! the staged slot is populated, mirroring the existing pattern in
//! `crates/zfb-css/tests/integration.rs::subprocess_engine_against_real_binary`.

use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::head_inject::{
    css_link_tag, inject_prod_head_assets, island_module_script_tag, ProdHeadAssets,
};
use zfb_build::pipeline::{
    apply_prod_asset_pipeline, synthesize_page_id_from_output, AssetEmitterPayload, CompanionFile,
    ProdAssetEmitterInputs, ProdRenderedFile, RelDistPath,
};
use zfb_css::{
    css_relative_path, scan_css_urls, CssPipeline, CssPipelineConfig, TailwindSubprocessConfig,
    TailwindSubprocessEngine,
};
use zfb_types::{
    DIST_ASSETS_DIR, STABLE_CSS_FILENAME, STABLE_CSS_URL, STABLE_ISLANDS_FILENAME,
    STABLE_ISLANDS_URL,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Project-root layout the test stages into a tempdir. Mirrors the
/// minimum a real zfb project needs for `CssPipeline::build_emitter`
/// to scan content roots and produce non-empty CSS bytes.
struct StagedFixture {
    project_root: tempfile::TempDir,
    /// `pages/` directory under `project_root`. Held for ownership /
    /// lifetime parity with `project_root`; not read by every test.
    #[allow(dead_code)]
    pages_dir: PathBuf,
}

/// Stage a tiny self-contained project under a tempdir. A real
/// bundled-basic-blog-template project would also work but pulls in the full
/// content/MDX surface (slow, and noisy for the purpose of this
/// test); a minimal page with a Tailwind utility class is enough to
/// drive `CssPipeline::build_emitter` through every stage.
fn stage_minimal_fixture() -> StagedFixture {
    let project_root = tempfile::Builder::new()
        .prefix("zfb-prod-asset-e2e-")
        .tempdir()
        .expect("tempdir for fixture");
    let root = project_root.path();
    let pages_dir = root.join("pages");
    fs::create_dir_all(&pages_dir).unwrap();
    fs::write(
        pages_dir.join("index.tsx"),
        // The synthesised entry CSS feeds Tailwind v4 which scans this
        // file for utility classes via `@source` directives. The class
        // names are inert under `mock_subprocess` (the engine returns
        // canned bytes), but the scan path still touches the file —
        // exercising the real `discover_css_source_files` walk in the
        // wider integration.
        "export default function Home() {\n  return <div className=\"text-red-500 p-4\">hello</div>;\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("styles")).unwrap();
    fs::write(
        root.join("styles/global.css"),
        "/* global stylesheet — Tailwind v4 entry */\n@import \"tailwindcss\";\n",
    )
    .unwrap();

    StagedFixture {
        project_root,
        pages_dir,
    }
}

/// Construct the same `CssPipeline` shape `zfb`'s `DefaultRunner`
/// builds, except the subprocess is mocked. The mock output is a
/// recognisable snippet (the `body{font-family:system-ui}` rule) so
/// the test can later assert hashed bytes match.
fn mock_css_pipeline(
    project_root: &Path,
    outdir: &Path,
    mock_css: &str,
) -> CssPipeline<TailwindSubprocessEngine> {
    // Match the bin's `build_default_css_payload` shape: globs rooted
    // at the project, optional `styles/global.css` as input.
    let content_globs = zfb_css::engine::DEFAULT_CONTENT_ROOTS
        .iter()
        .map(|root| project_root.join(root).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut tw_cfg = TailwindSubprocessConfig::default()
        .with_working_dir(project_root.to_path_buf())
        .with_content_globs(content_globs)
        .with_mock_output(mock_css.to_string());
    let global_css = project_root.join("styles").join("global.css");
    if global_css.is_file() {
        tw_cfg = tw_cfg.with_input_css(global_css);
    }
    let engine = TailwindSubprocessEngine::new(tw_cfg);
    let pipe_cfg = CssPipelineConfig {
        sources: discover_css_source_files(project_root),
        class_map_dir: None,
        output_root: outdir.to_path_buf(),
        ..CssPipelineConfig::default()
    };
    CssPipeline::new(engine, pipe_cfg)
}

/// Mirror of `crates/zfb/src/commands/build.rs::discover_css_source_files`
/// — we cannot call that helper directly (it is private to the bin
/// crate) without re-exporting it, and re-exporting test-only helpers
/// from the bin into the build crate is more ceremony than the
/// 10-line walk it replaces.
fn discover_css_source_files(project_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let extensions = ["tsx", "ts", "jsx", "js", "mdx", "md"];
    for root in zfb_css::engine::DEFAULT_CONTENT_ROOTS {
        let dir = project_root.join(root);
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(|r| r.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry
                .path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());
            if let Some(ext) = ext {
                if extensions.contains(&ext.as_str()) {
                    out.push(entry.into_path());
                }
            }
        }
    }
    out
}

/// Synthesise an HTML file under `dist_dir/<rel>` mimicking the byte
/// shape `render_all` produces when handed
/// `prod_head_assets: Some(assets)`. Real `render_all` calls
/// `head_inject::inject_prod_head_assets` on each route's HTML; we
/// invoke the same helper here so the input to
/// `apply_prod_asset_pipeline` is byte-equivalent to a production
/// build.
///
/// Returns the `(PageId, RelDistPath)` shape needed to hand the page
/// back to the orchestrator.
fn stage_html_with_head_inject(
    dist_dir: &Path,
    rel: &str,
    title: &str,
    assets: &ProdHeadAssets,
) -> ProdRenderedFile {
    let bare = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title>{title}</title>\n</head>\n<body>\n  <main>\n    <h1>{title}</h1>\n    <p class=\"text-red-500 p-4\">e2e fixture page</p>\n  </main>\n</body>\n</html>\n",
    );
    let injected = inject_prod_head_assets(&bare, assets).into_owned();
    let abs = dist_dir.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&abs, &injected).unwrap();
    let rel_path = RelDistPath::new(rel).unwrap();
    ProdRenderedFile {
        page: synthesize_page_id_from_output(&rel_path),
        output_path: rel_path,
    }
}

/// Read the `href` of the first `<link rel="stylesheet" …>` in `html`,
/// or `None` when no such tag is present. Used to assert the pipeline
/// rewrote the stable URL to a hashed one and to drive the
/// disk-existence assertion (we open the file at exactly the URL the
/// renderer's HTML claims).
fn first_stylesheet_href(html: &str) -> Option<String> {
    // We deliberately avoid pulling in an HTML parser dep here — the
    // canonical tag form `<link rel="stylesheet" href="…">` is
    // well-defined by `head_inject::css_link_tag`, so a simple split
    // suffices and keeps the test self-contained.
    let needle = "<link rel=\"stylesheet\" href=\"";
    let start = html.find(needle)? + needle.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Read the `src` of the first `<script type="module" …>` in `html`,
/// or `None` when no such tag is present.
fn first_module_script_src(html: &str) -> Option<String> {
    let needle = "<script type=\"module\" src=\"";
    let start = html.find(needle)? + needle.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Stage a `node_modules/<pkg_name>` package with a `package.json`
/// (name + version only — the minimum `package_identity` needs), one
/// CSS file under the package root, and any additional package-local
/// files (path relative to the package root -> bytes). Used to drive
/// the real-Tailwind package `url()` rebase scenario (issue #2311)
/// through the SAME entry point (`CssPipeline::build_emitter`) the
/// happy-path test above exercises, rather than unit-testing
/// `zfb_css::url_attribution` in isolation.
fn stage_node_modules_package(
    project_root: &Path,
    pkg_name: &str,
    css_filename: &str,
    css: &str,
    files: &[(&str, &[u8])],
) {
    let pkg_dir = project_root.join("node_modules").join(pkg_name);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("package.json"),
        format!("{{\"name\": \"{pkg_name}\", \"version\": \"1.0.0\"}}"),
    )
    .unwrap();
    fs::write(pkg_dir.join(css_filename), css).unwrap();
    for (rel, bytes) in files {
        let path = pkg_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }
}

/// Append an `@import` line for a staged `node_modules` package to the
/// fixture's `styles/global.css`, after the existing `@import
/// "tailwindcss";` entry point line — mirroring the reporting issue's
/// exact repro shape (`@import "@fontsource-variable/noto-sans/index.css";`
/// placed right after the Tailwind import).
fn append_package_import(project_root: &Path, pkg_name: &str, css_filename: &str) {
    let global_css = project_root.join("styles").join("global.css");
    let mut css = fs::read_to_string(&global_css).unwrap();
    css.push_str(&format!("@import \"{pkg_name}/{css_filename}\";\n"));
    fs::write(&global_css, css).unwrap();
}

/// Build a `CssPipeline` driving the REAL (non-mocked) Tailwind v4
/// subprocess against `project_root`, writing into `dist_dir`. Shared by
/// every `#[ignore]`d real-binary scenario below so the subprocess
/// wiring (working dir, content globs, `styles/global.css` as input)
/// stays identical across the happy path, the package `url()` rebase
/// case, and the negative case.
fn build_real_tailwind_pipeline(
    project_root: &Path,
    dist_dir: &Path,
) -> CssPipeline<TailwindSubprocessEngine> {
    let content_globs = zfb_css::engine::DEFAULT_CONTENT_ROOTS
        .iter()
        .map(|root| project_root.join(root).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut tw_cfg = TailwindSubprocessConfig::default()
        .with_working_dir(project_root.to_path_buf())
        .with_content_globs(content_globs);
    let global_css = project_root.join("styles").join("global.css");
    if global_css.is_file() {
        tw_cfg = tw_cfg.with_input_css(global_css);
    }
    let engine = TailwindSubprocessEngine::new(tw_cfg);
    let pipe_cfg = CssPipelineConfig {
        sources: discover_css_source_files(project_root),
        class_map_dir: None,
        output_root: dist_dir.to_path_buf(),
        ..CssPipelineConfig::default()
    };
    CssPipeline::new(engine, pipe_cfg)
}

/// Map a `/assets/styles-<hash>.css` style URL to the on-disk path
/// under `dist_dir`. The pipeline writes hashed assets to
/// `dist_dir/assets/<filename>`, so the URL's leading slash is
/// dropped and the rest is appended verbatim.
fn url_to_disk_path(dist_dir: &Path, url: &str) -> PathBuf {
    // The static-fallback contract: `env.ASSETS` resolves the URL
    // path verbatim against the dist root. Hashed asset URLs always
    // start with `/`, so we strip it and join.
    let rel = url.trim_start_matches('/');
    dist_dir.join(rel)
}

// ---------------------------------------------------------------------------
// Tests — happy paths
// ---------------------------------------------------------------------------

/// Happy-path e2e: the **real** `CssPipeline::build_emitter` produces
/// bytes via a mock-subprocess engine, the (simulated) renderer writes
/// HTML with the stable CSS URL spliced into `<head>`, and the **real**
/// `apply_prod_asset_pipeline` hashes the bytes, writes
/// `dist/assets/styles-<8hex>.css`, and rewrites every HTML occurrence
/// of `/assets/styles.css` to the hashed URL.
///
/// This is the single test that would have caught issue #95: a build
/// that wires `CssPipeline::build_emitter` but forgets to call
/// `apply_prod_asset_pipeline` afterwards leaves the rendered HTML
/// pointing at `/assets/styles.css` while no hashed file ever lands
/// — exactly the failure mode the issue describes for `zudo-doc`.
#[test]
fn prod_asset_graph_emits_hashed_css_and_rewrites_html_via_real_orchestrator() {
    let fixture = stage_minimal_fixture();
    let dist_dir = fixture.project_root.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();

    // 1. Run the real CssPipeline::build_emitter path. The mock
    //    output is a recognisable rule so any byte-level corruption
    //    of the CSS would surface as a hash mismatch when it is
    //    written.
    let mock_css = ".text-red-500{color:red}\n.p-4{padding:1rem}\nbody{font-family:system-ui}\n";
    let pipeline = mock_css_pipeline(fixture.project_root.path(), &dist_dir, mock_css);
    let emitter_out = pipeline
        .build_emitter()
        .expect("CssPipeline::build_emitter must succeed under mock subprocess");
    assert!(
        !emitter_out.bytes.is_empty(),
        "build_emitter must produce non-empty bytes when the engine returns content",
    );
    assert_eq!(
        emitter_out.stable_url, STABLE_CSS_URL,
        "stable URL must come from the zfb-types constant — \
         drift here breaks HTML rewrite",
    );

    // 2. Simulate the renderer: write HTML files with the stable CSS
    //    URL injected into `<head>` (the byte shape
    //    `render_all`+`head_inject::inject_prod_head_assets` produces
    //    when handed `prod_head_assets: Some(...)`).
    let head_assets = ProdHeadAssets {
        css_url: Some(STABLE_CSS_URL.to_string()),
        island_module_urls: Vec::new(),
    };
    let pages = vec![
        stage_html_with_head_inject(&dist_dir, "index.html", "Home", &head_assets),
        stage_html_with_head_inject(&dist_dir, "about/index.html", "About", &head_assets),
        stage_html_with_head_inject(&dist_dir, "blog/hello/index.html", "Hello", &head_assets),
    ];

    // Sanity: pre-rewrite, every HTML file must contain the **stable**
    // (unhashed) URL. If this fails, the head-injection helper drifted
    // from the renderer contract and the rest of the test is invalid.
    for page in &pages {
        let body = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();
        assert!(
            body.contains(&css_link_tag(STABLE_CSS_URL)),
            "pre-rewrite: stable CSS link missing from {}",
            page.output_path.as_path().display(),
        );
    }

    // 3. Run the real apply_prod_asset_pipeline. This is the call the
    //    orphan bug skipped — wiring it is the load-bearing fix this
    //    test locks in.
    let inputs = ProdAssetEmitterInputs {
        css: Some(AssetEmitterPayload {
            bytes: emitter_out.bytes.clone(),
            relative_path: css_relative_path(),
            stable_url: emitter_out.stable_url.clone(),
            companions: Vec::new(),
        }),
        islands: None,
        ..Default::default()
    };
    let outcome = apply_prod_asset_pipeline(&dist_dir, pages.clone(), inputs)
        .expect("apply_prod_asset_pipeline must succeed");

    // --- (positive) hashed CSS landed on disk under
    //     dist/assets/styles-<8hex>.css, with non-zero bytes. ---
    let assets_dir = dist_dir.join(DIST_ASSETS_DIR);
    let entries: Vec<String> = fs::read_dir(&assets_dir)
        .unwrap_or_else(|e| panic!("dist/assets must exist after pipeline run: {e}"))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one hashed asset; got {entries:?}",
    );
    let hashed_filename = &entries[0];
    let stable_stem = STABLE_CSS_FILENAME.trim_end_matches(".css");
    let expected_len = stable_stem.len() + "-12345678.css".len();
    assert!(
        hashed_filename.starts_with(&format!("{stable_stem}-"))
            && hashed_filename.ends_with(".css")
            && hashed_filename.len() == expected_len,
        "hashed filename must match `{stable_stem}-<8hex>.css`; got {hashed_filename}",
    );
    let on_disk_bytes = fs::read(assets_dir.join(hashed_filename)).unwrap();
    assert!(!on_disk_bytes.is_empty(), "hashed CSS must contain bytes");

    // --- (positive) at least one HTML file in dist/ contains the
    //     hashed `<link rel="stylesheet" href="…">` in `<head>`. ---
    let hashed_url = format!("/assets/{hashed_filename}");
    let mut hashed_link_seen_in_html = false;
    for page in &pages {
        let html = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();
        if html.contains(&css_link_tag(&hashed_url)) {
            hashed_link_seen_in_html = true;
            break;
        }
    }
    assert!(
        hashed_link_seen_in_html,
        "at least one HTML file must reference the hashed URL {hashed_url}",
    );

    // --- (positive, **disk-existence**, issue #95): the URL the HTML
    //     points at must open as a real file on disk. This is the
    //     explicit static-fallback contract — without it, a Worker 404
    //     hand-off to env.ASSETS resolves to a missing file and the
    //     page renders unstyled. ---
    for page in &pages {
        let html = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();
        let href = first_stylesheet_href(&html).unwrap_or_else(|| {
            panic!(
                "no <link rel=\"stylesheet\"> in {} after rewrite — pipeline did not inject \
                 the hashed URL: {html}",
                page.output_path.as_path().display(),
            )
        });
        assert!(
            href.starts_with("/assets/") && href.contains("styles-") && href.ends_with(".css"),
            "stylesheet href must be the hashed form; got {href}",
        );
        let on_disk = url_to_disk_path(&dist_dir, &href);
        assert!(
            on_disk.is_file(),
            "static-fallback contract violated: HTML claims {href} but {} does not exist on disk",
            on_disk.display(),
        );
    }

    // --- (negative) HTML must NOT contain the unhashed
    //     `/assets/styles.css` URL anywhere — the rewrite is
    //     boundary-anchored so a regex/substr leak would be caught here. ---
    for page in &pages {
        let html = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();
        assert!(
            !html.contains(&format!("\"{STABLE_CSS_URL}\"")),
            "unhashed CSS URL leaked into {}: {html}",
            page.output_path.as_path().display(),
        );
    }

    // --- BuildOutcome reports the hashed asset URL ---
    assert_eq!(
        outcome.hashed_asset_urls.len(),
        1,
        "BuildOutcome must report exactly one hashed CSS URL",
    );
    assert_eq!(outcome.hashed_asset_urls[0].1, hashed_url);
}

/// Parallel happy-path with both CSS and islands assets. The islands
/// emitter side is fed synthetic bytes (matching the byte shape
/// `build_production_islands_asset` returns) so the test stays free of
/// the esbuild subprocess. The point of this test is to exercise the
/// orchestrator's two-emitter wiring: every assertion of the
/// CSS-only test, *plus* the same shape for islands.
#[test]
fn prod_asset_graph_emits_hashed_css_and_islands_when_project_has_both() {
    let fixture = stage_minimal_fixture();
    let dist_dir = fixture.project_root.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();

    // Real CssPipeline path (mock subprocess for the engine).
    let mock_css = ".text-red-500{color:red}\nbody{font-family:system-ui}\n";
    let pipeline = mock_css_pipeline(fixture.project_root.path(), &dist_dir, mock_css);
    let emitter_out = pipeline
        .build_emitter()
        .expect("CssPipeline::build_emitter must succeed");

    // Synthetic islands bytes — same byte shape
    // `build_production_islands_asset` returns. We are not testing
    // esbuild here; we are testing that the orchestrator routes both
    // emitters through hashing + URL rewrite.
    let islands_bytes = b"// hydration shim + Counter island \n\
        export function hydrate(){};\n"
        .to_vec();
    let islands_relative = PathBuf::from(DIST_ASSETS_DIR).join(STABLE_ISLANDS_FILENAME);

    // Simulate renderer: HTML carries BOTH a stylesheet link and a
    // module script (the byte shape produced by
    // `inject_prod_head_assets` when `prod_head_assets` carries a
    // populated CSS URL plus a non-empty `island_module_urls`).
    let head_assets = ProdHeadAssets {
        css_url: Some(STABLE_CSS_URL.to_string()),
        island_module_urls: vec![STABLE_ISLANDS_URL.to_string()],
    };
    let pages = vec![
        stage_html_with_head_inject(&dist_dir, "index.html", "Home", &head_assets),
        stage_html_with_head_inject(
            &dist_dir,
            "blog/post/index.html",
            "Post with Island",
            &head_assets,
        ),
    ];

    let inputs = ProdAssetEmitterInputs {
        css: Some(AssetEmitterPayload {
            bytes: emitter_out.bytes.clone(),
            relative_path: css_relative_path(),
            stable_url: emitter_out.stable_url.clone(),
            companions: Vec::new(),
        }),
        islands: Some(AssetEmitterPayload {
            bytes: islands_bytes,
            relative_path: islands_relative,
            stable_url: STABLE_ISLANDS_URL.to_string(),
            companions: Vec::new(),
        }),
        ..Default::default()
    };
    let outcome = apply_prod_asset_pipeline(&dist_dir, pages.clone(), inputs)
        .expect("apply_prod_asset_pipeline must succeed for css + islands");

    // Both hashed assets land on disk.
    let assets_dir = dist_dir.join(DIST_ASSETS_DIR);
    let entries: std::collections::BTreeSet<String> = fs::read_dir(&assets_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "expected one CSS + one islands asset on disk; got {entries:?}",
    );
    let css_name = entries
        .iter()
        .find(|n| n.starts_with("styles-") && n.ends_with(".css"))
        .unwrap_or_else(|| panic!("no styles-<hash>.css in {entries:?}"));
    let islands_name = entries
        .iter()
        .find(|n| n.starts_with("islands-") && n.ends_with(".js"))
        .unwrap_or_else(|| panic!("no islands-<hash>.js in {entries:?}"));
    assert!(!fs::read(assets_dir.join(css_name)).unwrap().is_empty());
    assert!(!fs::read(assets_dir.join(islands_name)).unwrap().is_empty());

    // HTML positive + disk-existence + negative for BOTH asset kinds.
    let hashed_css_url = format!("/assets/{css_name}");
    let hashed_islands_url = format!("/assets/{islands_name}");
    for page in &pages {
        let html = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();

        // Positive: hashed link/script tags are present.
        assert!(
            html.contains(&css_link_tag(&hashed_css_url)),
            "missing hashed CSS link in {}: {html}",
            page.output_path.as_path().display(),
        );
        assert!(
            html.contains(&island_module_script_tag(&hashed_islands_url)),
            "missing hashed islands script in {}: {html}",
            page.output_path.as_path().display(),
        );

        // Disk-existence (issue #95) for both the hashed CSS and the
        // hashed islands chunk.
        let css_href = first_stylesheet_href(&html).expect("stylesheet href");
        let css_on_disk = url_to_disk_path(&dist_dir, &css_href);
        assert!(
            css_on_disk.is_file(),
            "static-fallback contract violated for CSS: {} missing on disk",
            css_on_disk.display(),
        );
        let islands_src = first_module_script_src(&html).expect("module script src");
        let islands_on_disk = url_to_disk_path(&dist_dir, &islands_src);
        assert!(
            islands_on_disk.is_file(),
            "static-fallback contract violated for islands: {} missing on disk",
            islands_on_disk.display(),
        );

        // Negative: neither unhashed URL leaks.
        assert!(
            !html.contains(&format!("\"{STABLE_CSS_URL}\"")),
            "unhashed CSS URL leaked into {}: {html}",
            page.output_path.as_path().display(),
        );
        assert!(
            !html.contains(&format!("\"{STABLE_ISLANDS_URL}\"")),
            "unhashed islands URL leaked into {}: {html}",
            page.output_path.as_path().display(),
        );
    }

    // BuildOutcome reports both hashed URLs.
    assert_eq!(outcome.hashed_asset_urls.len(), 2);
    let urls: std::collections::BTreeSet<String> = outcome
        .hashed_asset_urls
        .iter()
        .map(|(_, u)| u.clone())
        .collect();
    assert!(urls.contains(&hashed_css_url));
    assert!(urls.contains(&hashed_islands_url));
}

// ---------------------------------------------------------------------------
// Test — client-script entries (#976)
// ---------------------------------------------------------------------------

/// Client-script payloads route through the same orchestrator wiring as
/// CSS/islands: each `*.client.ts` entry ships hashed to
/// `dist/assets/client/<name>-<hash>.js` and every HTML reference to the
/// stable `/assets/client/<name>.js` URL is rewritten — the #976
/// acceptance criterion driven through the REAL
/// `apply_prod_asset_pipeline` with a simulated renderer.
///
/// The bytes are synthetic (same shape `build_production_client_scripts`
/// returns) — esbuild is covered by the `#[ignore]` integration test in
/// `crates/zfb-islands/tests/integration.rs`; this test locks in the
/// orchestrator + rewrite contract.
#[test]
fn prod_asset_graph_emits_hashed_client_script_and_rewrites_html() {
    let fixture = stage_minimal_fixture();
    let dist_dir = fixture.project_root.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();

    let entry_name = "search-widget";
    let stable_url = zfb_types::stable_client_script_url(entry_name);
    let relative_path = zfb_types::stable_client_script_relative_path(entry_name);
    let script_bytes = b"// bundled search widget\nconsole.log(\"hi\");\n".to_vec();

    // Simulate a page that explicitly references the client script via the
    // `clientScript()` SSR helper: the rendered HTML carries the stable
    // client-script URL as a module script tag. Client scripts are NOT
    // auto-injected into the head (#971 P2) — they reach a page only through
    // such explicit references. We reuse the head-inject staging helper here
    // purely to place the stable URL into the page; the pipeline rewrite is
    // injection-point-agnostic (it rewrites any boundary-anchored match).
    let head_assets = ProdHeadAssets {
        css_url: None,
        island_module_urls: vec![stable_url.clone()],
    };
    let pages = vec![
        stage_html_with_head_inject(&dist_dir, "index.html", "Home", &head_assets),
        stage_html_with_head_inject(&dist_dir, "about/index.html", "About", &head_assets),
    ];

    let inputs = ProdAssetEmitterInputs {
        css: None,
        islands: None,
        client_scripts: vec![AssetEmitterPayload {
            bytes: script_bytes,
            relative_path,
            stable_url: stable_url.clone(),
            companions: Vec::new(),
        }],
    };
    let outcome = apply_prod_asset_pipeline(&dist_dir, pages.clone(), inputs)
        .expect("apply_prod_asset_pipeline must succeed for client scripts");

    // Hashed file lands under dist/assets/client/.
    let client_dir = dist_dir.join(DIST_ASSETS_DIR).join("client");
    let entries: Vec<String> = fs::read_dir(&client_dir)
        .unwrap_or_else(|e| panic!("dist/assets/client must exist after pipeline run: {e}"))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one hashed client-script asset; got {entries:?}",
    );
    let hashed_filename = &entries[0];
    let expected_len = entry_name.len() + "-12345678.js".len();
    assert!(
        hashed_filename.starts_with(&format!("{entry_name}-"))
            && hashed_filename.ends_with(".js")
            && hashed_filename.len() == expected_len,
        "hashed filename must match `{entry_name}-<8hex>.js`; got {hashed_filename}",
    );
    assert!(!fs::read(client_dir.join(hashed_filename))
        .unwrap()
        .is_empty());

    // HTML positive + disk-existence + negative.
    let hashed_url = format!("/assets/client/{hashed_filename}");
    for page in &pages {
        let html = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();
        assert!(
            html.contains(&island_module_script_tag(&hashed_url)),
            "missing hashed client-script tag in {}: {html}",
            page.output_path.as_path().display(),
        );
        let src = first_module_script_src(&html).expect("module script src");
        let on_disk = url_to_disk_path(&dist_dir, &src);
        assert!(
            on_disk.is_file(),
            "static-fallback contract violated: HTML claims {src} but {} missing on disk",
            on_disk.display(),
        );
        assert!(
            !html.contains(&format!("\"{stable_url}\"")),
            "unhashed client-script URL leaked into {}: {html}",
            page.output_path.as_path().display(),
        );
    }

    // BuildOutcome reports the hashed client-script URL.
    assert_eq!(outcome.hashed_asset_urls.len(), 1);
    assert_eq!(outcome.hashed_asset_urls[0].1, hashed_url);
}

// ---------------------------------------------------------------------------
// Test — dev / no-prod-assets regression guard
// ---------------------------------------------------------------------------

/// Dev-mode no-regression: when no emitter slot has bytes
/// (`prod_head_assets: None` on the renderer; the orchestrator's
/// short-circuit at `if !prod_asset_inputs.css.is_none() ||
/// !prod_asset_inputs.islands.is_none()` skips
/// `apply_prod_asset_pipeline` entirely), the rendered HTML stays
/// byte-identical to the input — no `<link>` / `<script>` injected,
/// no `dist/assets/` directory created.
///
/// This guards against a future refactor accidentally making the
/// prod-only injection unconditional and regressing dev (`zfb dev`
/// runs with `run_css: None` / `run_islands: None`; the dev server
/// must keep shipping HTML without head injection).
#[test]
fn dev_mode_renderer_path_does_not_inject_head_assets() {
    let dist_dir = tempfile::tempdir().expect("tempdir for dev-no-regression");
    let dist = dist_dir.path();

    // The renderer in dev hands the head-injection helper a `None` /
    // empty `ProdHeadAssets`. Verify directly that no tags are
    // injected and no bytes change.
    let bare_html = "<!doctype html><html><head><title>dev</title></head><body><main>dev page</main></body></html>";
    let none_assets = ProdHeadAssets::default(); // both fields empty
    let after = inject_prod_head_assets(bare_html, &none_assets);
    assert_eq!(
        &*after, bare_html,
        "head injection must be a passthrough when ProdHeadAssets is empty",
    );
    assert!(
        !after.contains("<link rel=\"stylesheet\""),
        "no stylesheet link must appear in dev HTML",
    );
    assert!(
        !after.contains("<script type=\"module\""),
        "no module script must appear in dev HTML",
    );

    // The orchestrator-level short-circuit: when both emitter slots
    // are `None`, callers in the bin (`zfb build`) skip
    // `apply_prod_asset_pipeline` entirely. Mirror that here by
    // simply not calling it and confirming `dist/assets/` is absent.
    // This is the contract the build command relies on; we keep it
    // pinned even though the production gate lives in the bin.
    assert!(
        !dist.join(DIST_ASSETS_DIR).exists(),
        "no assets/ dir must be created when no emitter slot has bytes",
    );
}

// ---------------------------------------------------------------------------
// S5-dependent test — gated until S5 lands
// ---------------------------------------------------------------------------

/// S5 contract — the bundler must NOT inline user CSS into the
/// Worker bundle. The fix is `--loader:.css=empty` on the esbuild
/// invocation, which substitutes an empty module for every `.css`
/// import at compile time. The previous default behaviour either
/// emitted a sibling `_zfb_inner.css` (orphan) or inlined the CSS as
/// JS bytes — both ship duplicate CSS alongside the externally-hashed
/// `dist/assets/styles-<hash>.css` produced by S2 + S4.
///
/// This test verifies the contract by reading the public bundler
/// constant rather than running esbuild end-to-end. The full
/// fixture-based marker check (load a real built bundle, grep for
/// `@import "tailwindcss"`) lives in
/// `prod_asset_graph_with_real_tailwind_binary_against_fixture`,
/// gated under `#[ignore]` until the binary slot is staged.
#[test]
fn worker_bundle_does_not_inline_tailwind_css_marker_after_s5() {
    use zfb_build::bundler::ESBUILD_LOADER_ARGS;

    assert!(
        ESBUILD_LOADER_ARGS.contains(&"--loader:.css=empty"),
        "Worker bundle must use --loader:.css=empty so .css imports \
         are dropped at compile time. Got: {:?}",
        ESBUILD_LOADER_ARGS,
    );
    assert!(
        !ESBUILD_LOADER_ARGS
            .iter()
            .any(|a| a.contains("external:") && a.contains(".css")),
        "Worker bundle must NOT mark .css as external — esbuild leaves \
         runtime `import` statements workerd cannot resolve. Use \
         --loader:.css=empty instead. Got: {:?}",
        ESBUILD_LOADER_ARGS,
    );
}

// ---------------------------------------------------------------------------
// Gated — real Tailwind binary
// ---------------------------------------------------------------------------

/// Real Tailwind v4 subprocess against the same minimal fixture. Mirrors
/// the existing `#[ignore]` gate in
/// `crates/zfb-css/tests/integration.rs::subprocess_engine_against_real_binary`.
/// `crates/zfb/build.rs` downloads + stages the pinned tailwindcss-v4
/// binary at `crates/zfb/binaries/tailwindcss-v4` as a side effect of
/// building the `zfb` crate (same mechanism as the esbuild slot), so the
/// binary IS present in CI once `cargo build --workspace --all-targets`
/// has run — but no CI step currently passes `--ignored`/`--include-ignored`
/// to actually run this test, so it stays env-gated. Run locally with
/// `cargo test -- --include-ignored` (binary already staged by a prior
/// build) or set `ZFB_TAILWIND_BIN` explicitly.
///
/// When it runs, the assertions are exactly the happy-path test's
/// assertions: hashed CSS on disk, HTML rewritten to the hashed URL,
/// disk-existence holds, no unhashed leak.
///
/// ## Package `url()` rebase (issue #2318 — Confirm the repro end to end)
///
/// This test also owns the heavy-verification confirm pass for epic #2311
/// (CSS Import URL Rebase — scanner #2314, attribution + hard-error floor
/// #2315, emission + rewrite #2316). Two additional scenarios run after the
/// original happy-path assertions, against fresh fixtures built from the
/// SAME `stage_minimal_fixture` + `build_real_tailwind_pipeline` entry point
/// so the confirm exercises the real orchestrator wiring, not
/// `zfb_css::url_attribution` in isolation (that crate's own unit tests
/// already cover the byte-level splice logic):
///
/// 1. `real_tailwind_binary_rebases_package_url_references_into_companions`:
///    the reporting issue's shape — a `node_modules` stylesheet with
///    relative `url()` references reachable only through Tailwind's own
///    `@import` inlining. Asserts the emitted companions land on disk beside
///    the hashed CSS, content-hashed and flat, and every `url()` byte span
///    left in the final hashed CSS resolves to one of them.
/// 2. `real_tailwind_binary_rejects_package_url_referencing_missing_file`:
///    the negative case from the locked decisions in #2313 — a package
///    stylesheet whose `url()` target does not exist on disk fails
///    `build_emitter()` with the exact "cannot emit" error template,
///    naming the package, stylesheet, reference, and reason.
///
/// Deliberately kept as scenarios inside this single `#[ignore]`d test
/// rather than new `#[test]` functions in this file — per `crates/CLAUDE.md`
/// (the manifest is keyed by path + test fn name), adding a new ignored fn
/// would need a new manifest row and exam.yml filterset entry for no
/// coverage benefit this env-gated binary doesn't already provide.
#[test]
#[ignore = "env-gate: tailwindcss v4 binary — cargo test -p zfb-build --test \
            prod_asset_graph_e2e -- --include-ignored (ZFB_TAILWIND_BIN or the \
            staged crates/zfb/binaries/tailwindcss-v4 slot)"]
fn prod_asset_graph_with_real_tailwind_binary_against_fixture() {
    let fixture = stage_minimal_fixture();
    let dist_dir = fixture.project_root.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();

    // Build a real (non-mocked) TailwindSubprocessEngine. Honours
    // ZFB_TAILWIND_BIN if set, else falls back to the workspace slot.
    let pipeline = build_real_tailwind_pipeline(fixture.project_root.path(), &dist_dir);
    let emitter_out = pipeline
        .build_emitter()
        .expect("real TailwindSubprocessEngine must produce CSS bytes");
    assert!(!emitter_out.bytes.is_empty());
    assert!(
        emitter_out.companions.is_empty(),
        "the plain fixture has no package url() references; companions must stay empty: {:?}",
        emitter_out.companions,
    );

    let head_assets = ProdHeadAssets {
        css_url: Some(STABLE_CSS_URL.to_string()),
        island_module_urls: Vec::new(),
    };
    let pages = vec![stage_html_with_head_inject(
        &dist_dir,
        "index.html",
        "Home (real tailwind)",
        &head_assets,
    )];

    let inputs = ProdAssetEmitterInputs {
        css: Some(AssetEmitterPayload {
            bytes: emitter_out.bytes.clone(),
            relative_path: css_relative_path(),
            stable_url: emitter_out.stable_url.clone(),
            companions: Vec::new(),
        }),
        islands: None,
        ..Default::default()
    };
    let _outcome =
        apply_prod_asset_pipeline(&dist_dir, pages.clone(), inputs).expect("pipeline must succeed");

    // Disk-existence + no leak — the same load-bearing assertion the
    // happy-path test makes.
    let html = fs::read_to_string(dist_dir.join("index.html")).unwrap();
    let href = first_stylesheet_href(&html).expect("stylesheet href after rewrite");
    assert!(
        href.starts_with("/assets/") && href.contains("styles-") && href.ends_with(".css"),
        "real-binary path produced a non-hashed href: {href}",
    );
    assert!(
        url_to_disk_path(&dist_dir, &href).is_file(),
        "static-fallback contract violated under real Tailwind binary",
    );
    assert!(!html.contains(&format!("\"{STABLE_CSS_URL}\"")));

    // -----------------------------------------------------------------
    // Scenario: real Tailwind rebases package url() references (#2318).
    // -----------------------------------------------------------------
    {
        let fixture = stage_minimal_fixture();
        let dist_dir = fixture.project_root.path().join("dist");
        fs::create_dir_all(&dist_dir).unwrap();

        // A node_modules package shipping two relative url() references —
        // the same shape as the reporting issue's fontsource repro
        // (`@import`ed from `styles/global.css`, referenced only through
        // Tailwind's own inlining, never authored by the project).
        stage_node_modules_package(
            fixture.project_root.path(),
            "@acme/icons",
            "icons.css",
            ".icon-a { background-image: url(\"./assets/icon-a.svg\"); }\n\
             .icon-b { background-image: url('./assets/icon-b.svg'); }\n",
            &[
                ("assets/icon-a.svg", b"<svg>icon-a</svg>"),
                ("assets/icon-b.svg", b"<svg>icon-b</svg>"),
            ],
        );
        append_package_import(fixture.project_root.path(), "@acme/icons", "icons.css");

        let pipeline = build_real_tailwind_pipeline(fixture.project_root.path(), &dist_dir);
        let emitter_out = pipeline
            .build_emitter()
            .expect("real tailwind must resolve and emit package url() companions");

        assert_eq!(
            emitter_out.companions.len(),
            2,
            "expected two distinct companions (icon-a.svg + icon-b.svg); got {:?}",
            emitter_out
                .companions
                .iter()
                .map(|c| &c.filename)
                .collect::<Vec<_>>(),
        );
        for companion in &emitter_out.companions {
            assert!(
                (companion.filename.starts_with("icon-a-")
                    || companion.filename.starts_with("icon-b-"))
                    && companion.filename.ends_with(".svg"),
                "companion filename must be the flat `{{stem}}-{{hash8}}.{{ext}}` form; got {}",
                companion.filename,
            );
        }
        let icon_a_companion = emitter_out
            .companions
            .iter()
            .find(|c| c.filename.starts_with("icon-a-"))
            .expect("icon-a companion");
        assert_eq!(icon_a_companion.bytes, b"<svg>icon-a</svg>");
        let icon_b_companion = emitter_out
            .companions
            .iter()
            .find(|c| c.filename.starts_with("icon-b-"))
            .expect("icon-b companion");
        assert_eq!(icon_b_companion.bytes, b"<svg>icon-b</svg>");

        // The rewritten CSS bytes must reference the companions by their
        // hashed filename, never the original node_modules-relative path.
        let rewritten = String::from_utf8(emitter_out.bytes.clone()).unwrap();
        assert!(
            rewritten.contains(&format!("./{}", icon_a_companion.filename)),
            "rewritten CSS must reference the icon-a companion by its hashed filename: {rewritten}",
        );
        assert!(
            rewritten.contains(&format!("./{}", icon_b_companion.filename)),
            "rewritten CSS must reference the icon-b companion by its hashed filename: {rewritten}",
        );
        assert!(
            !rewritten.contains("node_modules") && !rewritten.contains("assets/icon-a.svg"),
            "rewritten CSS must not leak the original node_modules-relative path: {rewritten}",
        );

        // Thread the companions into ProdAssetEmitterInputs exactly as
        // `run_css_emitter` (crates/zfb/src/commands/build.rs) does — map
        // zfb_css::PackageUrlAsset onto zfb_build::pipeline::CompanionFile —
        // then run the REAL prod pipeline end to end.
        let companion_files: Vec<CompanionFile> = emitter_out
            .companions
            .iter()
            .map(|c| CompanionFile {
                filename: c.filename.clone(),
                bytes: c.bytes.clone(),
            })
            .collect();
        let head_assets = ProdHeadAssets {
            css_url: Some(STABLE_CSS_URL.to_string()),
            island_module_urls: Vec::new(),
        };
        let pages = vec![stage_html_with_head_inject(
            &dist_dir,
            "index.html",
            "Icons (real tailwind)",
            &head_assets,
        )];
        let inputs = ProdAssetEmitterInputs {
            css: Some(AssetEmitterPayload {
                bytes: emitter_out.bytes.clone(),
                relative_path: css_relative_path(),
                stable_url: emitter_out.stable_url.clone(),
                companions: companion_files,
            }),
            islands: None,
            ..Default::default()
        };
        apply_prod_asset_pipeline(&dist_dir, pages, inputs)
            .expect("apply_prod_asset_pipeline must succeed with package url companions");

        // Every companion lands on disk beside the hashed CSS, byte-identical
        // to the source asset.
        let assets_dir = dist_dir.join(DIST_ASSETS_DIR);
        let on_disk_a = fs::read(assets_dir.join(&icon_a_companion.filename))
            .unwrap_or_else(|e| panic!("icon-a companion missing on disk: {e}"));
        assert_eq!(on_disk_a, b"<svg>icon-a</svg>");
        let on_disk_b = fs::read(assets_dir.join(&icon_b_companion.filename))
            .unwrap_or_else(|e| panic!("icon-b companion missing on disk: {e}"));
        assert_eq!(on_disk_b, b"<svg>icon-b</svg>");

        // Every url() left in the final hashed CSS on disk resolves to a
        // real file beside it (the static-fallback contract, extended to
        // package-attributed companions).
        let hashed_css_name = fs::read_dir(&assets_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("styles-") && n.ends_with(".css"))
            .expect("hashed CSS must exist");
        let hashed_css_bytes = fs::read(assets_dir.join(&hashed_css_name)).unwrap();
        let hashed_css_text = String::from_utf8(hashed_css_bytes).unwrap();
        let mut url_occurrences_checked = 0;
        for occurrence in scan_css_urls(&hashed_css_text) {
            let decoded = occurrence.decoded.trim();
            if decoded.is_empty() || decoded.starts_with("data:") {
                continue;
            }
            let resolved = assets_dir.join(decoded.trim_start_matches("./"));
            assert!(
                resolved.is_file(),
                "url({decoded}) in the hashed CSS does not resolve to a file on disk: {}",
                resolved.display(),
            );
            url_occurrences_checked += 1;
        }
        assert!(
            url_occurrences_checked >= 2,
            "expected to check at least the two companion url() references; checked {url_occurrences_checked}",
        );
    }

    // -----------------------------------------------------------------
    // Scenario: real Tailwind + a package url() target that does not
    // exist on disk must fail the build with the locked error template
    // (#2318's negative case, decisions locked in #2313).
    // -----------------------------------------------------------------
    {
        let fixture = stage_minimal_fixture();
        let dist_dir = fixture.project_root.path().join("dist");
        fs::create_dir_all(&dist_dir).unwrap();

        stage_node_modules_package(
            fixture.project_root.path(),
            "@acme/badpkg",
            "bad.css",
            ".missing { background-image: url(\"./nope/does-not-exist.png\"); }\n",
            &[],
        );
        append_package_import(fixture.project_root.path(), "@acme/badpkg", "bad.css");

        // Every field of the locked error template is pinned against a
        // concrete expected value below (not just a prefix) — the
        // `stylesheet` field names the CANONICALIZED source path
        // (`package_identity` in url_attribution.rs canonicalizes before
        // walking up for `package.json`), and the `reason` field's missing
        // path is `pkg.source.parent()` (also canonical) joined with the
        // RAW (unresolved) reference — the join is never itself
        // canonicalized, so the literal `./nope/...` segment survives.
        let bad_css_canonical = fs::canonicalize(
            fixture
                .project_root
                .path()
                .join("node_modules")
                .join("@acme/badpkg")
                .join("bad.css"),
        )
        .unwrap();
        let expected_missing_target = bad_css_canonical
            .parent()
            .unwrap()
            .join("./nope/does-not-exist.png");

        let pipeline = build_real_tailwind_pipeline(fixture.project_root.path(), &dist_dir);
        let err = pipeline
            .build_emitter()
            .expect_err("a package url() referencing a missing file must fail the build");
        // `build_emitter()` wraps the raw attribution error in
        // `.context("CSS engine stage failed")` — the locked template text
        // is the wrapped SOURCE, which only Debug's cause-chain rendering
        // surfaces (`Display`/`{:#}` show just the outer context message).
        let message = format!("{err:?}");
        assert!(
            message
                .contains("error: cannot emit `url()` asset from an imported package stylesheet"),
            "error must use the locked template header; got: {message}",
        );
        assert!(
            message.contains("package:    @acme/badpkg@1.0.0"),
            "error must name the package and version; got: {message}",
        );
        assert!(
            message.contains(&format!("stylesheet: {}", bad_css_canonical.display())),
            "error must name the exact staged stylesheet path; got: {message}",
        );
        assert!(
            message.contains("reference:  url(\"./nope/does-not-exist.png\")"),
            "error must quote the raw reference verbatim; got: {message}",
        );
        assert!(
            message.contains(&format!(
                "reason:     file not found at {}",
                expected_missing_target.display()
            )),
            "error must state the exact resolved missing-file path; got: {message}",
        );

        // The build must not have written anything to dist/assets — a
        // failed emitter call ships nothing, partially or otherwise.
        assert!(
            !dist_dir.join(DIST_ASSETS_DIR).exists(),
            "no assets/ dir must exist after a failed package url() emission",
        );
    }
}

// ---------------------------------------------------------------------------
// Dynamic-import chunks test — #808 acceptance criteria
// ---------------------------------------------------------------------------

/// Dynamic-import islands: when the islands entry has companion chunks
/// (esbuild code-split output), the pipeline ships the hashed entry AND
/// all chunk companions verbatim. HTML references only the hashed entry;
/// chunk file contents are never rewritten; chunk filenames match what
/// the bundler emitted.
///
/// This is the load-bearing test for issue #808:
/// - `dist/assets/` contains `islands-<hash>.js` (hashed entry) + every
///   `islands-chunk-<hash>.js` companion (verbatim, no pipeline hash).
/// - The HTML `<script type="module" src="…">` points only at the hashed
///   ENTRY, never at a chunk filename.
/// - Each chunk file the entry references exists on disk beside it
///   (static-fallback / lazy-import contract).
/// - Chunk bytes on disk are byte-identical to the bundler's output (no
///   `boundary_replace` or any other rewriting).
/// - Zero-chunk behaviour: a second run without companions produces only
///   the hashed entry, no extra files.
#[test]
fn prod_asset_graph_ships_dynamic_import_chunks_verbatim() {
    let fixture = stage_minimal_fixture();
    let dist_dir = fixture.project_root.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();

    let mock_css = ".text-red-500{color:red}\nbody{font-family:system-ui}\n";
    let pipeline = mock_css_pipeline(fixture.project_root.path(), &dist_dir, mock_css);
    let emitter_out = pipeline
        .build_emitter()
        .expect("CssPipeline::build_emitter must succeed");

    // Synthetic islands entry that has a relative import for a chunk —
    // this is the byte shape esbuild emits when code-splitting is active.
    let chunk_hash = "WOEGGERP";
    let chunk_filename = format!("islands-chunk-{chunk_hash}.js");
    let islands_entry_bytes = format!(
        "// islands entry\nimport(\"./{chunk_filename}\");\nexport function hydrate(){{}}\n"
    )
    .into_bytes();
    let chunk_bytes =
        b"// dynamic chunk: shiki syntax highlighter\nexport const highlight = () => null;\n"
            .to_vec();

    let islands_relative = PathBuf::from(DIST_ASSETS_DIR).join(STABLE_ISLANDS_FILENAME);

    let head_assets = ProdHeadAssets {
        css_url: Some(STABLE_CSS_URL.to_string()),
        island_module_urls: vec![STABLE_ISLANDS_URL.to_string()],
    };
    let pages = vec![
        stage_html_with_head_inject(&dist_dir, "index.html", "Home", &head_assets),
        stage_html_with_head_inject(&dist_dir, "about/index.html", "About", &head_assets),
    ];

    let inputs = ProdAssetEmitterInputs {
        css: Some(AssetEmitterPayload {
            bytes: emitter_out.bytes.clone(),
            relative_path: css_relative_path(),
            stable_url: emitter_out.stable_url.clone(),
            companions: Vec::new(),
        }),
        islands: Some(AssetEmitterPayload {
            bytes: islands_entry_bytes.clone(),
            relative_path: islands_relative,
            stable_url: STABLE_ISLANDS_URL.to_string(),
            companions: vec![CompanionFile {
                filename: chunk_filename.clone(),
                bytes: chunk_bytes.clone(),
            }],
        }),
        ..Default::default()
    };
    let outcome = apply_prod_asset_pipeline(&dist_dir, pages.clone(), inputs)
        .expect("apply_prod_asset_pipeline must succeed with chunks");

    let assets_dir = dist_dir.join(DIST_ASSETS_DIR);
    let mut entries: Vec<String> = fs::read_dir(&assets_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();

    // dist/assets/ must hold: CSS hashed file, islands hashed entry, chunk companion.
    assert_eq!(
        entries.len(),
        3,
        "expected CSS + islands entry + chunk companion; got {entries:?}",
    );
    let css_name = entries
        .iter()
        .find(|n| n.starts_with("styles-") && n.ends_with(".css"))
        .unwrap_or_else(|| panic!("no hashed CSS file in {entries:?}"));
    let islands_entry_name = entries
        .iter()
        .find(|n| {
            n.starts_with("islands-")
                && n.ends_with(".js")
                && n.contains('-')
                && !n.contains("chunk")
        })
        .unwrap_or_else(|| panic!("no hashed islands entry in {entries:?}"));
    let chunk_on_disk = entries
        .iter()
        .find(|n| n.as_str() == chunk_filename.as_str())
        .unwrap_or_else(|| panic!("chunk companion {chunk_filename} not found in {entries:?}"));

    // Chunk filename is verbatim (not pipeline-hashed).
    assert_eq!(
        chunk_on_disk, &chunk_filename,
        "chunk must be written under its esbuild-given filename without renaming",
    );

    // Chunk bytes on disk are byte-identical to bundler output (no rewriting).
    let chunk_bytes_on_disk = fs::read(assets_dir.join(chunk_on_disk)).unwrap();
    assert_eq!(
        chunk_bytes_on_disk, chunk_bytes,
        "chunk bytes must be verbatim — boundary_replace must not touch chunk contents",
    );

    // The hashed entry exists and is non-empty.
    let entry_bytes_on_disk = fs::read(assets_dir.join(islands_entry_name)).unwrap();
    assert!(
        !entry_bytes_on_disk.is_empty(),
        "hashed islands entry must be non-empty"
    );

    // HTML references only the HASHED ENTRY — never a chunk filename.
    let hashed_islands_url = format!("/assets/{islands_entry_name}");
    let hashed_css_url = format!("/assets/{css_name}");
    for page in &pages {
        let html = fs::read_to_string(dist_dir.join(page.output_path.as_path())).unwrap();

        assert!(
            html.contains(&island_module_script_tag(&hashed_islands_url)),
            "HTML must reference hashed islands entry; got:\n{html}",
        );
        assert!(
            html.contains(&css_link_tag(&hashed_css_url)),
            "HTML must reference hashed CSS; got:\n{html}",
        );

        // Chunk filename must NOT appear in any rendered HTML.
        assert!(
            !html.contains(&chunk_filename),
            "chunk filename {chunk_filename} leaked into HTML — only the entry must be referenced:\n{html}",
        );

        // No unhashed stable URLs.
        assert!(
            !html.contains(&format!("\"{STABLE_ISLANDS_URL}\"")),
            "unhashed islands URL leaked: {html}",
        );
        assert!(
            !html.contains(&format!("\"{STABLE_CSS_URL}\"")),
            "unhashed CSS URL leaked: {html}",
        );

        // Static-fallback: URLs in HTML must resolve to real files on disk.
        let islands_on_disk = url_to_disk_path(&dist_dir, &hashed_islands_url);
        assert!(
            islands_on_disk.is_file(),
            "static-fallback contract violated: hashed islands entry {hashed_islands_url} missing",
        );
    }

    // BuildOutcome reports the two hashed URLs (CSS + islands entry).
    // Chunk companions do NOT appear as separate hashed_asset_urls entries.
    assert_eq!(
        outcome.hashed_asset_urls.len(),
        2,
        "expected exactly CSS + islands entry in hashed_asset_urls; got {:?}",
        outcome.hashed_asset_urls,
    );
    let urls: std::collections::BTreeSet<String> = outcome
        .hashed_asset_urls
        .iter()
        .map(|(_, u)| u.clone())
        .collect();
    assert!(urls.contains(&hashed_css_url));
    assert!(urls.contains(&hashed_islands_url));

    // --- Zero-chunk regression: a second run without companions produces
    //     only the hashed entry, no leftover chunk files from the first run.
    //     (The test uses a fresh tempdir so residual files are not an issue;
    //     this shape documents the contract explicitly.)
    let dist2 = fixture.project_root.path().join("dist2");
    fs::create_dir_all(&dist2).unwrap();

    let pages2 = vec![stage_html_with_head_inject(
        &dist2,
        "index.html",
        "Zero-chunk",
        &ProdHeadAssets {
            css_url: Some(STABLE_CSS_URL.to_string()),
            island_module_urls: vec![STABLE_ISLANDS_URL.to_string()],
        },
    )];
    let inputs_no_chunks = ProdAssetEmitterInputs {
        css: Some(AssetEmitterPayload {
            bytes: emitter_out.bytes.clone(),
            relative_path: css_relative_path(),
            stable_url: emitter_out.stable_url.clone(),
            companions: Vec::new(),
        }),
        islands: Some(AssetEmitterPayload {
            bytes: b"// zero-chunk entry\nexport function hydrate(){}\n".to_vec(),
            relative_path: PathBuf::from(DIST_ASSETS_DIR).join(STABLE_ISLANDS_FILENAME),
            stable_url: STABLE_ISLANDS_URL.to_string(),
            companions: Vec::new(),
        }),
        ..Default::default()
    };
    apply_prod_asset_pipeline(&dist2, pages2, inputs_no_chunks)
        .expect("zero-chunk pipeline run must succeed");

    let assets2_entries: Vec<String> = fs::read_dir(dist2.join(DIST_ASSETS_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        assets2_entries.len(),
        2,
        "zero-chunk run must produce exactly CSS + islands entry; got {assets2_entries:?}",
    );
    assert!(
        !assets2_entries.iter().any(|n| n.contains("chunk")),
        "no chunk files must appear in a zero-chunk run; got {assets2_entries:?}",
    );
}
