//! Issue #2081 (Staging Correctness 2 epic #2078, Wave 1) originally pinned a
//! KNOWN, then-open gap in the SSR stage-escape audit (issue #2050,
//! superseded by #2078): at a root-claimed workspace
//! (`first_party_root == project_root`, so `shadow == work` —
//! `crates/zfb-build/src/bundler.rs` :2100-2135), whenever
//! `<work>/node_modules` held a **real (non-symlink) staged copy** of a
//! workspace package instead of the usual symlink, an undeclared stage escape
//! shipped with no error at all — exactly the P1 "completely silent" framing
//! #1730/#1988 were meant to close.
//!
//! **Both halves of that gap are now closed.** It took two fixes in two
//! different crates, and this file's four tests are the record of both:
//!
//! 1. **Eligibility** (sub #2087, Wave 3). Guard (b)'s eligibility check
//!    (`zfb_types::stage_escape_audit_eligibility`) used to scan
//!    `work.join("node_modules")` for a **symlink** whose canonical target is
//!    a claimed workspace package
//!    (`crates/zfb-types/src/audit_eligibility.rs`); with only a real copy
//!    present it found none, fell back to
//!    [`zfb_types::AuditEligibility::NoReachableFirstPartyPackage`], and
//!    silently disarmed the audit. #2087 extended row 3's evidence to
//!    **declared** identity (`package.json` `name` + workspace claim, via
//!    `zfb_types::claimed_workspace_member_names`), so a real copy now arms
//!    the audit.
//! 2. **Classification** (issue #2127, this wave). Arming eligibility alone
//!    did NOT make `bundle_with_session` reject the escape: a SECOND,
//!    previously-undiscovered gap sat one layer up, in
//!    `crates/zfb-build/src/metafile_deps.rs::audit_metafile_stage_escape`'s
//!    own case classification. A real-copy-staged package's canonical
//!    (symlink-resolved) path trivially STILL contains a `node_modules`
//!    segment — there is no symlink to resolve away from, the file is
//!    genuinely, physically there — so it was unconditionally classified
//!    "case 3: ordinary third-party dependency, allowed" before declared
//!    identity was ever consulted. #2127 made the case-2/case-3 boundary
//!    consult the claimed-member roster by declared NAME for exactly this
//!    shape (an ordinary registry dependency's name matches nothing there, so
//!    case 3 is untouched), and then apply the same declared-entry rule case 2
//!    already applies to the symlink shape.
//!
//! # Why this file is still called `…_disarm_pin`
//!
//! The name states the QUESTION all four tests investigate — whether either
//! staging configuration #2050 named can disarm the stage-escape audit at a
//! root-claimed workspace — not the answer, which is now uniformly "no". The
//! per-test names below carry the current facts. Renaming was considered and
//! declined: every candidate either collided confusingly with the sibling
//! `bundler_root_workspace_stage_escape_audit_armed_regression.rs` binary or
//! misdescribed the last test
//! ([`empty_exclude_workspace_package_exact_staging_does_not_disarm_the_audit_at_root_claimed_workspace`]),
//! a symlink-shape negative control that is not about real-copy staging at
//! all.
//!
//! # The two trigger shapes named in #2081 — and what exploration found
//!
//! #2081 asked this pin to cover two shapes that make `<work>/node_modules`
//! hold real copies instead of a symlink
//! (`crates/zfb-build/src/bundler.rs` :2591-2624, :3130-3135):
//!
//! 1. **`bundle.exclude` is non-empty.** Confirmed reproducible below
//!    ([`real_copy_staging_under_active_bundle_exclude_stages_a_real_copy_and_arms_eligibility`]):
//!    with any non-empty (even non-matching) `bundle.exclude`, the live
//!    `<shadow>/node_modules -> <live node_modules>` symlink is never created;
//!    non-excluded dependencies — including an undeclared workspace sibling
//!    reached by bare package name — are staged as REAL copies at their
//!    natural position instead. Eligibility arms (`FirstPartyPackageReachable`,
//!    since #2087) AND the build now fails
//!    ([`real_copy_staging_under_active_bundle_exclude_rejects_the_undeclared_escape`],
//!    since #2127).
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

