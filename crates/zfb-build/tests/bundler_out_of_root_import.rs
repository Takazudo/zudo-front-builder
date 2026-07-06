//! L3 integration test for #1385 pt.2 / issue #1388.
//!
//! A relative import that lexically escapes the project root (the
//! pnpm-workspace monorepo shape from #1385: an app package whose page
//! walks OUT of its own project root into a sibling `packages/`-style
//! directory) fails to resolve under zfb's shadow-copy bundling. Owner
//! decision recorded on the #1386 epic: this is **diagnose-only** — the
//! import is NOT made to work, only explained. This test asserts the
//! failure names the REAL source path (not the ephemeral shadow tempdir),
//! states the shadow-copy project-root boundary rule, and mentions the
//! package-specifier + wildcard-`exports` workaround.

use std::fs;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

#[test]
fn escaping_relative_import_gets_actionable_boundary_error() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_out_of_root_import] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    // workspace/
    //   app/           <- project_root passed to the bundler
    //     pages/index.tsx  <- imports "../../outside/shared.ts"
    //   outside/
    //     shared.ts        <- OUTSIDE app/, unreachable through the shadow
    let workspace = tempfile::tempdir().expect("tempdir");
    let app_root = workspace.path().join("app");
    let outside_dir = workspace.path().join("outside");
    for d in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(app_root.join(d)).unwrap();
    }
    fs::create_dir_all(&outside_dir).unwrap();

    fs::write(
        outside_dir.join("shared.ts"),
        "export const label = \"shared\";\n",
    )
    .unwrap();

    fs::write(
        app_root.join("pages/index.tsx"),
        r#"
            import { label } from "../../outside/shared.ts";
            export default function Home() {
              return label;
            }
        "#,
    )
    .unwrap();

    let mut input = BundlerInput::for_project(
        app_root.clone(),
        Framework::Preact,
        BundleMode::Production,
        app_root.join("dist"),
        None,
    );
    // Keep the SSR entry's own bare imports external so this fixture does
    // not need a real node_modules tree (mirrors bundler_integration.rs).
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime/server".into(),
    ];
    input.esbuild_binary = Some(esbuild);

    let err = bundle(input).expect_err(
        "the escaping relative import must still fail to bundle — owner \
         decision on #1386: out-of-root relative imports stay unsupported, \
         only the diagnostic gets clearer",
    );
    let message = err.to_string();

    // 1. Names the REAL source path, not the ephemeral shadow tempdir.
    let real_importer = app_root.join("pages/index.tsx");
    assert!(
        message.contains(&real_importer.to_string_lossy().to_string()),
        "error should name the real project path {}: {message}",
        real_importer.display()
    );
    assert!(
        !message.contains("zfb-bundler-"),
        "error should not leak the shadow tempdir's own directory name: {message}"
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

    drop(workspace);
}
