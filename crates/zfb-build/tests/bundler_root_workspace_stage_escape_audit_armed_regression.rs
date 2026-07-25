//! Issue #1988 (Wave 6, epic #1982) — the SSR half of #1984's red test, now
//! GREEN. Renamed from
//! `bundler_root_workspace_stage_escape_audit_disabled_regression.rs`
//! (formerly `real_esbuild_silently_resolves_root_workspace_child_package_escape_through_node_modules_symlink`)
//! to reflect the armed reality: the escape it proved silent is now REJECTED.
//!
//! When `project_root` is itself claimed by `pnpm-workspace.yaml` (e.g.
//! `packages: ['.', 'packages/*']`), `zfb_types::first_party_root_for`
//! (`crates/zfb-types/src/first_party.rs`,
//! `project_root_that_is_the_workspace_root_maps_to_itself`) maps
//! `first_party_root` to `project_root` unchanged, so `workspace_rel` is
//! `None` and `shadow == work` (no nested project mirror). Before this
//! wave, every step gated on `workspace_rel.is_some()` — including the
//! guard (b) metafile stage-escape audit
//! (`audit_metafile_stage_escape_at_path`, ~bundler.rs:3979) — was skipped.
//! SSR has no scan-time guard (a) at all (that is islands-only,
//! `zfb-islands/src/scanner.rs`), so with guard (b) off there was NO
//! backstop whatsoever for this build — a completely SILENT stage escape.
//!
//! Wave 6 swaps that `workspace_rel.is_some()` proxy for
//! `zfb_types::stage_escape_audit_eligibility` (issue #1986), which reads
//! the reachable `node_modules/@scope/child -> packages/child` link as
//! eligible regardless of widening (row 3 of its decision table). The child
//! package below declares neither `exports` nor `main` — an UNDECLARED
//! entry, deliberately distinct from #2040's "consume from source" carve-out
//! (see `bundler_consume_from_source_esbuild_regression.rs`) — so once guard
//! (b) is armed, the build must now FAIL with a stage-escape error naming
//! the escaped input instead of silently succeeding.
//!
//! A first-party CHILD package still lives under `packages/*`, physically
//! nested inside `project_root`. As an ordinary (non-gitignored) top-level
//! directory it IS still copied into the shadow by
//! `enumerate_extra_top_level_dirs`'s "extra source dir" pass — a staged
//! `packages/child/index.ts` genuinely exists. But `import { x } from
//! "@scope/child"` is a BARE package-name specifier, so esbuild resolves it
//! through its ordinary node_modules walk, never through that staged
//! relative path. The wholesale `<shadow>/node_modules` symlink to the live
//! `node_modules_dir` still exists (a normal, non-workspace-widened build
//! sets it up unconditionally), and a real pnpm-style
//! `node_modules/@scope/child -> packages/child` symlink resolves the bare
//! import straight to the LIVE child source — bypassing the staged copy
//! entirely, not merely reaching an unmirrored file. Guard (b) exists
//! precisely to reject a case-2 key like this one; it is now armed here.

use std::fs;
use std::path::Path;

use zfb_build::{bundle_with_session, BundleMode, BundlerInput, ShadowSession};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directory");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// `project_root` is itself the workspace root: both `.` and `packages/*`
/// are explicitly claimed, matching #1730's repro shape (as opposed to
/// `bundler_workspace_root_alias_esbuild_regression.rs`'s NESTED
/// `sub-packages/host` topology, where `first_party_root != project_root`
/// and the audit was already armed before this wave).
fn write_root_workspace(root: &Path) {
    write(
        &root.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'packages/*'\n",
    );
    write(
        &root.join("package.json"),
        r#"{ "name": "host", "private": true }"#,
    );
    for dir in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(root.join(dir)).expect("create required project directory");
    }
}

fn base_input(
    project: &Path,
    esbuild: std::path::PathBuf,
    node_modules: std::path::PathBuf,
) -> BundlerInput {
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
    input
}

#[cfg(unix)]
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_armed_regression -- --ignored"]
fn real_esbuild_now_rejects_root_workspace_child_package_escape_through_node_modules_symlink() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_stage_escape_audit_armed_regression] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace(root);

    // First-party CHILD package, physically nested INSIDE project_root
    // (since project_root == workspace root here). It IS staged as an
    // ordinary "extra top-level dir" — the escape below is about a bare
    // package-name import bypassing that staged copy, not about the
    // package never being staged at all. Deliberately UNDECLARED (no
    // `exports`/`main`), distinct from #2040's consume-from-source carve-out.
    write(
        &root.join("packages/child/package.json"),
        r#"{ "name": "@scope/child", "private": true }"#,
    );
    write(
        &root.join("packages/child/index.ts"),
        r#"export const childMarker = "CHILD_PACKAGE_ESCAPE_MARKER";"#,
    );

    // The genuine pnpm-style symlink a real install produces:
    // node_modules/@scope/child -> packages/child.
    let node_modules = root.join("node_modules");
    fs::create_dir_all(node_modules.join("@scope")).expect("create node_modules/@scope");
    std::os::unix::fs::symlink(
        root.join("packages/child"),
        node_modules.join("@scope/child"),
    )
    .expect("link first-party child package into node_modules");

    write(
        &root.join("pages/index.tsx"),
        r#"
            import { childMarker } from "@scope/child";
            export default function Home() {
              return "ROOT_SSR_MARKER:" + childMarker;
            }
        "#,
    );

    let mut session = ShadowSession::new(root).expect("shadow session");
    let error = bundle_with_session(base_input(root, esbuild, node_modules), Some(&mut session))
        .expect_err(
            "now that the eligibility predicate (issue #1986) arms guard (b) at a root-claimed \
             workspace, the undeclared child package's bare-package-name import must be \
             rejected as a stage escape instead of silently resolving to live source",
        );

    // Guard (b)'s metafile stage-escape audit names the escaped input
    // explicitly — the epic's documented "case 2" shape (a
    // `node_modules`-shaped key) with no declared entry root.
    let message = format!("{error:#}");
    assert!(
        message.contains("node_modules/@scope/child/index.ts"),
        "expected the stage-escape error to name the escaped case-2 child-package metafile key \
         node_modules/@scope/child/index.ts; got: {message}"
    );
    assert!(
        message.contains("stage-escape audit") || message.contains("escaped their stage"),
        "expected a guard (b) stage-escape audit failure; got: {message}"
    );

    // `packages/` is an ordinary (non-gitignored) top-level directory, so
    // `enumerate_extra_top_level_dirs` DOES sweep it into the shadow as a
    // normal "extra source dir" copy — a staged `packages/child/index.ts`
    // genuinely exists. That staged copy is exactly what makes this an
    // interesting escape rather than a trivial "nothing was ever mirrored"
    // case: `import { childMarker } from "@scope/child"` is a BARE
    // package-name specifier, so esbuild resolves it via its node_modules
    // walk, not via the staged relative path — it never even looks at
    // `shadow/packages/child`. The staged copy sits there unused while
    // resolution takes the `<shadow>/node_modules` symlink straight to the
    // LIVE source instead, which is exactly what guard (b) now catches.
    assert!(
        session
            .shadow_root()
            .join("packages/child/index.ts")
            .is_file(),
        "packages/child must have been staged normally (it is an ordinary, non-gitignored \
         top-level dir) — the bug this test used to prove was that the bare package-name import \
         bypassed this staged copy entirely, not that nothing was ever staged; guard (b) now \
         rejects that bypass instead of letting it through silently"
    );
}
