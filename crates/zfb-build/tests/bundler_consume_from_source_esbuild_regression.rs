//! Issue #2039 (epic #1982, Wave 1) — RED tests for the "consume-from-source"
//! workspace idiom.
//!
//! Sourced from the real-world repro in #1730's second comment: a workspace
//! layout `packages: [".", "packages/*", "apps/*", "doc"]` where a first-party
//! sibling package (`@acme/ui`) is intentionally consumed from source — its
//! `package.json` `exports` map points straight at `./src/*`, with no
//! `dist`/build step. Every bare package-name import
//! (`import { ctaButton } from "@acme/ui/cta-button"`) resolves through the
//! `node_modules/@acme/ui` workspace symlink straight to `packages/ui/src/**`,
//! leaving no `node_modules` segment in the canonical path — exactly
//! `metafile_deps.rs`'s documented "case 2: OFFENDER" shape.
//!
//! **This file documents TODAY'S asymmetric behavior, not a settled policy.**
//! Per the epic's "Scope widened during planning verification" section,
//! consuming a first-party sibling from source is LEGITIMATE, not a stage
//! escape — the guard's current theory that case 2 is always an offender
//! assumes every workspace package ships a built `dist/`, which is not
//! universal. Wave 4 (#2040) redefines case 2 accordingly; Waves 5-6 (#1987,
//! #1988) then wire the audit at the root. Arming the audit at the root
//! WITHOUT first settling the case-2 policy would turn the currently-green
//! root build red — these two tests are what makes that regression visible.
//!
//! - [`nested_member_consume_from_source_sibling_hard_fails_as_case_two_offender_today`]
//!   is **current WRONG behavior to be flipped**: a perfectly ordinary
//!   pnpm-workspace idiom hard-fails today. #2040 must make this pass.
//! - [`root_project_consume_from_source_sibling_builds_clean_today_for_the_identical_shape`]
//!   is **current behavior that must be PRESERVED** (the build must still be
//!   green after #2040/#1987/#1988 land) — but today it is green only by
//!   accident, because `first_party_root == project_root` at the workspace
//!   root disables the widened-stage guard entirely (the #1730 blind spot),
//!   not because the policy has been decided. Do not read this test's
//!   present-day green result as proof the policy question is settled.
//!
//! Do NOT change the audit, the case-2 classification, or any policy here —
//! this file is tests only, per the epic's HARD STOP RULES.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle_with_session, BundleMode, BundlerInput, ShadowSession};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directory");
    }
    fs::write(path, contents).expect("write fixture file");
}

fn write_required_project_dirs(project: &Path) {
    for dir in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(project.join(dir)).expect("create required project directory");
    }
}

/// The real-world workspace shape from #1730's second comment:
/// `packages: [".", "packages/*", "apps/*", "doc"]`.
fn write_workspace_root(root: &Path) {
    write(
        &root.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'packages/*'\n  - 'apps/*'\n  - 'doc'\n",
    );
    write(
        &root.join("package.json"),
        r#"{ "name": "workspace-root", "private": true }"#,
    );
}

/// A first-party sibling package intentionally consumed from source — its
/// `exports` map points straight at `./src/*`, with no `dist`/build step. The
/// workspace-hoisted `node_modules/@acme/ui` symlink is what makes a bare
/// package-name import resolve straight into `packages/ui/src/**`.
fn write_consume_from_source_sibling(root: &Path) -> PathBuf {
    let ui = root.join("packages/ui");
    write(
        &ui.join("package.json"),
        r#"{
            "name": "@acme/ui",
            "exports": {
                "./cta-button": "./src/cta-button/cta-button.tsx",
                "./theme-state": "./src/theme-control/theme-state.ts"
            }
        }"#,
    );
    write(
        &ui.join("src/cta-button/cta-button.tsx"),
        r#"export const ctaButton = "CTA_BUTTON_SOURCE_MARKER";"#,
    );
    write(
        &ui.join("src/theme-control/theme-state.ts"),
        r#"export const themeState = "THEME_STATE_SOURCE_MARKER";"#,
    );

    let node_modules = root.join("node_modules");
    fs::create_dir_all(node_modules.join("@acme")).expect("create scoped node_modules directory");
    let link = node_modules.join("@acme/ui");
    if !link.exists() {
        std::os::unix::fs::symlink(&ui, &link)
            .expect("link consume-from-source sibling into hoisted install");
    }
    node_modules
}

