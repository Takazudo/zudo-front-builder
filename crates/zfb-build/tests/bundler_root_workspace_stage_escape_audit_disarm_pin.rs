//! Issue #2081 (Staging Correctness 2 epic #2078, Wave 1) — pin of a KNOWN,
//! currently-open gap in the SSR stage-escape audit's eligibility predicate
//! (issue #2050, superseded by #2078): at a root-claimed workspace
//! (`first_party_root == project_root`, so `shadow == work` —
//! `crates/zfb-build/src/bundler.rs` :2100-2135), guard (b)'s eligibility
//! check (`zfb_types::stage_escape_audit_eligibility`) scans
//! `work.join("node_modules")` for a **symlink** whose canonical target is a
//! claimed workspace package (`crates/zfb-types/src/audit_eligibility.rs`
//! :78-99). Whenever that directory instead holds a **real (non-symlink)
//! staged copy** of the package, no symlink is found, eligibility falls back
//! to [`zfb_types::AuditEligibility::NoReachableFirstPartyPackage`], and the
//! audit is silently disarmed — an undeclared stage escape then ships with no
//! error at all, exactly the P1 "completely silent" framing #1730/#1988 were
//! meant to close.
//!
//! Sub #2087 (Wave 3 of epic #2078) lands the real fix (recognising a staged
//! package by its **declared** identity — `package.json` name + workspace
//! claim — instead of by link-ness). This file only PINS today's behavior and
//! adds a RED desired-form twin for #2087 to flip; no behavior changes here.
//!
//! # The two trigger shapes named in #2081 — and what exploration found
//!
//! #2081 asks this pin to cover two shapes that make `<work>/node_modules`
//! hold real copies instead of a symlink
//! (`crates/zfb-build/src/bundler.rs` :2591-2624, :3130-3135):
//!
//! 1. **`bundle.exclude` is non-empty.** Confirmed reproducible below
//!    ([`real_copy_staging_under_active_bundle_exclude_disarms_eligibility_and_ships_the_escape_silently`]):
//!    with any non-empty (even non-matching) `bundle.exclude`, the live
//!    `<shadow>/node_modules -> <live node_modules>` symlink is never created;
//!    non-excluded dependencies — including an undeclared workspace sibling
//!    reached by bare package name — are staged as REAL copies at their
//!    natural position instead. `stage_escape_audit_eligibility` then finds no
//!    symlink and returns `NoReachableFirstPartyPackage`; the build succeeds
//!    even though `@scope/child` is undeclared (no `exports`/`main`) and would
//!    have been rejected as a case-2 offender had the symlink survived (as it
//!    does in `bundler_root_workspace_stage_escape_audit_armed_regression.rs`'s
//!    otherwise-IDENTICAL fixture).
//!
//! 2. **Empty `bundle.exclude`, but `workspace_package_staging_active` is
//!    true** (`bundler.rs` :2591-2602). Investigated directly against working
//!    tree HEAD (verified by temporarily instrumenting
//!    `workspace_package_staging_active`'s computation with an `eprintln!` and
//!    reverting it — not committed) and found **structurally unreachable at a
//!    root-claimed workspace**: `workspace_package_staging_active`'s own
//!    "is this a staged target a workspace package" check calls
//!    `canonical_workspace_package_logical_path`, whose FIRST line is
//!    `if first_party_root_for(project_root) == project_root { return None; }`
//!    — i.e. it unconditionally bails for exactly the root-claimed case this
//!    pin's precondition requires (`first_party_root == project_root`). This
//!    was confirmed empirically: forcing a workspace-sibling package into
//!    `exact_target_staging_dirs` via a plugin alias entry (bypassing the
//!    ordinary bare-import discovery path entirely) with an EMPTY
//!    `bundle.exclude` still measured `workspace_package_staging_active =
//!    false`, and the live `<shadow>/node_modules` symlink was created as
//!    usual — the audit stayed ARMED and correctly rejected the escape. This
//!    function's "workspace package" check can only ever return `Some` in a
//!    WIDENED (nested-member, `first_party_root != project_root`) build,
//!    where `stage_escape_audit_eligibility` is *already* unconditionally
//!    eligible via row 1 (`WidenedStage`) regardless of `node_modules`
//!    contents — so even if shape 2 fired, it could never independently
//!    disarm anything. [`empty_exclude_workspace_package_exact_staging_does_not_disarm_the_audit_at_root_claimed_workspace`]
//!    pins this as a **passing negative control**: this configuration builds
//!    clean today (no escape ships) precisely because the audit never
//!    disarms here. It is not a bug pin and carries no RED twin — there is
//!    nothing here for #2087 to fix. Flagging this for the epic/#2087's scope:
//!    the "second trigger shape" is not an independent disarm vector at the
//!    root-claimed level as coded; only shape 1 (`bundle.exclude` non-empty)
//!    needs #2087's fix.
//!
//! Unix-only: fixtures wire `node_modules` symlinks via
//! `std::os::unix::fs::symlink`, matching every other stage-escape-audit
//! fixture in this crate.

