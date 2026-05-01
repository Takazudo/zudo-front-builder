//! Integration-style tests for the public surface of `zfb-islands` (Sub 2).
//!
//! These exercise the bundler trait, the subprocess engine, and the URL
//! helper. Tests that need the real esbuild binary are gated by `#[ignore]`
//! and a comment — they will be enabled in a release-engineering follow-up
//! once the binary is materialised at
//! `crates/zfb/binaries/esbuild/esbuild` (Sub 2 reserves the slot but does
//! not download the binary).

use std::path::{Path, PathBuf};

use zfb_islands::{
    bundle_link_href, manifest_json, scan_islands, BundleConfig, BundleOutput, ClientBundler,
    EsbuildSubprocessBundler, EsbuildSubprocessConfig, FsResolver, Island, Manifest,
    NativeRustBundler,
};

fn island(name: &str, path: &str) -> Island {
    Island {
        component_name: name.to_string(),
        source_path: PathBuf::from(path),
    }
}

#[test]
fn native_bundler_returns_not_implemented_error() {
    let bundler = NativeRustBundler::new();
    let err = bundler
        .bundle(
            &[island("Counter", "components/counter.tsx")],
            &BundleConfig::default(),
        )
        .expect_err("NativeRustBundler must return an error");
    let msg = format!("{err}");
    assert!(
        msg.contains("not yet implemented"),
        "expected 'not yet implemented' in error, got: {msg}"
    );
}

#[test]
fn subprocess_bundler_mock_short_circuits_command() {
    // Use the mock-output escape hatch so this test does not require the
    // esbuild binary to be present.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default()
        .with_mock_output("export const Counter = () => null;\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out: BundleOutput = bundler
        .bundle(
            &[island("Counter", "components/counter.tsx")],
            &bundle_cfg,
        )
        .expect("mock bundler should succeed");
    assert!(
        out.asset_path.starts_with(tmp.path()),
        "asset must land under outdir: {}",
        out.asset_path.display()
    );
    assert!(out.asset_path.exists(), "asset must be written to disk");
    let on_disk = std::fs::read_to_string(&out.asset_path).expect("read asset");
    assert!(on_disk.contains("Counter"));
    assert_eq!(out.module_ids, vec!["Counter".to_string()]);
    // S0 contract: stable filename + URL, no hash. Production hashing
    // happens in `ProductionAssetPipeline`.
    assert_eq!(out.asset_url, "/assets/islands.js");
    assert_eq!(out.asset_path, tmp.path().join("assets").join("islands.js"));
}

#[test]
fn subprocess_bundler_reports_missing_binary_clearly() {
    let cfg = EsbuildSubprocessConfig::default()
        .with_binary_path("/nonexistent/zfb-esbuild-please-do-not-create");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let err = bundler
        .bundle(&[], &BundleConfig::default())
        .expect_err("missing binary must error");
    let msg = format!("{err}");
    assert!(msg.contains("not found"), "got: {msg}");
}

#[test]
fn bundle_filename_is_stable_regardless_of_payload() {
    // Under the S0 contract the bundler emits the stable filename
    // `assets/islands.js` — the bytes vary with input, but the
    // filename and public URL never do. Hashing is the
    // `ProductionAssetPipeline`'s job.
    let make = |payload: &str, root: &Path| {
        let cfg = EsbuildSubprocessConfig::default().with_mock_output(payload);
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let bundle_cfg = BundleConfig::default().with_outdir(root);
        bundler
            .bundle(&[island("X", "components/x.tsx")], &bundle_cfg)
            .expect("bundle")
    };

    let tmp1 = tempfile::tempdir().expect("tempdir");
    let tmp2 = tempfile::tempdir().expect("tempdir");
    let a = make("export const X = 1;\n", tmp1.path());
    let b = make("export const X = 2;\n", tmp2.path());
    assert_eq!(a.asset_path.file_name(), b.asset_path.file_name());
    assert_eq!(a.asset_url, b.asset_url);
    assert_eq!(a.asset_url, "/assets/islands.js");

    // Bytes-on-disk still differ even though the names match.
    let bytes_a = std::fs::read(&a.asset_path).unwrap();
    let bytes_b = std::fs::read(&b.asset_path).unwrap();
    assert_ne!(bytes_a, bytes_b);
}

#[test]
fn bundle_output_layout_is_stable_assets_islands_js() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out = bundler
        .bundle(&[island("X", "x.tsx")], &bundle_cfg)
        .expect("bundle");

    // Path layout: {outdir}/assets/islands.js (stable; hashing is
    // performed downstream by `ProductionAssetPipeline`).
    let parent = out
        .asset_path
        .parent()
        .expect("asset path has a parent")
        .to_path_buf();
    assert_eq!(parent, tmp.path().join("assets"));
    let filename = out
        .asset_path
        .file_name()
        .expect("asset path has a filename")
        .to_string_lossy()
        .into_owned();
    assert_eq!(filename, "islands.js");
}

