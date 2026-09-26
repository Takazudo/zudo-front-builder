//! Issue #3162 (epic #3160) — the dev SSR module-dependency set published to
//! `RawImportInvalidation` must name the ORIGINAL workspace-package files a
//! route imports, never the staged copies under `zfb-shadow-session-*`.
//!
//! A nested host (`sub-packages/host` inside a pnpm workspace) consumes a
//! first-party workspace package by name. The bundler stages that package as a
//! REAL copy inside the shadow session (#1901/#3161). A watcher registered on a
//! staged copy can never observe the source edit, so the metafile-derived dep
//! set must map staged copies back to their workspace source. Driven by the
//! real esbuild binary in `BundleMode::Development` (the only mode that parses
//! the metafile into `route_module_deps`).

use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{
    bundle_with_session, BundleMode, BundlerInput, RawImportInvalidation, ShadowSession,
};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

fn write_nested_host_workspace(ws_root: &Path) -> PathBuf {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n  - 'packages/*'\n",
    )
    .unwrap();
    let project = ws_root.join("sub-packages/host");
    for d in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(project.join(d)).unwrap();
    }
    fs::write(
        project.join("package.json"),
        r#"{"name":"host","dependencies":{"data":"workspace:*","lib":"workspace:*"}}"#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import value from "data/value.json";
            import { marker } from "lib/islands";
            export default function Home() { return value.label + marker; }
        "#,
    )
    .unwrap();

    let data = ws_root.join("packages/data");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("package.json"),
        r#"{"name":"data","exports":{"./*":"./*"}}"#,
    )
    .unwrap();
    fs::write(data.join("value.json"), r#"{"label":"DATA_LABEL"}"#).unwrap();

    let lib = ws_root.join("packages/lib");
    fs::create_dir_all(lib.join("dist")).unwrap();
    fs::write(
        lib.join("package.json"),
        r#"{"name":"lib","exports":{"./islands":"./dist/islands.js"}}"#,
    )
    .unwrap();
    fs::write(
        lib.join("dist/islands.js"),
        "export const marker = 'COMPILED_DIST_MARKER';\n",
    )
    .unwrap();

    fs::create_dir_all(ws_root.join("node_modules")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&data, ws_root.join("node_modules/data")).unwrap();
        std::os::unix::fs::symlink(&lib, ws_root.join("node_modules/lib")).unwrap();
    }
    project
}

fn dev_input(project: &Path, esbuild: PathBuf) -> BundlerInput {
    let mut input = BundlerInput::for_project(
        project.to_path_buf(),
        Framework::Preact,
        BundleMode::Development,
        project.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input
}

fn is_under_shadow_session(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name.starts_with("zfb-shadow-session-"))
    })
}

#[cfg(unix)]
#[test]
fn nested_host_publishes_original_workspace_package_files_not_staged_copies() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_ssr_module_deps_workspace_staging] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws_root = fs::canonicalize(tmp.path()).unwrap();
    let project = write_nested_host_workspace(&ws_root);

    let mut session = ShadowSession::new(&project).unwrap();
    let out = bundle_with_session(dev_input(&project, esbuild), Some(&mut session))
        .unwrap_or_else(|error| panic!("dev bundle must succeed: {error:#}"));
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(body.contains("DATA_LABEL"), "{body}");
    assert!(body.contains("COMPILED_DIST_MARKER"), "{body}");

    let deps: Vec<PathBuf> = out
        .route_module_deps
        .iter()
        .flat_map(|route| route.module_deps.iter().cloned())
        .collect();
    let staged: Vec<&PathBuf> = deps.iter().filter(|p| is_under_shadow_session(p)).collect();
    assert!(
        staged.is_empty(),
        "route module deps must never name a staged shadow copy: {staged:?} (all: {deps:?})"
    );

    let invalidation = RawImportInvalidation::default();
    invalidation.replace_ssr_module_deps(deps.clone());
    let published = invalidation.ssr_module_dep_paths();
    let original_json = ws_root.join("packages/data/value.json");
    let original_dist = ws_root.join("packages/lib/dist/islands.js");
    assert!(
        published.contains(&original_json),
        "published SSR deps must contain the original workspace JSON {}: {published:?}",
        original_json.display()
    );
    assert!(
        published.contains(&original_dist),
        "published SSR deps must contain the original workspace dist/ file {}: {published:?}",
        original_dist.display()
    );
    assert!(
        !published.iter().any(|p| is_under_shadow_session(p)),
        "no zfb-shadow-session-* path may be published: {published:?}"
    );
    assert!(invalidation.is_ssr_module_dependency(&original_json));
}