#![cfg(unix)]

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

/// The #1730 repro shape: `project_root` is itself the workspace root (both
/// `.` and `packages/*` explicitly claimed), matching
/// `bundler_root_workspace_stage_escape_audit_armed_regression.rs`'s own
/// fixture exactly, so the only difference between that file's armed
/// behavior and this file's disarmed behavior is the trigger condition under
/// test, never the topology.
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

/// An UNDECLARED (no `exports`/`main`) first-party child package, physically
/// nested inside `project_root` (since `project_root` == workspace root
/// here), reached ONLY through the genuine pnpm-style
/// `node_modules/@scope/child -> packages/child` symlink a real install
/// produces — the audit's case-2 offender shape, distinct from #2040's
/// consume-from-source carve-out.
fn write_undeclared_child_package(root: &Path) -> std::path::PathBuf {
    write(
        &root.join("packages/child/package.json"),
        r#"{ "name": "@scope/child", "private": true }"#,
    );
    write(
        &root.join("packages/child/index.ts"),
        r#"export const childMarker = "CHILD_PACKAGE_ESCAPE_MARKER";"#,
    );

    let node_modules = root.join("node_modules");
    fs::create_dir_all(node_modules.join("@scope")).expect("create node_modules/@scope");
    std::os::unix::fs::symlink(
        root.join("packages/child"),
        node_modules.join("@scope/child"),
    )
    .expect("link first-party child package into node_modules");
    node_modules
}

fn write_consumer_entry(root: &Path) {
    write(
        &root.join("pages/index.tsx"),
        r#"
            import { childMarker } from "@scope/child";
            export default function Home() {
              return "ROOT_SSR_MARKER:" + childMarker;
            }
        "#,
    );
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

fn bundle_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read emitted bundle")
}

/// **Part 1 pin — trigger shape 1 (non-empty `bundle.exclude`).** Today's
/// ACTUAL, known-disarmed behavior: an active (even non-matching)
/// `bundle.exclude` forces every staged dependency — including the
/// undeclared `@scope/child` workspace sibling — into REAL copies at their
/// natural shadow position instead of leaving the live
/// `<shadow>/node_modules` symlink in place. `stage_escape_audit_eligibility`
/// then finds no symlink evidence and returns `NoReachableFirstPartyPackage`,
/// so guard (b) never arms and the build succeeds — shipping the undeclared
/// sibling's source with no stage-escape error at all. This is a
/// characterization pin of a known-wrong-but-current behavior (see #2050);
/// it stays green until #2087 lands the declared-identity fix, at which point
/// this test itself documents history and its RED twin below flips.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_disarm_pin -- --ignored"]
fn real_copy_staging_under_active_bundle_exclude_disarms_eligibility_and_ships_the_escape_silently()
{
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_stage_escape_audit_disarm_pin] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace(root);
    let node_modules = write_undeclared_child_package(root);
    write_consumer_entry(root);

    let mut input = base_input(root, esbuild, node_modules);
    // Non-empty but non-matching: only the "exclusions active" branch matters
    // here, not what it excludes.
    input.bundle_exclude = vec!["**/*.neverMatchesAnything".to_string()];

    let mut session = ShadowSession::new(root).expect("shadow session");
    let output = bundle_with_session(input, Some(&mut session)).expect(
        "KNOWN-DISARMED (issue #2050/#2081): active bundle.exclude stages the undeclared \
         @scope/child sibling as a real copy instead of leaving the live node_modules symlink \
         in place, so guard (b)'s eligibility predicate finds no symlink evidence and never \
         arms — the build succeeds even though this is the same undeclared case-2 escape the \
         symlink-preserving sibling fixture rejects. Sub #2087 fixes this; until then the build \
         must keep succeeding here so this pin stays a faithful record of today's behavior.",
    );

    // The escape genuinely shipped: the undeclared sibling's source reached
    // the emitted bundle, completely unaudited.
    let body = bundle_text(&output.bundle_path);
    assert!(
        body.contains("CHILD_PACKAGE_ESCAPE_MARKER"),
        "the undeclared @scope/child sibling's source must have shipped into the bundle \
         unaudited; got: {body}"
    );

    // `<shadow>/node_modules` (== `<work>/node_modules` here, since
    // shadow == work at a root-claimed workspace) is a REAL directory, not a
    // symlink to the live tree — the mechanism that starves the eligibility
    // predicate of symlink evidence.
    let shadow_nm = session.shadow_root().join("node_modules");
    let shadow_nm_meta = fs::symlink_metadata(&shadow_nm).expect("shadow node_modules must exist");
    assert!(
        !shadow_nm_meta.file_type().is_symlink(),
        "shadow node_modules must be a REAL directory under active bundle.exclude, not a \
         symlink to the live tree — that is exactly the disarm mechanism this pin documents"
    );

    // Pin the exact eligibility reason directly, independent of the build's
    // pass/fail outcome above, so a future change to `bundle_with_session`'s
    // error handling can never mask a change in the eligibility predicate
    // itself.
    let eligibility = zfb_types::stage_escape_audit_eligibility(
        root,
        &zfb_types::first_party_root_for(root),
        &shadow_nm,
    );
    assert_eq!(
        eligibility,
        zfb_types::AuditEligibility::NoReachableFirstPartyPackage,
        "expected the KNOWN-DISARMED eligibility reason; got {eligibility:?}"
    );
}