fn write_nested_host(root: &Path) -> PathBuf {
    let project = root.join("apps/demo");
    write(
        &project.join("package.json"),
        r#"{ "name": "demo", "private": true }"#,
    );
    write_required_project_dirs(&project);
    project
}

fn write_consumer_entry(project: &Path) {
    write(
        &project.join("pages/index.tsx"),
        r#"
            import { ctaButton } from "@acme/ui/cta-button";
            import { themeState } from "@acme/ui/theme-state";
            export default function Home() {
              return "CONSUMER_MARKER:" + ctaButton + ":" + themeState;
            }
        "#,
    );
}

fn base_input(project: &Path, esbuild: PathBuf, node_modules: PathBuf) -> BundlerInput {
    let mut input = BundlerInput::for_project(
        project.to_path_buf(),
        Framework::Preact,
        BundleMode::Production,
        project.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(node_modules);
    input.tsconfig_paths = BTreeMap::new();
    input
}

fn bundle_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read emitted bundle")
}

/// **Current WRONG behavior — #2040 must flip this to pass.**
///
/// A nested workspace member (`apps/demo`) importing a consume-from-source
/// sibling by bare package name hard-fails the SSR work-mirror stage-escape
/// audit today, even though the resolved target is ordinary first-party
/// workspace source reached via a package name instead of a relative path.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_consume_from_source_esbuild_regression -- --ignored"]
fn nested_member_consume_from_source_sibling_hard_fails_as_case_two_offender_today() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_consume_from_source_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_workspace_root(root);
    write_required_project_dirs(root);
    let node_modules = write_consume_from_source_sibling(root);
    let project = write_nested_host(root);
    write_consumer_entry(&project);

    let error = bundle_with_session(
        base_input(&project, esbuild, node_modules),
        Some(&mut ShadowSession::new(&project).expect("shadow session")),
    )
    .expect_err(
        "today's guard hard-fails a nested member consuming a first-party \
         sibling from source by bare package name (case 2) — this is the \
         behavior #2040 must invert",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("stage-escape audit"),
        "expected a stage-escape audit failure; got: {message}"
    );
    assert!(
        message.contains("package import resolved outside node_modules to workspace sibling"),
        "expected the case-2 offender classification text; got: {message}"
    );
    let ui_src = root.join("packages/ui/src").to_string_lossy().into_owned();
    assert!(
        message.contains(&ui_src),
        "expected the live workspace-sibling source path to appear in the \
         offender message; got: {message}"
    );
    assert!(
        message.contains("node_modules/@acme/ui/src/cta-button/cta-button.tsx")
            && message.contains("node_modules/@acme/ui/src/theme-control/theme-state.ts"),
        "expected both staged package-name metafile keys in the offender \
         message; got: {message}"
    );
}

/// **Current behavior that must be PRESERVED** (the build must stay green
/// after #2040/#1987/#1988 land) — but today it is green ONLY because
/// `first_party_root == project_root` at the workspace root disables the
/// widened-stage guard entirely (the #1730 blind spot), not because the
/// case-2 policy has been decided. This documents the asymmetry explicitly:
/// the identical import shape that hard-fails for the nested member above
/// builds clean here, purely by omission of the guard rather than by design.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_consume_from_source_esbuild_regression -- --ignored"]
fn root_project_consume_from_source_sibling_builds_clean_today_for_the_identical_shape() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_consume_from_source_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_workspace_root(root);
    write_required_project_dirs(root);
    let node_modules = write_consume_from_source_sibling(root);
    // The project IS the workspace root itself (`.` in pnpm-workspace.yaml) —
    // `first_party_root_for(project_root) == project_root`, so `workspace_rel`
    // is `None` and the widened-stage guard never arms. Same bare-name import
    // of the same consume-from-source sibling as the nested test above.
    write_consumer_entry(root);

    let output = bundle_with_session(
        base_input(root, esbuild, node_modules),
        Some(&mut ShadowSession::new(root).expect("shadow session")),
    )
    .expect(
        "the workspace ROOT project builds clean for the identical \
         consume-from-source import shape that hard-fails when nested — \
         purely because the guard never arms at the root today (#1730)",
    );

    let body = bundle_text(&output.bundle_path);
    for marker in [
        "CONSUMER_MARKER",
        "CTA_BUTTON_SOURCE_MARKER",
        "THEME_STATE_SOURCE_MARKER",
    ] {
        assert!(
            body.contains(marker),
            "bundle must contain {marker}; got: {body}"
        );
    }
}
