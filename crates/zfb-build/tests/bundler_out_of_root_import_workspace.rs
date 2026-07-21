//! L3 integration test for the workspace tier of issue #1839 (epic #1836,
//! from #1814): the sub-issue's diagnostic + documentation track for the
//! next.90 parent-escape tightening (behavior unchanged, owner decision on
//! the #1386 epic still holds — a relative import escaping the staged
//! build boundary stays unsupported, only the diagnostic gets clearer).
//!
//! `bundler_out_of_root_import.rs` covers the plain (non-workspace) case,
//! where `shadow == work_root`. This file covers the pnpm-workspace case,
//! where a claimed sibling package's own source is staged only under
//! `work_root` (the wholesale-mirrored workspace root), OUTSIDE the
//! narrower `shadow` (the project mirror nested under it — see
//! `run_esbuild`'s doc comment in `bundler.rs`). Before issue #1839's fix,
//! `friendly_esbuild_error`'s `Err(_)` fallback named that importer using
//! the raw `work_root` tempdir path (leaking the ephemeral spelling the
//! user never created) instead of mapping it back through
//! `first_party_root` to the real live-tree path.
//!
//! The fixture mirrors case (c) of
//! `bundler_sibling_mirror_esbuild_regression.rs` (a sibling reachable ONLY
//! through a wildcard tsconfig alias, `@shared/*` -> `lib/shared/*`), except
//! the sibling's own source has TWO relative imports that walk past the
//! WORKSPACE root itself (not just past the project root) — a real escape
//! beyond the `#1692` wholesale mirror. Two offenders exercise
//! `run_esbuild`'s `--log-limit=0` (uncapping esbuild's own 10-message
//! default) reporting both in one run instead of truncating.

use std::collections::BTreeMap;
use std::fs;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

#[test]
fn escaping_relative_import_from_workspace_sibling_gets_actionable_boundary_error() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_out_of_root_import_workspace] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    // outer/                         <- OUTSIDE the workspace entirely
    //   workspace/                   <- first_party_root (pnpm-workspace.yaml)
    //     pnpm-workspace.yaml
    //     sub-packages/host/         <- project_root passed to the bundler
    //       pages/index.tsx            imports "@shared/helper"
    //     lib/shared/
    //       helper.ts                  claimed ONLY via the "@shared/*"
    //                                   wildcard alias (never a relative
    //                                   import from the project); its own
    //                                   TWO relative imports climb past
    //                                   `workspace/` into `outer/`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let outer = tmp.path();
    let workspace = outer.join("workspace");
    let project = workspace.join("sub-packages/host");
    for d in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(project.join(d)).unwrap();
    }
    fs::write(
        workspace.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .unwrap();

    let shared_dir = workspace.join("lib/shared");
    fs::create_dir_all(&shared_dir).unwrap();
    fs::write(
        shared_dir.join("helper.ts"),
        r#"
            import one from "../../../outer-one.ts";
            import two from "../../../outer-two.ts";
            export const help = one + two;
        "#,
    )
    .unwrap();

    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { help } from "@shared/helper";
            export default function Home() {
              return help;
            }
        "#,
    )
    .unwrap();

    let mut input = BundlerInput::for_project(
        project.clone(),
        Framework::Preact,
        BundleMode::Production,
        project.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime/server".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.tsconfig_paths = BTreeMap::from([(
        "@shared/*".to_string(),
        vec![shared_dir.join("*").to_string_lossy().into_owned()],
    )]);

    let err = bundle(input).expect_err(
        "the workspace-sibling's own out-of-workspace relative imports must \
         still fail to bundle — owner decision on #1386: escaping relative \
         imports stay unsupported, only the diagnostic gets clearer",
    );
    let message = err.to_string();

    // 1. Names the sibling's REAL live-tree path (mapped through
    //    `first_party_root`), not the ephemeral `work_root` tempdir the
    //    wholesale mirror staged it under.
    let real_importer = shared_dir.join("helper.ts");
    assert!(
        message.contains(&real_importer.to_string_lossy().to_string()),
        "error should name the real workspace-sibling path {}: {message}",
        real_importer.display()
    );
    assert!(
        !message.contains("zfb-bundler-"),
        "error should not leak the work_root tempdir's own directory name: {message}"
    );

    // 2. States the shadow-copy project-root boundary rule.
    assert!(
        message.to_lowercase().contains("shadow-copy"),
        "error should explain the shadow-copy boundary: {message}"
    );

    // 3. Mentions the package-specifier + wildcard-exports workaround.
    assert!(
        message.contains("exports"),
        "error should mention the package.json `exports` workaround: {message}"
    );

    // 4. Both offenders are named — `run_esbuild`'s `--log-limit=0` must
    //    not let esbuild's default 10-message cap (irrelevant here at only
    //    2, but this is the behavior it protects) truncate either.
    assert!(
        message.contains("../../../outer-one.ts"),
        "the first escaping specifier must be named: {message}"
    );
    assert!(
        message.contains("../../../outer-two.ts"),
        "the second escaping specifier must ALSO be named: {message}"
    );

    drop(tmp);
}
