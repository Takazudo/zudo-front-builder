//! Issue #2208 (Md & Staging Fixes epic #2205) — the next.96 → next.97
//! regression for a project that IS the pnpm workspace root
//! (`pnpm-workspace.yaml` claims `'.'`) importing a repo-root loose file
//! through its declared broad alias (`"@/*": ["<root>/*"]`).
//!
//! Two individually-correct changes interact; staging rules did NOT change
//! 96→97 — only the audit arming did:
//!
//! 1. Root loose files reached via a wildcard root alias were staged ONLY
//!    under active exclusions (`!bundle_exclude.is_empty()` at the
//!    `stage_project_root_loose_files` gate in `bundler.rs`). Under an empty
//!    exclude the dual-target rebased tsconfig keeps a live-real fallback,
//!    so `@/data.json` silently resolved against the LIVE project root —
//!    fine pre-arming, when no audit looked at this build's metafile.
//! 2. next.97 armed the SSR stage-escape audit for root-claimed workspaces:
//!    the guard (b) call site now uses `zfb_types::stage_escape_audit_eligibility`
//!    (row 3: a reachable first-party `node_modules` link) instead of the
//!    old `workspace_rel.is_some()` proxy. The live-resolved loose file now
//!    classifies as case 4 — "first-party input resolved outside every stage
//!    root, no staged spelling" — and hard-fails the build.
//!
//! The #2208 fix stages the narrow surface instead of touching the audit
//! (the #1840 precedent): the loose-file staging gate widens to
//! `(!bundle_exclude.is_empty() || root_claimed_workspace)`, where
//! `root_claimed_workspace` is the declared-data predicate
//! `workspace_rel.is_none() && zfb_types::first_party::workspace_root_claims_path(
//! &first_party_root, &project_root)`. With the loose file staged, esbuild's
//! shadow-first dual-target resolution picks the staged copy and the
//! metafile records the STAGED spelling — the audit accepts via the
//! in-stage short-circuit, unweakened.
//!
//! Written RED-first: before the staging fix, the ACCEPT test below failed
//! with the exact case-4 message quoted above, and the negative control's
//! offender list wrongly included the live `data.json` spelling beside the
//! genuine `@scope/child` escape. Note `stage_project_root_loose_files`
//! stages ALL eligible depth-1 root loose files, not only files proven
//! imported — esbuild still determines what enters the bundle.

use std::collections::BTreeMap;
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
/// are explicitly claimed (the #1730 shape, modeled on
/// `bundler_root_workspace_stage_escape_audit_armed_regression.rs`'s
/// `write_root_workspace`), with the source issue's repro chain: a tracked
/// root loose `data.json`, reached from `pages/index.tsx` through
/// `src/lib/wrapper.ts` via the broad `@/*` alias.
fn write_root_workspace_with_loose_file(root: &Path) {
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

    // The tracked repo-root loose file the broad alias reaches.
    write(
        &root.join("data.json"),
        r#"{ "looseMarker": "ROOT_LOOSE_JSON_MARKER" }"#,
    );
    write(
        &root.join("src/lib/wrapper.ts"),
        r#"
            import data from "@/data.json";
            export const wrapped = "WRAPPED:" + data.looseMarker;
        "#,
    );
    write(
        &root.join("pages/index.tsx"),
        r#"
            import { wrapped } from "../src/lib/wrapper";
            export default function Home() {
              return "ROOT_SSR_MARKER:" + wrapped;
            }
        "#,
    );

    // A first-party CHILD package under `packages/*`, deliberately
    // UNDECLARED (no `exports`/`main`) — distinct from #2040's
    // consume-from-source carve-out. Its `node_modules` symlink below is
    // what arms the audit's eligibility row 3 at this root-claimed
    // workspace; the negative-control test also imports it by bare package
    // name to prove the audit stayed unweakened.
    write(
        &root.join("packages/child/package.json"),
        r#"{ "name": "@scope/child", "private": true }"#,
    );
    write(
        &root.join("packages/child/index.ts"),
        r#"export const childMarker = "CHILD_PACKAGE_ESCAPE_MARKER";"#,
    );
}

/// The genuine pnpm-style symlink a real install produces:
/// `node_modules/@scope/child -> packages/child`. Purely to arm the
/// stage-escape audit's eligibility row 3 (a reachable first-party
/// `node_modules` link) — the exact arming that turned the previously-silent
/// live-fallback resolution into a hard case-4 failure.
#[cfg(unix)]
fn link_child_into_node_modules(root: &Path) -> std::path::PathBuf {
    let node_modules = root.join("node_modules");
    fs::create_dir_all(node_modules.join("@scope")).expect("create node_modules/@scope");
    std::os::unix::fs::symlink(
        root.join("packages/child"),
        node_modules.join("@scope/child"),
    )
    .expect("link first-party child package into node_modules");
    node_modules
}

/// EMPTY `bundle.exclude` — the regression's precondition. The broad
/// `@/*` alias targets the project root itself, and `node_modules_dir` +
/// non-empty `tsconfig_paths` + `node_modules_preserve_symlinks = false`
/// is the real-consumer shape that runs esbuild WITHOUT
/// `--preserve-symlinks` (`esbuild_will_preserve_symlinks` branch 4), so a
/// symlink workaround would be canonicalised away — staging is the only
/// clean spelling source.
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
    input.tsconfig_paths = BTreeMap::from([(
        "@/*".to_string(),
        vec![project.join("*").to_string_lossy().into_owned()],
    )]);
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

