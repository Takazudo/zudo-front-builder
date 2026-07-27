//! Issue #2039 (epic #1982, Wave 1) — regressions for the "consume-from-source"
//! workspace idiom. Wave 4 (#2040) flipped the first test from RED to green and
//! added the dist-shipping negative that keeps the new acceptance rule honest.
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
//! Wave 4 (#2040) settled the policy the Wave-1 header called open: consuming
//! a first-party sibling from source is LEGITIMATE, not a stage escape. The
//! old theory that case 2 is always an offender assumed every workspace
//! package ships a built `dist/`, which is not universal. Case 2 now accepts
//! an input the target package itself **declares** as an entry root (and that
//! `pnpm-workspace.yaml` claims as a package); everything else stays an
//! offender. Waves 5-6 (#1987, #1988) then wire the audit at the root — safe
//! only because this policy landed first.
//!
//! - [`nested_member_consume_from_source_sibling_is_accepted_as_declared_first_party_source`]
//!   was the Wave-1 RED test (then named
//!   `…_hard_fails_as_case_two_offender_today`), flipped by #2040: the
//!   ordinary pnpm-workspace idiom builds clean and emits the sibling's source.
//! - [`nested_member_undeclared_deep_import_into_dist_shipping_sibling_stays_rejected`]
//!   is #2040's own negative: acceptance keys on the declared-source
//!   condition, not on "workspace sibling" alone.
//! - [`root_project_consume_from_source_sibling_builds_clean_today_for_the_identical_shape`]
//!   is **current behavior that must be PRESERVED** (the build must still be
//!   green after #1987/#1988 land). Today it is green because
//!   `first_party_root == project_root` at the workspace root disables the
//!   widened-stage guard entirely (the #1730 blind spot); once the wiring
//!   waves arm the audit there, it must stay green on the merits — which is
//!   exactly what #2040 provides.
//!
//! Unix-only: the fixtures wire their workspace-hoisted `node_modules/@acme/*`
//! symlinks via `std::os::unix::fs::symlink`, matching the same `#[cfg(unix)]`
//! gate `sibling_css_module_command_layer_build.rs` and
//! `css_modules_components_build.rs` use for their own symlink fixtures.

#![cfg(unix)]

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