/// **RED desired-form twin for trigger shape 1.** Once #2087 lands
/// declared-identity recognition, the SAME configuration above must instead
/// REJECT the escape — an undeclared workspace sibling reached only via a
/// staged real copy is still a case-2 offender, real-copy staging must not
/// exempt it. Written in the assertion form #2087 should make true, so that
/// sub can flip this green by dropping the `#[ignore]` (replaced per the
/// epic's corrected flip protocol — never just deleted) without touching a
/// single assertion.
#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2087"]
fn real_copy_staging_under_active_bundle_exclude_should_reject_the_undeclared_escape_once_declared_identity_lands(
) {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_stage_escape_audit_disarm_pin] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace(root);
    let node_modules = write_undeclared_child_package(root);
    write_consumer_entry(root);

    let mut input = base_input(root, esbuild, node_modules);
    input.bundle_exclude = vec!["**/*.neverMatchesAnything".to_string()];

    let mut session = ShadowSession::new(root).expect("shadow session");
    let error = bundle_with_session(input, Some(&mut session)).expect_err(
        "once #2087 lands declared-identity eligibility recognition, an undeclared workspace \
         sibling reached only via a staged REAL COPY under active bundle.exclude must still be \
         rejected as a stage escape — real-copy staging must not become a blanket exemption",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("node_modules/@scope/child/index.ts"),
        "expected the stage-escape error to name the escaped case-2 child-package metafile key \
         node_modules/@scope/child/index.ts; got: {message}"
    );
}

/// **Negative control for trigger shape 2 (empty `bundle.exclude` +
/// `workspace_package_staging_active`).** See this file's header for the full
/// investigation. Forces a workspace-sibling package into
/// `exact_target_staging_dirs` via a plugin alias entry that points directly
/// at the `node_modules/@scope/child` symlink — the same mechanism
/// `plan_concrete_target_staging` uses for ANY plugin alias, unconditionally,
/// regardless of `bundle.exclude` — while keeping `bundle.exclude` empty.
/// This is the closest reachable approximation of "a workspace package is
/// staged as an exact target with an empty exclude"; it still builds clean
/// (rejects the escape) because `workspace_package_staging_active` cannot
/// become true at a root-claimed workspace (see header). Not a bug pin, no
/// RED twin: there is nothing for #2087 to fix here.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_disarm_pin -- --ignored"]
fn empty_exclude_workspace_package_exact_staging_does_not_disarm_the_audit_at_root_claimed_workspace(
) {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_stage_escape_audit_disarm_pin] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace(root);
    let node_modules = write_undeclared_child_package(root);
    write_consumer_entry(root);

    let mut input = base_input(root, esbuild, node_modules.clone());
    // Empty `bundle.exclude` (the default) — the whole point of this
    // negative control. Force the workspace package into
    // `exact_target_staging_dirs` via an unconditional plugin alias, the one
    // staging path that runs regardless of `bundle.exclude`.
    debug_assert!(input.bundle_exclude.is_empty());
    input.plugin_alias_entries = vec![(
        "virtual:disarm-pin-probe-alias".to_string(),
        node_modules
            .join("@scope/child")
            .to_string_lossy()
            .to_string(),
    )];

    let mut session = ShadowSession::new(root).expect("shadow session");
    let error = bundle_with_session(input, Some(&mut session)).expect_err(
        "empty bundle.exclude + a workspace package staged as an exact target must NOT disarm \
         the audit at a root-claimed workspace: workspace_package_staging_active's own \
         workspace-membership check (canonical_workspace_package_logical_path) unconditionally \
         returns None whenever first_party_root == project_root, so it can never observe this \
         staged target as a workspace package here — the live node_modules symlink is created \
         as usual and the escape must still be rejected",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("node_modules/@scope/child/index.ts"),
        "expected the stage-escape error to name the escaped case-2 child-package metafile key \
         node_modules/@scope/child/index.ts; got: {message}"
    );

    // Confirm the mechanism directly: the live symlink was created (not a
    // real copy), consistent with workspace_package_staging_active == false.
    let shadow_nm = session.shadow_root().join("node_modules");
    let shadow_nm_meta = fs::symlink_metadata(&shadow_nm).expect("shadow node_modules must exist");
    assert!(
        shadow_nm_meta.file_type().is_symlink(),
        "shadow node_modules must remain a symlink to the live tree here — \
         workspace_package_staging_active never fires at a root-claimed workspace, so the \
         empty-bundle_exclude live-symlink branch still applies"
    );
}