/// The ACCEPT regression (issue #2208): a root-claimed workspace importing a
/// repo-root loose file through its declared broad alias must build clean
/// under an EMPTY `bundle.exclude`, with the metafile recording the STAGED
/// spelling — never the live-root fallback the audit rejects as case 4.
#[cfg(unix)]
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_loose_file_alias_esbuild_regression -- --ignored"]
fn real_esbuild_accepts_root_workspace_loose_file_reached_via_broad_alias() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_loose_file_alias_esbuild_regression] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace_with_loose_file(root);
    let node_modules = link_child_into_node_modules(root);

    let mut session = ShadowSession::new(root).expect("shadow session");
    let output = bundle_with_session(base_input(root, esbuild, node_modules), Some(&mut session))
        .expect(
            "issue #2208: a root-claimed workspace reaching a tracked root loose file through \
             its declared broad alias must build clean under an empty bundle.exclude — before \
             the loose-file staging gate widened, this failed with the exact case-4 \
             \"first-party input resolved outside every stage root, no staged spelling\" \
             rejection",
        );

    let body = fs::read_to_string(&output.bundle_path).expect("read emitted bundle");
    assert!(
        body.contains("ROOT_LOOSE_JSON_MARKER"),
        "the loose file's JSON payload must ship in the bundle; got: {body}"
    );

    // For a root-claimed workspace `shadow == work == session.shadow_root()`
    // and esbuild's cwd is that shadow, so the staged loose file's metafile
    // spelling is the bare relative `data.json`. Canonicalise for macOS's
    // `/var` -> `/private/var` tempdir aliasing.
    let shadow = fs::canonicalize(session.shadow_root()).expect("canonicalize shadow root");
    assert!(
        shadow.join("data.json").is_file(),
        "the root loose file must have been staged into the shadow"
    );
    let keys = metafile_input_keys(&shadow);
    assert!(
        keys.iter().any(|key| key == "data.json"),
        "metafile must record the exact STAGED spelling data.json; got {keys:?}"
    );
    assert!(
        keys.iter()
            .filter(|key| key.contains("data.json"))
            .all(|key| key == "data.json"),
        "no live-root fallback spelling of data.json may appear in the metafile \
         (the staged copy must win the dual-target resolution); got {keys:?}"
    );
}

/// The negative control, in the SAME fixture: widening the loose-file
/// staging gate must NOT loosen the audit. The undeclared `@scope/child`
/// bare-package-name escape stays REJECTED — while the staged `data.json`
/// no longer appears among the offenders.
#[cfg(unix)]
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_loose_file_alias_esbuild_regression -- --ignored"]
fn undeclared_bare_package_name_escape_stays_rejected_beside_loose_file_staging() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_loose_file_alias_esbuild_regression] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace_with_loose_file(root);
    let node_modules = link_child_into_node_modules(root);

    // Same page, plus the escape: a bare package-name import of the
    // undeclared child package, resolved through the node_modules symlink
    // straight to LIVE source (the armed-regression shape).
    write(
        &root.join("pages/index.tsx"),
        r#"
            import { wrapped } from "../src/lib/wrapper";
            import { childMarker } from "@scope/child";
            export default function Home() {
              return "ROOT_SSR_MARKER:" + wrapped + ":" + childMarker;
            }
        "#,
    );

    let mut session = ShadowSession::new(root).expect("shadow session");
    let error = bundle_with_session(base_input(root, esbuild, node_modules), Some(&mut session))
        .expect_err(
            "the undeclared child package's bare-package-name import must stay rejected as a \
             stage escape — the #2208 loose-file staging fix must not loosen the audit",
        );

    let message = format!("{error:#}");
    assert!(
        message.contains("stage-escape audit") || message.contains("escaped their stage"),
        "expected a guard (b) stage-escape audit failure; got: {message}"
    );
    // Unlike `bundler_root_workspace_stage_escape_audit_armed_regression.rs`
    // (empty `tsconfig_paths`, so esbuild keeps `--preserve-symlinks` and the
    // key stays `node_modules/@scope/child/index.ts`, case 2), THIS fixture's
    // broad alias makes `tsconfig_paths` non-empty, flipping
    // `esbuild_will_preserve_symlinks` to branch 4 (copy mode): esbuild
    // canonicalises the `node_modules/@scope/child` symlink away and records
    // the live `..`-climbing `packages/child/index.ts` spelling instead —
    // the #2086 canonicalized-key territory, still fail-closed (the child
    // declares no entries, so the declared-identity lookup yields None and
    // the case-4 rejection stands).
    assert!(
        message.contains("packages/child/index.ts"),
        "expected the stage-escape error to name the escaped child-package input (the \
         canonicalised live spelling packages/child/index.ts under copy mode); got: {message}"
    );
    assert!(
        !message.contains("data.json"),
        "the staged root loose file must NOT be an offender — before the #2208 staging fix its \
         live-fallback spelling wrongly joined the offender list beside the genuine escape; \
         got: {message}"
    );
}