#[test]
fn bundle_link_href_derives_public_url_from_asset_path() {
    // The helper itself is content-agnostic — feed it the stable
    // asset path the bundler now emits.
    let p = PathBuf::from("dist/assets/islands.js");
    assert_eq!(bundle_link_href("/", &p), "/assets/islands.js");
    assert_eq!(
        bundle_link_href("https://cdn.example.com", &p),
        "https://cdn.example.com/assets/islands.js"
    );
}

#[test]
fn module_ids_list_preserves_island_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out = bundler
        .bundle(
            &[
                island("Counter", "components/counter.tsx"),
                island("Tabs", "components/tabs.tsx"),
            ],
            &bundle_cfg,
        )
        .expect("bundle");
    assert_eq!(out.module_ids, vec!["Counter".to_string(), "Tabs".to_string()]);
}

#[test]
fn asset_url_uses_configured_base_url() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default()
        .with_outdir(tmp.path())
        .with_base_url("https://cdn.example.com");
    let out = bundler
        .bundle(&[island("X", "x.tsx")], &bundle_cfg)
        .expect("bundle");
    // Stable filename, configurable base URL.
    assert_eq!(
        out.asset_url, "https://cdn.example.com/assets/islands.js",
        "got: {}",
        out.asset_url
    );
}

#[test]
#[ignore = "Requires the real esbuild binary at crates/zfb/binaries/esbuild/esbuild. \
            Will be enabled in a release-engineering follow-up."]
fn subprocess_bundler_against_real_binary() {
    let bundler = EsbuildSubprocessBundler::with_default_config();
    let tmp = tempfile::tempdir().expect("tempdir");

    // The caller would normally pass real island source paths here. For
    // the gated smoke test we ship a one-line ESM file in a temp dir.
    let entry = tmp.path().join("entry.js");
    std::fs::write(&entry, "export const Counter = () => null;\n").expect("write entry");

    let bundle_cfg = BundleConfig::production().with_outdir(tmp.path());
    let out = bundler
        .bundle(
            &[Island {
                component_name: "Counter".into(),
                source_path: entry,
            }],
            &bundle_cfg,
        )
        .expect("real esbuild binary should produce a bundle");
    assert!(out.asset_path.exists());
}

// -----------------------------------------------------------------------------
// Sub-task 3 — manifest emission (acceptance)
//
// The 2-island fixture under `fixtures/two-islands/` is the smallest realistic
// project we can scan: one page imports two distinct `"use client"` modules,
// each exporting a single component. The manifest must surface both with the
// resolved tsx paths.
// -----------------------------------------------------------------------------

fn fixture_root(name: &str) -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.join("fixtures").join(name)
}

