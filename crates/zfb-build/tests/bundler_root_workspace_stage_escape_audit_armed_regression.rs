//! Issue #1984 (epic #1982) — RED test for #1730's SSR site.
//!
//! When `project_root` is itself claimed by `pnpm-workspace.yaml` (e.g.
//! `packages: ['.', 'packages/*']`), `zfb_types::first_party_root_for`
//! (`crates/zfb-types/src/first_party.rs`,
//! `project_root_that_is_the_workspace_root_maps_to_itself`) maps
//! `first_party_root` to `project_root` unchanged. `bundle_with_session`
//! (`crates/zfb-build/src/bundler.rs`) computes:
//!
//! ```text
//! let workspace_rel = normalize_path_lexical(&input.project_root)
//!     .strip_prefix(&first_party_root)
//!     .ok()
//!     .filter(|rel| !rel.as_os_str().is_empty());
//! ```
//!
//! so `workspace_rel` is `None` too, and `shadow == work` (no nested project
//! mirror). Every step gated on `workspace_rel.is_some()` — including the
//! guard (b) metafile stage-escape audit (`audit_metafile_stage_escape_at_path`,
//! ~bundler.rs:3890) — is skipped. SSR has no scan-time guard (a) at all
//! (that is islands-only, `zfb-islands/src/scanner.rs`), so with guard (b)
//! off there is now NO backstop whatsoever for this build.
//!
//! A first-party CHILD package still lives under `packages/*`, physically
//! nested inside `project_root`. As an ordinary (non-gitignored) top-level
//! directory it IS still copied into the shadow by
//! `enumerate_extra_top_level_dirs`'s "extra source dir" pass — a staged
//! `packages/child/index.ts` genuinely exists. But `import { x } from
//! "@scope/child"` is a BARE package-name specifier, so esbuild resolves it
//! through its ordinary node_modules walk, never through that staged
//! relative path. The wholesale `<shadow>/node_modules` symlink to the live
//! `node_modules_dir` (bundler.rs:3078) still exists (a normal,
//! non-workspace-widened build sets it up unconditionally), and a real
//! pnpm-style `node_modules/@scope/child -> packages/child` symlink resolves
//! the bare import straight to the LIVE child source — bypassing the staged
//! copy entirely, not merely reaching an unmirrored file.
//!
//! This test proves the escape is **silent**: `bundle_with_session` returns
//! `Ok`, the emitted bundle carries the live child package's marker, and the
//! real esbuild metafile records the import as `node_modules/@scope/child/index.ts`
//! — the epic's documented "case 2" shape (a `node_modules`-shaped key) —
//! even though a staged `packages/child/index.ts` sits right there, unused.
//! Guard (b) exists precisely to reject a case-2 key like this one; it is
//! not armed here.
//!
//! Not fixed by this test — Wave 3 (#1986), Wave 4 (#2040), and Wave 6
//! (#1988) own the eligibility predicate, the case-2 policy, and the SSR
//! wiring fix; this only proves today's defect.

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
/// and the audit stays armed).
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

fn metafile_input_keys(shadow: &Path) -> Vec<String> {
    let bytes = fs::read(shadow.join(".zfb-metafile.json")).expect("read real esbuild metafile");
    let metafile: serde_json::Value =
        serde_json::from_slice(&bytes).expect("parse esbuild metafile");
    metafile["inputs"]
        .as_object()
        .expect("metafile inputs object")
        .keys()
        .cloned()
        .collect()
}

fn bundle_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read emitted bundle")
}

#[cfg(unix)]
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_disabled_regression -- --ignored"]
fn real_esbuild_silently_resolves_root_workspace_child_package_escape_through_node_modules_symlink()
{
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_stage_escape_audit_disabled_regression] no esbuild binary; skipping."
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
    // package never being staged at all.
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
    let output = bundle_with_session(base_input(root, esbuild, node_modules), Some(&mut session))
        .expect(
            "today the build succeeds even though the child package resolved through \
             node_modules to LIVE, unmirrored source: SSR has no scan-time guard (a), and \
             guard (b)'s metafile stage-escape audit is disabled at the root (workspace_rel is \
             None), so this is a completely SILENT stage escape",
        );

    let body = bundle_text(&output.bundle_path);
    assert!(
        body.contains("CHILD_PACKAGE_ESCAPE_MARKER"),
        "the live child package's marker must reach the emitted bundle unflagged; got: {body}"
    );

    // Prove the escape at the metafile level too. Esbuild's real metafile
    // records the resolved input as `node_modules/@scope/child/index.ts` —
    // this is exactly the epic's documented "case 2" shape (a
    // `node_modules`-shaped key): the wholesale `<shadow>/node_modules`
    // symlink resolves the bare package-name import straight to the live
    // child package, bypassing the staged copy asserted below entirely (see
    // that assertion's comment for why a staged copy existing at all does
    // not save this import). Guard (b) exists precisely to reject a case-2
    // key like this one when armed; it is not armed here.
    let keys = metafile_input_keys(session.shadow_root());
    assert!(
        keys.iter()
            .any(|key| key == "node_modules/@scope/child/index.ts"),
        "expected the case-2 escaped child-package metafile key \
         node_modules/@scope/child/index.ts; got {keys:?}"
    );

    // `packages/` is an ordinary (non-gitignored) top-level directory, so
    // `enumerate_extra_top_level_dirs` DOES sweep it into the shadow as a
    // normal "extra source dir" copy — a staged `packages/child/index.ts`
    // genuinely exists. That staged copy is exactly what makes this an
    // interesting escape rather than a trivial "nothing was ever mirrored"
    // case: `import { childMarker } from "@scope/child"` is a BARE
    // package-name specifier, so esbuild resolves it via its node_modules
    // walk, not via the staged relative path — it never even looks at
    // `shadow/packages/child`. The staged copy above sits there unused while
    // resolution silently takes the `<shadow>/node_modules` symlink straight
    // to the LIVE source instead.
    assert!(
        session
            .shadow_root()
            .join("packages/child/index.ts")
            .is_file(),
        "packages/child must have been staged normally (it is an ordinary, non-gitignored \
         top-level dir) — the bug is that the bare package-name import bypasses this staged \
         copy entirely, not that nothing was ever staged"
    );
}