/// The POSITIVE twin of [`write_undeclared_child_package`]: the identical
/// package and topology, except `@scope/child` DECLARES its source tree as an
/// entry root — #2040's "consume from source" carve-out. Everything else is
/// byte-for-byte the same, so the only variable between the two fixtures is
/// declaredness itself.
///
/// The `"."` export is what the shared [`write_consumer_entry`] bare-name
/// import (`from "@scope/child"`) resolves through; the sibling `"./*"` entry
/// is the wildcard shape #2040's own fixtures use. Both declare the `src/`
/// entry root, which is what the audit reads.
fn write_declared_child_package(root: &Path) -> std::path::PathBuf {
    write(
        &root.join("packages/child/package.json"),
        r#"{ "name": "@scope/child", "exports": { ".": "./src/index.ts", "./*": "./src/*" } }"#,
    );
    write(
        &root.join("packages/child/src/index.ts"),
        r#"export const childMarker = "CHILD_PACKAGE_DECLARED_MARKER";"#,
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

/// **Part 1 — trigger shape 1 (non-empty `bundle.exclude`): the two
/// PRECONDITIONS.** Renamed and reasserted by issue #2127 from
/// `real_copy_staging_under_active_bundle_exclude_is_now_eligible_yet_the_metafile_audit_still_admits_it_as_case_three`,
/// which #2087 had in turn renamed from #2081's original disarm pin. Two of
/// that version's assertions — *the build succeeds* and *the escape marker
/// ships* — described the very gap #2127 closed and are now false, so they
/// are gone (the marker assertion is not merely false but unreachable: the
/// build fails, so no bundle is emitted to read). The two that survive are
/// the ones this test uniquely pins, and they are what makes its sibling's
/// rejection meaningful rather than incidental:
///
/// 1. **the staging MECHANISM** — `<shadow>/node_modules` really is a real
///    directory here, not a symlink to the live tree, so the rejection below
///    genuinely exercises the real-copy classification path and not the
///    ordinary symlink one (`bundler.rs`'s live-link branch is skipped once
///    `bundle.exclude` is non-empty);
/// 2. **the eligibility VERDICT** — `stage_escape_audit_eligibility` returns
///    `FirstPartyPackageReachable` for this topology (#2087), asserted
///    directly rather than inferred from the build's outcome, so a future
///    change to either fix can never mask a regression in the other.
///
/// The rejection itself is asserted by
/// [`real_copy_staging_under_active_bundle_exclude_rejects_the_undeclared_escape`]
/// below; this test only records that the build no longer succeeds, without
/// inspecting the diagnostic.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_disarm_pin -- --ignored"]
fn real_copy_staging_under_active_bundle_exclude_stages_a_real_copy_and_arms_eligibility() {
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
    let _rejected = bundle_with_session(input, Some(&mut session)).expect_err(
        "with both halves of the #2050 gap closed (#2087 eligibility + #2127 classification), \
         the undeclared @scope/child sibling staged as a real copy under active bundle.exclude \
         must be rejected — the diagnostic itself is asserted by this test's sibling below",
    );

    // `<shadow>/node_modules` (== `<work>/node_modules` here, since
    // shadow == work at a root-claimed workspace) is a REAL directory, not a
    // symlink to the live tree — the staging mechanism itself, unchanged by
    // either fix (both changed how the staged copy is CLASSIFIED, never how
    // it is staged).
    let shadow_nm = session.shadow_root().join("node_modules");
    let shadow_nm_meta = fs::symlink_metadata(&shadow_nm).expect("shadow node_modules must exist");
    assert!(
        !shadow_nm_meta.file_type().is_symlink(),
        "shadow node_modules must be a REAL directory under active bundle.exclude, not a \
         symlink to the live tree — the staging mechanism this test documents"
    );

    // Pin the exact eligibility reason directly, independent of the build's
    // pass/fail outcome above, so a future change to `bundle_with_session`'s
    // error handling can never mask a change in the eligibility predicate
    // itself. This is the ONE assertion #2087 flips: eligibility is now
    // ARMED for this topology.
    let eligibility = zfb_types::stage_escape_audit_eligibility(
        root,
        &zfb_types::first_party_root_for(root),
        &shadow_nm,
    );
    let zfb_types::AuditEligibility::FirstPartyPackageReachable { link, target } = eligibility
    else {
        panic!("expected the NOW-ARMED eligibility reason (issue #2087); got {eligibility:?}");
    };
    assert_eq!(
        link,
        std::fs::canonicalize(node_modules_child_dir(&shadow_nm)).expect("canonicalize link"),
        "the declared-identity match must point at the staged real copy itself"
    );
    assert_eq!(
        target,
        std::fs::canonicalize(root.join("packages/child")).expect("canonicalize target"),
        "the declared-identity match must resolve to the claimed workspace member's own \
         directory, looked up by name — never by resolving the real copy's own path"
    );
}

fn node_modules_child_dir(node_modules: &Path) -> std::path::PathBuf {
    node_modules.join("@scope/child")
}

/// **The acceptance test for trigger shape 1 — written RED by #2081, GREEN
/// since #2127.** The SAME configuration as the preconditions test above must
/// REJECT the escape: an undeclared workspace sibling reached only via a
/// staged real copy is still a case-2 offender, and real-copy staging must
/// not exempt it.
///
/// #2081 wrote both assertions below in their desired post-fix form, and
/// **not one byte of them has been edited since** — the flip was the fix
/// landing under them, never an assertion being rewritten to suit it
/// (root CLAUDE.md rule 8). It was first tagged `pending-feature: #2087`, on the
/// assumption that arming eligibility alone would close this; #2087 landed,
/// armed eligibility correctly, and this test still failed, which is what
/// surfaced the separate classification gap #2127 (see this file's header
/// for both halves). Retagged to `#2127`, then flipped by #2127's
/// `metafile_deps.rs` fix and — per epic #2078's corrected flip protocol —
/// tagged with the ordinary `env-gate: esbuild` reason rather than having its
/// `#[ignore]` deleted: this binary needs a staged esbuild, so deleting the
/// attribute would drop it out of the `-- --ignored` lanes that run it
/// entirely. `health.yml`'s matching `--skip` was removed in the same commit.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_disarm_pin -- --ignored"]
fn real_copy_staging_under_active_bundle_exclude_rejects_the_undeclared_escape() {
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

/// **The POSITIVE control for #2127 at the production call site.** The
/// rejection above is only correct if the identical topology with a DECLARED
/// sibling still builds: #2127 changed the case-2/case-3 boundary, which
/// governs ALL third-party dependency classification, so the failure mode to
/// guard against is the fix becoming an overzealous "anything staged into
/// `node_modules` as a real copy is suspect" regression that breaks ordinary
/// builds.
///
/// Same workspace, same active `bundle.exclude`, same bare-package-name
/// import, same real-copy staging — the ONLY difference from the rejection
/// test above is that `@scope/child` declares its source tree as an entry
/// root (`exports: {"./*": "./src/*"}`), #2040's consume-from-source
/// carve-out. `bundle_with_session` must succeed and the sibling's source
/// must ship.
///
/// This closes the one real-esbuild coverage gap #2127 would otherwise leave:
/// this crate's other consume-from-source acceptance tests
/// (`bundler_consume_from_source_esbuild_regression.rs`) all run with an
/// EMPTY `bundle.exclude`, so they exercise the symlink shape and never reach
/// the real-copy discriminator at all. The unit-level twin is
/// `metafile_deps.rs`'s
/// `stage_escape_allows_consume_from_source_sibling_staged_as_a_real_copy`.
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_root_workspace_stage_escape_audit_disarm_pin -- --ignored"]
fn real_copy_staging_under_active_bundle_exclude_accepts_the_declared_sibling() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_root_workspace_stage_escape_audit_disarm_pin] no esbuild binary; skipping."
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    write_root_workspace(root);
    let node_modules = write_declared_child_package(root);
    write_consumer_entry(root);

    let mut input = base_input(root, esbuild, node_modules);
    input.bundle_exclude = vec!["**/*.neverMatchesAnything".to_string()];

    let mut session = ShadowSession::new(root).expect("shadow session");
    let output = bundle_with_session(input, Some(&mut session)).expect(
        "a DECLARED consume-from-source workspace sibling (#2040) staged as a real copy under \
         active bundle.exclude must still build — #2127's real-copy discriminator must reject \
         only what the package fails to declare, never real-copy staging as such",
    );

    let body = fs::read_to_string(&output.bundle_path).expect("read emitted bundle");
    assert!(
        body.contains("CHILD_PACKAGE_DECLARED_MARKER"),
        "the declared sibling's source must reach the emitted bundle; got: {body}"
    );

    // Same staging mechanism as the rejection test — proving this build took
    // the real-copy path too, not an incidental symlink one.
    let shadow_nm = session.shadow_root().join("node_modules");
    let shadow_nm_meta = fs::symlink_metadata(&shadow_nm).expect("shadow node_modules must exist");
    assert!(
        !shadow_nm_meta.file_type().is_symlink(),
        "shadow node_modules must be a REAL directory here too, or this test would not be \
         exercising the real-copy discriminator at all"
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
