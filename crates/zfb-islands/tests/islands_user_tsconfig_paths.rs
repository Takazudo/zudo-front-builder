//! Real-esbuild regression test for issue #1238.
//!
//! The islands esbuild bundler writes a synthetic `tsconfig.json` and passes
//! `--tsconfig=<synthetic>` to esbuild whenever a plugin has registered alias /
//! virtual-module path entries. An explicit `--tsconfig=` makes esbuild stop
//! walking up to the project's real `tsconfig.json`, so before the #1238 fix the
//! synthetic config carried ONLY the plugin entries (an empty `BTreeMap` + merged
//! plugin paths) — the user's own `compilerOptions.paths` (e.g. `@/*`) were
//! silently dropped. Any `"use client"` island importing through `@/` then failed
//! to bundle (`Could not resolve "@/..."`), no hydration `<script type="module">`
//! was emitted, and the page SSR'd but never hydrated.
//!
//! The fix seeds the synthetic `paths` map from the user's tsconfig
//! (`zfb_plugin_resolver::read_tsconfig_paths_into_map`) BEFORE merging the plugin
//! entries, at both bundler sites (`bundle_one_entry` and
//! `bundle_client_script_file`), via the shared `build_plugin_tsconfig` helper.
//!
//! ## RED-before-green (verified during implementation)
//!
//! On current `main` (synthetic tsconfig built from plugin entries only),
//! `island_bundle_resolves_user_tsconfig_path_with_plugin_present` and
//! `client_script_resolves_user_tsconfig_path_with_plugin_present` fail with the
//! exact #1238 error (`Could not resolve "@/marker"`); the fix makes them pass.
//!
//! ## Anti-false-positive design (the key subtlety)
//!
//! The plugin registration in these tests is a dummy `virtual:zfb-test` module
//! that is UNRELATED to the `@/marker` import. Its only job is to make
//! `resolver_inputs.paths_entries` non-empty so the synthetic-tsconfig branch
//! switches on. It deliberately does NOT register `@/marker` — if it did, the
//! plugin path (not the user's `tsconfig.paths`) would resolve the import and the
//! test could pass on current `main` for the wrong reason. The ONLY thing that may
//! resolve `@/marker` is the user's `tsconfig.json`.
//!
//! Esbuild gating mirrors the other islands integration tests: locate a binary via
//! `zfb_test_utils::locate_esbuild()` (env `ZFB_ESBUILD_BIN` → workspace
//! `crates/zfb/binaries/esbuild/` slot → `which esbuild`) and skip with a printed
//! note when none is present.

use std::fs;
use std::path::{Path, PathBuf};

use zfb_islands::{
    BundleConfig, ClientBundler, EsbuildSubprocessBundler, EsbuildSubprocessConfig, Island,
};
use zfb_test_utils::locate_esbuild;

/// A unique marker symbol exported from the `@/`-aliased target module. Asserting
/// it reaches the bundle proves the `@/marker` import resolved through the user's
/// tsconfig `paths`.
const MARKER: &str = "UserTsconfigPathMarker";

/// Lay down the shared fixture under `root`:
///
/// - `tsconfig.json` — the user's hand-written contract: `baseUrl: "."` +
///   `paths: { "@/*": ["src/*"] }`. This is what the bug dropped.
/// - `src/marker.tsx` — the `@/`-aliased target, exporting `MARKER`. No JSX in the
///   module itself, so it bundles on both the island and client-script paths.
fn write_user_tsconfig_fixture(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["src/*"] }
  }
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/marker.tsx"),
        format!("export default function {MARKER}() {{ return null; }}\n"),
    )
    .unwrap();
}

/// The external flags every fixture shares — same set the neighbouring
/// `exact_match_resolution.rs` uses so the preact runtime stays unbundled.
fn preact_externals() -> Vec<std::ffi::OsString> {
    vec![
        std::ffi::OsString::from("--external:preact"),
        std::ffi::OsString::from("--external:preact/*"),
        std::ffi::OsString::from("--external:@takazudo/*"),
    ]
}

