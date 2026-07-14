//! Level-3 bundle-output coverage for Wasm imports in the SSR bundle.
//!
//! The fixture stops after esbuild: Wave 2 owns making the embedded V8 host
//! execute these modules. This layer proves the deployable ESM output and its
//! copied asset manifest agree for both production bundle passes.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use zfb_build::{bundle, BundleMode, BundlerInput, BundlerOutput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

const WASM_BYTES: &[u8] = b"\0asm\x01\0\0\0";

fn scaffold_project(root: &Path, imports_wasm: bool) {
    for dir in ["pages", "content", "components", "layouts", "wasm"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    if imports_wasm {
        fs::write(root.join("wasm/answer.wasm"), WASM_BYTES).unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            r#"
                import wasmModule from "../wasm/answer.wasm";

                export default function Home() {
                  return wasmModule;
                }
            "#,
        )
        .unwrap();
    } else {
        fs::write(
            root.join("pages/index.tsx"),
            "export default function Home() { return \"no wasm\"; }\n",
        )
        .unwrap();
    }
}

fn make_input(root: &Path, esbuild: &Path, bundle_basename: Option<&str>) -> BundlerInput {
    let mut input = BundlerInput::for_project(
        root.to_path_buf(),
        Framework::Preact,
        BundleMode::Production,
        root.join("dist"),
        None,
    );
    // Keep the fixture hermetic: the synthetic SSR entry intentionally leaves
    // its runtime dependencies external, while the local `.wasm` import must
    // stay in esbuild's graph and use the copy loader.
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime/server".into(),
    ];
    input.esbuild_binary = Some(esbuild.to_path_buf());
    input.bundle_basename = bundle_basename.map(str::to_string);
    input
}

fn assert_wasm_bundle(out: &BundlerOutput) {
    assert_eq!(
        out.emitted_wasm_assets.len(),
        1,
        "one copied Wasm asset should be reported: {:?}",
        out.emitted_wasm_assets
    );
    let bundle_dir = out.bundle_path.parent().expect("bundle parent");
    let bundle = fs::read_to_string(&out.bundle_path).expect("read bundle");

    for asset in &out.emitted_wasm_assets {
        assert!(
            !asset.is_absolute(),
            "manifest asset must be relative: {asset:?}"
        );
        assert!(
            !asset
                .components()
                .any(|component| matches!(component, Component::ParentDir)),
            "manifest asset must not escape the bundle directory: {asset:?}"
        );

        let copied_asset = bundle_dir.join(asset);
        assert!(
            copied_asset.is_file(),
            "copied Wasm asset missing: {copied_asset:?}"
        );
        assert_eq!(fs::read(&copied_asset).unwrap(), WASM_BYTES);

        let import_specifier = format!("./{}", asset.to_string_lossy().replace('\\', "/"));
        assert!(
            bundle.lines().any(|line| {
                line.contains("import") && line.contains("from") && line.contains(&import_specifier)
            }),
            "bundle must retain a relative ESM Wasm import ({import_specifier}); bundle:\n{bundle}"
        );
    }
}

#[test]
fn copied_wasm_assets_are_manifested_for_both_production_bundle_passes() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_wasm_assets] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    scaffold_project(tmp.path(), true);

    let ssg = bundle(make_input(tmp.path(), &esbuild, None)).expect("SSG bundle should succeed");
    let mut runtime_input = make_input(tmp.path(), &esbuild, Some("bundle-runtime.mjs"));
    runtime_input.worker_only_routes = Some(BTreeSet::from(["/".to_string()]));
    let runtime = bundle(runtime_input).expect("runtime-only bundle should succeed");

    assert_eq!(ssg.bundle_path.file_name().unwrap(), "bundle.mjs");
    assert_eq!(
        runtime.bundle_path.file_name().unwrap(),
        "bundle-runtime.mjs"
    );
    assert_wasm_bundle(&ssg);
    assert_wasm_bundle(&runtime);
    assert_eq!(ssg.emitted_wasm_assets, runtime.emitted_wasm_assets);
}

#[test]
fn wasm_free_projects_report_an_empty_wasm_manifest() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_wasm_assets] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    scaffold_project(tmp.path(), false);

    let out =
        bundle(make_input(tmp.path(), &esbuild, None)).expect("Wasm-free bundle should succeed");
    assert!(out.emitted_wasm_assets.is_empty());
    assert!(out.bundle_path.is_file());
}

#[cfg(unix)]
fn write_metafile_wrapper(root: &Path, esbuild: &Path, name: &str, action: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let wrapper = root.join(format!("esbuild-{name}.sh"));
    let quoted_esbuild = format!(
        "'{}'",
        esbuild.display().to_string().replace('\'', "'\\\"'\\\"'")
    );
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n\
             {quoted_esbuild} \"$@\"\n\
             status=$?\n\
             [ \"$status\" -eq 0 ] || exit \"$status\"\n\
             for arg in \"$@\"; do\n\
               case \"$arg\" in\n\
                 --metafile=*)\n\
                   metafile=${{arg#--metafile=}}\n\
                   {action}\n\
                   ;;\n\
               esac\n\
             done\n\
             exit \"$status\"\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    wrapper
}

/// A real esbuild process emits the Wasm import and output first; the wrapper
/// only damages the metafile afterward. This proves the bundle path fails
/// closed instead of silently returning an empty deploy-asset manifest.
#[cfg(unix)]
#[test]
fn wasm_importing_bundle_rejects_malformed_or_missing_metafile() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_wasm_assets] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    scaffold_project(tmp.path(), true);

    for (name, action) in [
        ("malformed", "printf '%s' '{not json' > \"$metafile\""),
        ("missing", "rm -f \"$metafile\""),
    ] {
        let wrapper = write_metafile_wrapper(tmp.path(), &esbuild, name, action);
        let err = bundle(make_input(
            tmp.path(),
            &wrapper,
            Some(&format!("{name}.mjs")),
        ))
        .expect_err("Wasm-importing bundle must reject an unusable metafile");
        assert!(
            err.to_string().contains("wasm asset manifest"),
            "{name} metafile error should name the asset manifest: {err:#}"
        );
    }
}