/// The counter-shape (#2040): a first-party sibling that DOES ship a built
/// `dist` and declares only `./dist/index.js`. It carries live `src/` source
/// beside that dist, reachable only by a deep import the package never
/// declares — the shape acceptance must keep rejecting.
fn write_dist_shipping_sibling(root: &Path) -> PathBuf {
    let built = root.join("packages/built");
    write(
        &built.join("package.json"),
        r#"{ "name": "@acme/built", "main": "./dist/index.js" }"#,
    );
    write(
        &built.join("dist/index.js"),
        r#"export const built = "BUILT_DIST_MARKER";"#,
    );
    write(
        &built.join("src/internal.ts"),
        r#"export const internal = "UNDECLARED_SOURCE_MARKER";"#,
    );

    let node_modules = root.join("node_modules");
    fs::create_dir_all(node_modules.join("@acme")).expect("create scoped node_modules directory");
    let link = node_modules.join("@acme/built");
    if !link.exists() {
        std::os::unix::fs::symlink(&built, &link)
            .expect("link dist-shipping sibling into hoisted install");
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

/// Same as [`base_input`], but with a non-empty `tsconfig_paths` — the second
/// half of `esbuild_will_preserve_symlinks`'s (bundler.rs :10153) three-way
/// test. With `node_modules_dir: Some(_)` (already set by `base_input`) and
/// `node_modules_preserve_symlinks` left at its `false` default, a non-empty
/// `tsconfig_paths` flips that predicate to `false`, so `bundle`
/// (bundler.rs :2019) sets `copy_mode = true` and drives esbuild WITHOUT
/// `--preserve-symlinks`. The alias itself is inert — it targets a real
/// path already inside `project_root` (`components/`, created by
/// `write_required_project_dirs`), which `collect_runtime_alias_claim_graph`
/// (bundler.rs :4870-4875) skips (`file.starts_with(project_root)`) even in
/// the nested-member topology where `first_party_root != project_root`. Its
/// only job is making `tsconfig_paths` non-empty.
fn base_input_copy_mode(project: &Path, esbuild: PathBuf, node_modules: PathBuf) -> BundlerInput {
    let mut input = base_input(project, esbuild, node_modules);
    input.tsconfig_paths = BTreeMap::from([(
        "#components/*".to_string(),
        vec![project
            .join("components")
            .join("*")
            .to_string_lossy()
            .into_owned()],
    )]);
    input
}

fn bundle_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read emitted bundle")
}

/// **Flipped by #2040 (Wave 4).** Before the fix this hard-failed the SSR
/// work-mirror stage-escape audit with the case-2 offender classification
/// (`package import resolved outside node_modules to workspace sibling …`) —
/// see this file's header and #2039 for the original assertion.
///
/// A nested workspace member (`apps/demo`) importing a consume-from-source
/// sibling by bare package name now builds clean: `@acme/ui` declares those
/// exact files as its entry points, is claimed by `pnpm-workspace.yaml`, and
/// its resolved target is ordinary first-party workspace source reached via a
/// package name instead of a relative path — not something staging was meant
/// to isolate. The emitted markers prove the sibling's source really was
/// bundled, not merely tolerated.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_consume_from_source_esbuild_regression -- --ignored"]
fn nested_member_consume_from_source_sibling_is_accepted_as_declared_first_party_source() {
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

    let output = bundle_with_session(
        base_input(&project, esbuild, node_modules),
        Some(&mut ShadowSession::new(&project).expect("shadow session")),
    )
    .expect(
        "a nested member consuming a first-party sibling from source by bare \
         package name is a legitimate monorepo idiom, not a stage escape (#2040)",
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

/// **Flipped by #2086 (Staging Correctness 2 epic #2078, Wave 3; #2082's Part
/// 1, epic Wave 1, was the RED author).** Identical topology to
/// [`nested_member_consume_from_source_sibling_is_accepted_as_declared_first_party_source`]
/// above, but under **copy_mode**
/// (`node_modules_dir` set + non-empty `tsconfig_paths`, `node_modules_preserve_symlinks`
/// left `false` — `esbuild_will_preserve_symlinks`, bundler.rs :10153/:2019).
///
/// Without `--preserve-symlinks`, esbuild canonicalises the
/// `node_modules/@acme/ui` symlink back to its real target and records a
/// CANONICALIZED metafile key for the resolved import (a `..`-climbing
/// relative path such as `../../packages/ui/src/cta-button/cta-button.tsx`),
/// not a `node_modules/...`-shaped one. `package_shaped`
/// (`metafile_deps.rs`'s `has_node_modules_segment` check on the KEY string,
/// not the canonical path) is `false` for such a key, so it never enters the
/// case-2 branch where `package_input_is_declared_first_party_entry` (the
/// #2040 exemption) is consulted at all — it used to fall through to the
/// case-1/case-4 stage-membership check instead, which rejected it as case 4
/// ("first-party input resolved outside every stage root, no staged
/// spelling") even though `@acme/ui` is declared, claimed, and its entry
/// covers the import — the SAME package the sibling test above already
/// accepts when esbuild happens to preserve symlinks.
/// `canonical_input_is_declared_first_party_entry` (`metafile_deps.rs`) now
/// resolves package identity from the canonical path instead, so the build
/// stays GREEN, identically to the `--preserve-symlinks` case.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_consume_from_source_esbuild_regression -- --ignored"]
fn nested_member_consume_from_source_sibling_accepted_via_canonicalized_key_under_copy_mode() {
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

    let output = bundle_with_session(
        base_input_copy_mode(&project, esbuild, node_modules),
        Some(&mut ShadowSession::new(&project).expect("shadow session")),
    )
    .expect(
        "a nested member consuming a first-party sibling from source by bare package \
         name is legitimate regardless of whether esbuild records a node_modules-shaped \
         or a canonicalized metafile key for the resolved import (#2047/#2086)",
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

/// **The guard that stops #2040 from becoming a blanket exemption.**
///
/// A sibling package that DOES ship a built `dist` and declares only
/// `./dist/index.js` is still a workspace sibling reached by bare package
/// name, i.e. still case-2 shaped. Its declared dist entry is accepted; the
/// deep import climbing past it into `src/internal.ts` reaches a location the
/// package never declared and must stay rejected. Acceptance therefore keys
/// on the declared-source condition, not on "workspace sibling" alone.
///
/// Both imports name a path inside the package rather than the bare package
/// name: zfb drives esbuild with the `neutral` platform, which ignores `main`
/// entirely, so `import "@acme/built"` would not resolve at all here. The
/// classification under test is unaffected — both keys are still
/// `node_modules/@acme/built/...`, i.e. case-2 shaped.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_consume_from_source_esbuild_regression -- --ignored"]
fn nested_member_undeclared_deep_import_into_dist_shipping_sibling_stays_rejected() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_consume_from_source_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_workspace_root(root);
    write_required_project_dirs(root);
    let node_modules = write_dist_shipping_sibling(root);
    let project = write_nested_host(root);
    write(
        &project.join("pages/index.tsx"),
        r#"
            import { built } from "@acme/built/dist/index.js";
            import { internal } from "@acme/built/src/internal.ts";
            export default function Home() {
              return "CONSUMER_MARKER:" + built + ":" + internal;
            }
        "#,
    );

    let error = bundle_with_session(
        base_input(&project, esbuild, node_modules),
        Some(&mut ShadowSession::new(&project).expect("shadow session")),
    )
    .expect_err(
        "a deep import into a location a dist-shipping sibling never declared \
         is a genuine stage escape and must stay rejected (#2040)",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("stage-escape audit"),
        "expected a stage-escape audit failure; got: {message}"
    );
    assert!(
        message.contains("node_modules/@acme/built/src/internal.ts"),
        "expected the undeclared deep-import key as the offender; got: {message}"
    );
    assert!(
        !message.contains("dist/index.js"),
        "the package's own declared dist entry must not be flagged; got: {message}"
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