/// **Site 1 (island bundle) — RED-before-green primary.**
///
/// User tsconfig declares `@/*: ["src/*"]`; the island imports `@/marker`; an
/// UNRELATED `virtual:zfb-test` plugin module is registered to switch on the
/// synthetic-tsconfig branch. With the fix the user's `@/` survives the explicit
/// `--tsconfig=` and the bundle resolves `@/marker`. On current `main` it fails
/// with `Could not resolve "@/marker"`.
#[test]
fn island_bundle_resolves_user_tsconfig_path_with_plugin_present() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[islands_user_tsconfig_paths] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_user_tsconfig_fixture(&root);

    fs::create_dir_all(root.join("components")).unwrap();
    let island_path = root.join("components/counter.tsx");
    fs::write(
        &island_path,
        "import Marker from \"@/marker\";\n\
         export const Counter = () => <Marker />;\n",
    )
    .unwrap();

    let cfg = EsbuildSubprocessConfig {
        extra_args: preact_externals(),
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root)
    // UNRELATED plugin registration — does NOT resolve `@/marker`. Its sole purpose
    // is to make `paths_entries` non-empty so the synthetic tsconfig is written.
    .with_virtual_modules(vec![(
        "virtual:zfb-test".to_string(),
        "export default function Unrelated() { return null; }\n".to_string(),
    )]);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));

    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect(
            "#1238: with a plugin registered, the synthetic tsconfig must still seed the \
             user's `@/*` path so `@/marker` resolves",
        );
    let body = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert!(
        body.contains(MARKER),
        "island bundle should contain the `@/`-aliased target's marker `{MARKER}` \
         (resolved via the user's tsconfig paths, not the plugin entry)."
    );
}

/// **Site 2 (client-script) — same guarantee on the other fixed site.**
///
/// `bundle_client_script_file` shares the `build_plugin_tsconfig` helper, so the
/// same user-tsconfig seeding must apply. Non-JSX entry to keep the client-script
/// path independent of any JSX configuration.
#[test]
fn client_script_resolves_user_tsconfig_path_with_plugin_present() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[islands_user_tsconfig_paths] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_user_tsconfig_fixture(&root);

    fs::create_dir_all(root.join("entry")).unwrap();
    let entry_path: PathBuf = root.join("entry/client.ts");
    fs::write(
        &entry_path,
        "import Marker from \"@/marker\";\n\
         export const used = Marker;\n",
    )
    .unwrap();

    let cfg = EsbuildSubprocessConfig {
        extra_args: preact_externals(),
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root)
    .with_virtual_modules(vec![(
        "virtual:zfb-test".to_string(),
        "export default function Unrelated() { return null; }\n".to_string(),
    )]);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));

    let js = bundler
        .bundle_client_script_file("client", &entry_path, &bundle_cfg)
        .expect(
            "#1238: the client-script synthetic tsconfig must also seed the user's `@/*` \
             path so `@/marker` resolves",
        );
    assert!(
        js.contains(MARKER),
        "client-script bundle should contain the `@/`-aliased target's marker `{MARKER}`."
    );
}

/// **No-plugin variant stays working (byte-identical path).**
///
/// With NO plugin registration, `paths_entries` is empty, so no synthetic tsconfig
/// is written and esbuild walks up to the real `tsconfig.json` exactly as before.
/// `@/marker` must still resolve. This pins that the fix does not regress the
/// no-plugin path. (The structural byte-identical guarantee is the helper's
/// `is_empty()` early return; this is the behavioural half.)
#[test]
fn island_bundle_resolves_user_tsconfig_path_without_plugin() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[islands_user_tsconfig_paths] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_user_tsconfig_fixture(&root);

    fs::create_dir_all(root.join("components")).unwrap();
    let island_path = root.join("components/counter.tsx");
    fs::write(
        &island_path,
        "import Marker from \"@/marker\";\n\
         export const Counter = () => <Marker />;\n",
    )
    .unwrap();

    // No `.with_virtual_modules` / `.with_alias_entries` — empty `paths_entries`,
    // so no `--tsconfig=` is emitted and esbuild discovers the real tsconfig.
    let cfg = EsbuildSubprocessConfig {
        extra_args: preact_externals(),
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));

    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect("no-plugin path: `@/marker` must resolve via esbuild's tsconfig walk-up");
    let body = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert!(
        body.contains(MARKER),
        "no-plugin island bundle should still resolve `@/marker` via tsconfig walk-up."
    );
}
