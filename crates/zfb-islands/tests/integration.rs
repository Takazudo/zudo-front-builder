//! Integration-style tests for the public surface of `zfb-islands` (Sub 2).
//!
//! These exercise the bundler trait, the subprocess engine, and the URL
//! helper. Tests that need the real esbuild binary are gated by `#[ignore]`
//! and a comment — they will be enabled in a release-engineering follow-up
//! once the binary is materialised at `crates/zfb/binaries/esbuild` (Sub 2
//! reserves the slot but does not download the binary).

use std::path::{Path, PathBuf};

use zfb_islands::{
    link_href, BundleConfig, BundleOutput, ClientBundler, EsbuildSubprocessBundler,
    EsbuildSubprocessConfig, Island, NativeRustBundler,
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
    let bundle_cfg = BundleConfig {
        output_root: tmp.path().to_path_buf(),
        ..BundleConfig::default()
    };
    let out: BundleOutput = bundler
        .bundle(
            &[island("Counter", "components/counter.tsx")],
            &bundle_cfg,
        )
        .expect("mock bundler should succeed");
    assert_eq!(out.hash.len(), 8);
    assert!(
        out.asset_path.starts_with(tmp.path()),
        "asset must land under output_root: {}",
        out.asset_path.display()
    );
    assert!(out.asset_path.exists(), "asset must be written to disk");
    let on_disk = std::fs::read_to_string(&out.asset_path).expect("read asset");
    assert!(on_disk.contains("Counter"));
    assert_eq!(out.module_ids.get("Counter").map(String::as_str), Some("Counter"));
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
fn bundle_hash_is_deterministic_across_runs() {
    let payload = "export const X = 1;\n";

    let make = |root: &Path| {
        let cfg = EsbuildSubprocessConfig::default().with_mock_output(payload);
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let bundle_cfg = BundleConfig {
            output_root: root.to_path_buf(),
            ..BundleConfig::default()
        };
        bundler
            .bundle(&[island("X", "components/x.tsx")], &bundle_cfg)
            .expect("bundle")
    };

    let tmp1 = tempfile::tempdir().expect("tempdir");
    let tmp2 = tempfile::tempdir().expect("tempdir");
    let a = make(tmp1.path());
    let b = make(tmp2.path());
    assert_eq!(a.hash, b.hash, "identical payload must produce identical hash");
    assert_eq!(
        a.asset_path.file_name(),
        b.asset_path.file_name(),
        "filename suffix derives from hash"
    );
}

#[test]
fn bundle_hash_changes_when_payload_changes() {
    let make = |payload: &str, root: &Path| {
        let cfg = EsbuildSubprocessConfig::default().with_mock_output(payload);
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let bundle_cfg = BundleConfig {
            output_root: root.to_path_buf(),
            ..BundleConfig::default()
        };
        bundler
            .bundle(&[island("X", "components/x.tsx")], &bundle_cfg)
            .expect("bundle")
    };
    let tmp1 = tempfile::tempdir().expect("tempdir");
    let tmp2 = tempfile::tempdir().expect("tempdir");
    let a = make("export const X = 1;\n", tmp1.path());
    let b = make("export const X = 2;\n", tmp2.path());
    assert_ne!(a.hash, b.hash, "different payload must change the hash");
}

#[test]
fn bundle_output_layout_is_assets_islands_hash_js() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig {
        output_root: tmp.path().to_path_buf(),
        ..BundleConfig::default()
    };
    let out = bundler
        .bundle(&[island("X", "x.tsx")], &bundle_cfg)
        .expect("bundle");

    // Path layout: {output_root}/assets/islands-{hash}.js
    let expected = tmp
        .path()
        .join("assets")
        .join(format!("islands-{}.js", out.hash));
    assert_eq!(out.asset_path, expected);
}

#[test]
fn link_href_derives_public_url_from_asset_path() {
    let p = PathBuf::from("dist/assets/islands-abc12345.js");
    assert_eq!(link_href("/", &p), "/assets/islands-abc12345.js");
    assert_eq!(
        link_href("https://cdn.example.com", &p),
        "https://cdn.example.com/assets/islands-abc12345.js"
    );
}

#[test]
fn module_ids_map_is_populated_from_islands() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig {
        output_root: tmp.path().to_path_buf(),
        ..BundleConfig::default()
    };
    let out = bundler
        .bundle(
            &[
                island("Counter", "components/counter.tsx"),
                island("Tabs", "components/tabs.tsx"),
            ],
            &bundle_cfg,
        )
        .expect("bundle");
    assert_eq!(out.module_ids.len(), 2);
    assert!(out.module_ids.contains_key("Counter"));
    assert!(out.module_ids.contains_key("Tabs"));
}

#[test]
#[ignore = "Requires the real esbuild binary at crates/zfb/binaries/esbuild. \
            Will be enabled in a release-engineering follow-up."]
fn subprocess_bundler_against_real_binary() {
    let bundler = EsbuildSubprocessBundler::with_default_config();
    let tmp = tempfile::tempdir().expect("tempdir");

    // The caller would normally pass real island source paths here. For
    // the gated smoke test we ship a one-line ESM file in a temp dir.
    let entry = tmp.path().join("entry.js");
    std::fs::write(&entry, "export const Counter = () => null;\n").expect("write entry");

    let bundle_cfg = BundleConfig {
        output_root: tmp.path().to_path_buf(),
        ..BundleConfig::production()
    };
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