/// Regression for issue #122 / #117 — pnpm-workspace consumer shape.
///
/// Reproduces the bug where production builds of a pnpm-workspace
/// consumer ship `data-zfb-island` markers but no client runtime: the
/// scanner used to short-circuit on bare specifiers and `FsResolver`
/// returned `None` for them, so workspace packages whose source `.tsx`
/// carried `"use client"` never made it into the islands set.
///
/// The fixture lays out a consumer with:
///
/// - `pages/home.tsx` importing `@takazudo/zfb-blog-islands` by its
///   scoped bare name.
/// - `workspace/zfb-blog-islands/package.json` with a
///   `"source": "src/index.tsx"` field (the convention pnpm-workspace
///   TypeScript packages use for un-built sources).
/// - `workspace/zfb-blog-islands/src/index.tsx` carrying
///   `"use client"` and exporting two components.
/// - `node_modules/@takazudo/zfb-blog-islands` is set up by the test
///   as a symlink to the workspace package — the codex-review fix on
///   PR #125 narrowed the bare-specifier probe to symlink-shaped
///   `node_modules/<pkg>` entries (i.e. workspace packages), so this
///   regression test mirrors what pnpm produces on disk.
///
/// `scan_islands` against this fixture must yield two islands —
/// `Counter` and `ThemeToggle` — each pointing at the workspace
/// package's source file.
#[cfg(unix)]
#[test]
fn pnpm_workspace_consumer_fixture_yields_workspace_package_islands() {
    let root = fixture_root("pnpm-workspace-consumer");
    // Set up the workspace symlink pnpm would maintain at install time.
    // We do this in the test (not as a checked-in symlink) because
    // checked-in symlinks travel poorly across OSes / git settings.
    let pkg = root.join("workspace/zfb-blog-islands");
    let scope_dir = root.join("node_modules/@takazudo");
    let pkg_link = scope_dir.join("zfb-blog-islands");
    std::fs::create_dir_all(&scope_dir).expect("create node_modules scope dir");
    // Leftover from a prior run? Remove and re-create.
    let _ = std::fs::remove_file(&pkg_link);
    let _ = std::fs::remove_dir_all(&pkg_link);
    std::os::unix::fs::symlink(&pkg, &pkg_link).expect("symlink workspace pkg into node_modules");

    let pages = vec![root.join("pages/home.tsx")];
    let resolver = FsResolver::new();
    let islands = scan_islands(&pages, &resolver).expect("scan");

    let names: Vec<String> = islands.iter().map(|i| i.component_name.clone()).collect();
    assert_eq!(
        names,
        vec!["Counter".to_string(), "ThemeToggle".to_string()],
        "expected exactly Counter + ThemeToggle from workspace package; got {islands:?}",
    );

    // Both islands must point at the same workspace-package source file
    // — proves the scanner walked node_modules and read package.json's
    // `source` field rather than picking up a stray copy somewhere else.
    let expected = pkg
        .join("src/index.tsx")
        .canonicalize()
        .expect("canonicalize workspace island source");
    for island in &islands {
        assert_eq!(island.source_path, expected, "got: {island:?}");
    }
}

#[test]
fn two_islands_fixture_yields_two_entry_manifest() {
    let root = fixture_root("two-islands");
    let pages = vec![root.join("pages/home.tsx")];
    let resolver = FsResolver::new();
    let islands = scan_islands(&pages, &resolver).expect("scan");
    assert_eq!(islands.len(), 2, "got: {islands:?}");

    // FsResolver canonicalises (e.g. `/var` → `/private/var` on macOS).
    // Use the canonical form of the fixture root so `relative_to` can
    // strip the prefix cleanly on every host OS.
    let root = root.canonicalize().expect("canonicalize fixture root");

    // Build the manifest, rebased to the fixture root so paths are
    // portable across machines.
    let manifest = Manifest::from_islands(&islands).relative_to(&root);
    let counter = manifest.get("Counter").expect("Counter present");
    let theme = manifest.get("ThemeToggle").expect("ThemeToggle present");
    assert_eq!(
        counter,
        Path::new("components/counter.tsx"),
        "counter path must be relative to fixture root"
    );
    assert_eq!(
        theme,
        Path::new("components/theme-toggle.tsx"),
        "theme-toggle path must be relative to fixture root"
    );
    // No collisions for distinct component names.
    assert!(manifest.collisions().is_empty());

    // Spot-check the JSON wire format. (Use the canonical root as well.)
    let json = manifest_json(&islands, Some(root.as_path()));
    assert!(json.contains("\"Counter\""));
    assert!(json.contains("\"ThemeToggle\""));
    assert!(json.contains("\"components/counter.tsx\""));
    assert!(json.contains("\"components/theme-toggle.tsx\""));
    // Counter sorts before ThemeToggle alphabetically.
    let cpos = json.find("\"Counter\"").unwrap();
    let tpos = json.find("\"ThemeToggle\"").unwrap();
    assert!(cpos < tpos, "json keys must be sorted: {json}");
}
