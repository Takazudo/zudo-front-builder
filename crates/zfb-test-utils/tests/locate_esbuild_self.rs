//! Compile-time + runtime sanity check for `locate_esbuild()`.
//!
//! Contract (issue #1007 / #1015):
//! 1. The function compiles and links correctly.
//! 2. It never panics when esbuild is genuinely absent (returning `None`
//!    is the graceful skip) AND never panics when lookup succeeds.
//!    It panics ONLY on the harness-bug state: the expected slot binary
//!    (`crates/zfb/binaries/esbuild/<binary>`) exists as a file under a
//!    candidate workspace root, yet lookup still failed. A healthy
//!    checkout can never be in that state, so calling it here is safe on
//!    machines with and without esbuild.
//! 3. The workspace root is re-derived at runtime, so a stale rlib
//!    (compiled from a since-moved/deleted checkout path) cannot pin an
//!    outdated compile-time path and silently skip gated tests.
//!
//! The panic-vs-graceful decision is factored into the pure
//! `classify_failed_lookup` function so it is testable hermetically —
//! the tests below MUST keep passing on machines WITHOUT the slot binary.
//!
//! This test does NOT check for the macro — `CARGO_BIN_EXE_zfb` is only
//! set when compiling integration tests for the `zfb` binary crate; using
//! `zfb_binary!()` here would cause a compile error.
//!
//! Issue #2178 extends the contract: a slot file that cannot EXECUTE on
//! this host (wrong architecture, missing exec bit, not a binary) is
//! neither "present" nor "absent". The tests at the bottom of this file
//! cover the tri-state classification and rebuild the incident topology in
//! a tempdir with no real esbuild involved.

use std::path::{Path, PathBuf};

use zfb_test_utils::{
    candidate_workspace_roots, classify_failed_lookup, classify_slot_probes, finish_esbuild_lookup,
    locate_esbuild, locate_esbuild_from, SkipKind, SlotProbe, SlotState,
};

fn esbuild_binary_name() -> &'static str {
    if cfg!(windows) {
        "esbuild.exe"
    } else {
        "esbuild"
    }
}

#[test]
fn locate_esbuild_does_not_panic_in_legitimate_environments() {
    // Returning None is perfectly valid — the test environment may not have
    // esbuild installed. The only panic path is the harness-bug state (slot
    // binary present but lookup failed), which a correct lookup makes
    // unreachable on a healthy checkout.
    let _result = locate_esbuild();
}

#[test]
fn locate_esbuild_returns_existing_file_when_env_set() {
    // If ZFB_ESBUILD_BIN is set in the test environment and points to a real
    // file, locate_esbuild() must return that path.
    if let Some(bin_path) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = std::path::PathBuf::from(&bin_path);
        if p.is_file() {
            let result = locate_esbuild();
            assert_eq!(result.as_deref(), Some(p.as_path()));
        }
    }
    // If ZFB_ESBUILD_BIN is not set or doesn't point to a file, the test
    // passes trivially — this branch is just a bonus correctness check.
}

#[test]
fn runtime_derivation_finds_real_workspace_from_test_cwd() {
    // Cargo runs test binaries with the crate manifest dir as cwd
    // (crates/zfb-test-utils), so the runtime walk-up must find the
    // enclosing workspace root regardless of what path was baked in at
    // compile time. This is the stale-rlib defense from issue #1007.
    let roots = candidate_workspace_roots();
    assert!(
        !roots.is_empty(),
        "candidate_workspace_roots() must always yield at least the compile-time fallback"
    );
    assert!(
        roots
            .iter()
            .any(|r| r.join("crates/zfb-test-utils/Cargo.toml").is_file()),
        "no candidate root contains crates/zfb-test-utils — runtime derivation failed; \
         candidates: {roots:?}"
    );
    // Positively prove the RUNTIME derivation (not just the compile-time
    // fallback, which would also satisfy the assert above on a fresh
    // build): at least one candidate root must be an ancestor of the
    // process cwd, which only the runtime walk-up can produce.
    let cwd = std::env::current_dir().expect("test process must have a cwd");
    assert!(
        roots.iter().any(|r| cwd.starts_with(r)),
        "no candidate root is an ancestor of the test cwd {cwd:?} — the runtime cwd \
         walk-up is broken; candidates: {roots:?}"
    );
}

#[test]
fn slot_binary_present_implies_lookup_succeeds() {
    // The exact issue #1007 incident state: the slot binary exists yet
    // locate_esbuild() returns None. That must now be impossible.
    // Conditional on the slot binary existing so machines without it
    // (where this test trivially passes) stay green.
    let slot_present = candidate_workspace_roots().iter().any(|root| {
        root.join("crates/zfb/binaries/esbuild")
            .join(esbuild_binary_name())
            .is_file()
    });
    if slot_present {
        assert!(
            locate_esbuild().is_some(),
            "slot binary exists but locate_esbuild() returned None — the #1007 silent-skip \
             state has regressed"
        );
    }
}

#[test]
fn classify_failed_lookup_is_graceful_when_no_slot_binary_exists() {
    // Genuinely-absent esbuild: no probe found a file. Machines without
    // esbuild must skip gracefully, never turn red.
    assert_eq!(classify_failed_lookup(&[]), SkipKind::GenuinelyAbsent);
    assert_eq!(
        classify_failed_lookup(&[
            (
                PathBuf::from("/repo/crates/zfb/binaries/esbuild/esbuild"),
                false
            ),
            (
                PathBuf::from("/stale/worktree/crates/zfb/binaries/esbuild/esbuild"),
                false
            ),
        ]),
        SkipKind::GenuinelyAbsent,
        "absent slot binaries must classify as a graceful skip — the slot dir being \
         non-empty (.gitkeep, README.md) must never matter"
    );
}

#[test]
fn classify_failed_lookup_flags_harness_bug_when_slot_binary_present() {
    // The hermetic encoding of the #1007 incident: a slot path IS a file,
    // yet lookup failed. This must classify as the loud-panic state and
    // carry the offending paths for the diagnostic.
    let present = PathBuf::from("/repo/crates/zfb/binaries/esbuild/esbuild");
    let absent = PathBuf::from("/stale/worktree/crates/zfb/binaries/esbuild/esbuild");
    match classify_failed_lookup(&[(absent, false), (present.clone(), true)]) {
        SkipKind::HarnessBug { present_slots } => {
            assert_eq!(present_slots, vec![present]);
        }
        other => panic!("expected SkipKind::HarnessBug, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Issue #2178 — a staged slot binary that cannot execute on this host.
// ---------------------------------------------------------------------

const STALE_ERROR: &str = "Bad CPU type in executable (os error 86)";

#[test]
fn classify_slot_probes_is_graceful_when_every_slot_is_absent() {
    assert_eq!(classify_slot_probes(&[]), SkipKind::GenuinelyAbsent);
    assert_eq!(
        classify_slot_probes(&[
            probe(
                "/worktree/crates/zfb/binaries/esbuild/esbuild",
                SlotState::Absent
            ),
            probe(
                "/repo/crates/zfb/binaries/esbuild/esbuild",
                SlotState::Absent
            ),
        ]),
        SkipKind::GenuinelyAbsent,
        "machines genuinely without esbuild must still skip gracefully"
    );
}

#[test]
fn classify_slot_probes_flags_stale_binary_when_no_valid_slot_exists() {
    // The #2178 incident, hermetically: the worktree's own slot is absent
    // (gitignored, never staged there) and the enclosing repo's slot holds
    // a wrong-architecture binary. That is NOT "esbuild is absent" — it is
    // an actionable, loud failure naming the stale path.
    let stale = PathBuf::from("/repo/crates/zfb/binaries/esbuild/esbuild");
    match classify_slot_probes(&[
        probe(
            "/worktree/crates/zfb/binaries/esbuild/esbuild",
            SlotState::Absent,
        ),
        SlotProbe {
            path: stale.clone(),
            state: SlotState::Invalid(STALE_ERROR.to_string()),
        },
    ]) {
        SkipKind::InvalidSlots { invalid_slots } => {
            assert_eq!(invalid_slots.len(), 1, "got {invalid_slots:?}");
            assert_eq!(invalid_slots[0].path, stale);
            assert_eq!(
                invalid_slots[0].error, STALE_ERROR,
                "the underlying spawn error must be preserved verbatim, never re-diagnosed"
            );
        }
        other => panic!("expected SkipKind::InvalidSlots, got {other:?}"),
    }
}

#[test]
fn classify_slot_probes_still_flags_harness_bug_when_a_runnable_slot_was_missed() {
    // The #1007 tripwire outranks the #2178 state: if ANY slot binary runs
    // and the lookup still failed, that is a harness bug regardless of how
    // many stale siblings were seen beside it.
    let runnable = PathBuf::from("/repo/crates/zfb/binaries/esbuild/esbuild");
    match classify_slot_probes(&[
        SlotProbe {
            path: PathBuf::from("/worktree/crates/zfb/binaries/esbuild/esbuild"),
            state: SlotState::Invalid(STALE_ERROR.to_string()),
        },
        probe_valid(&runnable),
    ]) {
        SkipKind::HarnessBug { present_slots } => assert_eq!(present_slots, vec![runnable]),
        other => panic!("expected SkipKind::HarnessBug, got {other:?}"),
    }
}

#[test]
fn stale_outer_slot_is_skipped_and_reported_when_a_runnable_fallback_exists() {
    // The whole point of the fix: in a fresh worktree the lookup must keep
    // going past the enclosing repo's unusable slot and find the pnpm-store
    // host-arch binary, so gated tests RUN instead of dying with os error 86.
    let tmp = tempfile::tempdir().expect("tempdir");
    let topology = NestedWorktree::plant(tmp.path());
    let fallback = topology.plant_runnable_flat_pnpm_store();
    let roots = topology.roots();

    let lookup = locate_esbuild_from(&roots, &[]);

    assert_eq!(
        lookup.selected.as_deref(),
        Some(fallback.as_path()),
        "the runnable pnpm-store fallback must be selected; rejected: {:?}",
        lookup.rejected
    );
    assert_eq!(
        lookup
            .rejected
            .iter()
            .map(|rejected| rejected.path.clone())
            .collect::<Vec<_>>(),
        vec![topology.stale_slot.clone()],
        "the skipped stale slot must be reported in the rejected-candidate diagnostics, \
         not silently bypassed"
    );
    assert!(
        !lookup.rejected[0].error.is_empty(),
        "the rejection must carry the underlying spawn error for the diagnostic"
    );

    // The success path resolves to the fallback rather than panicking.
    assert_eq!(
        finish_esbuild_lookup(&roots, lookup).as_deref(),
        Some(fallback.as_path())
    );
}

#[test]
fn stale_outer_slot_with_no_fallback_panics_naming_the_path_error_and_remediation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let topology = NestedWorktree::plant(tmp.path());
    let roots = topology.roots();

    let lookup = locate_esbuild_from(&roots, &[]);
    assert!(
        lookup.selected.is_none(),
        "nothing runnable was planted, so the walk must come up empty"
    );
    let rejection_error = lookup.rejected[0].error.clone();

    // `catch_unwind` rather than `#[should_panic]`: the expected message
    // embeds a tempdir path unknowable at compile time, and all three parts
    // (path, underlying error, remediation) need asserting. The default
    // panic hook is deliberately left in place — swapping it is process-
    // global and would race the other tests in this binary.
    let panicked = std::panic::catch_unwind(|| finish_esbuild_lookup(&roots, lookup))
        .expect_err("a stale slot with no usable fallback must panic, never skip silently");
    let message = panic_message(&*panicked);

    assert!(
        message.contains(&topology.stale_slot.display().to_string()),
        "panic must name the stale path; got: {message}"
    );
    assert!(
        message.contains(&rejection_error),
        "panic must carry the underlying rejection error; got: {message}"
    );
    assert!(
        message.contains("run `cargo check -p zfb` to re-stage"),
        "panic must state the remediation; got: {message}"
    );
}

// --- fixture helpers -------------------------------------------------

fn probe(path: &str, state: SlotState) -> SlotProbe {
    SlotProbe {
        path: PathBuf::from(path),
        state,
    }
}

fn probe_valid(path: &Path) -> SlotProbe {
    SlotProbe {
        path: path.to_path_buf(),
        state: SlotState::Valid,
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

fn esbuild_slot(root: &Path) -> PathBuf {
    root.join("crates/zfb/binaries/esbuild")
        .join(esbuild_binary_name())
}

/// The issue #2178 incident topology, on disk and with no real esbuild: a
/// "worktree" root nested inside an enclosing "main repo" root — both
/// shaped like this workspace (`Cargo.toml` + `crates/`) — where only the
/// OUTER root has a staged esbuild, and that staged file cannot execute.
///
/// Roots are handed to `locate_esbuild_from` directly instead of being
/// derived by moving the process into the fixture: `set_current_dir` is
/// process-global, and mutating it under a multithreaded test binary is
/// exactly the hazard Rust 2024 made unsafe.
struct NestedWorktree {
    worktree_root: PathBuf,
    main_root: PathBuf,
    stale_slot: PathBuf,
}

impl NestedWorktree {
    fn plant(base: &Path) -> Self {
        let main_root = base.join("main-repo");
        let worktree_root = main_root.join("worktrees/topic");
        for root in [&main_root, &worktree_root] {
            std::fs::create_dir_all(root.join("crates")).expect("create crates/");
            std::fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
            // Both roots own the slot DIRECTORY (it is checked in, holding
            // .gitkeep + README.md); only the outer one gets a slot file.
            std::fs::create_dir_all(root.join("crates/zfb/binaries/esbuild"))
                .expect("create slot dir");
            std::fs::write(root.join("crates/zfb/binaries/esbuild/.gitkeep"), "")
                .expect("write .gitkeep");
        }

        // Stands in for the wrong-architecture Mach-O the release flow's
        // local cross-build leaves behind: a slot file that opens fine and
        // fails at EXEC time, which is the property under test.
        //
        // It is deliberately NOT "exec bit set + wrong magic": Apple's
        // `posix_spawnp` (which `std::process::Command` uses) implements
        // execvp's historical ENOEXEC fallback and re-runs the file under
        // `/bin/sh`, so a wrong-magic blob SPAWNS SUCCESSFULLY on macOS
        // (verified: exit 126, `Ok(ExitStatus)`). Denying execute permission
        // fails deterministically everywhere instead. The real incident is
        // unaffected — a wrong-arch Mach-O has valid magic, so it fails with
        // EBADARCH (os error 86) and never reaches the ENOEXEC fallback.
        let stale_slot = esbuild_slot(&main_root);
        std::fs::write(&stale_slot, b"\x00\x01not a real binary\n").expect("write stale slot");
        deny_execute(&stale_slot);

        Self {
            worktree_root,
            main_root,
            stale_slot,
        }
    }

    /// Candidate roots in the order `candidate_workspace_roots()` produces
    /// for a test running inside the worktree: the worktree root first, the
    /// enclosing main repo root after it.
    fn roots(&self) -> Vec<PathBuf> {
        vec![self.worktree_root.clone(), self.main_root.clone()]
    }

    /// Plant a host-runnable binary at the outer root's flat pnpm-store
    /// location — the tier a `pnpm install`ed checkout really hits.
    ///
    /// The fixture is a copy of this test binary rather than a shell script:
    /// it is runnable on every platform the lookup supports, and libtest
    /// rejects `--version` with a fast non-zero exit (which the probe
    /// deliberately accepts — it asks "can this execute", not "what version").
    fn plant_runnable_flat_pnpm_store(&self) -> PathBuf {
        let dir = self
            .main_root
            .join("node_modules/.pnpm/node_modules/esbuild/bin");
        std::fs::create_dir_all(&dir).expect("create flat pnpm store dir");
        let fake = dir.join(esbuild_binary_name());
        let self_exe = std::env::current_exe().expect("current_exe");
        std::fs::copy(&self_exe, &fake).expect("copy test binary into the fake store");
        make_executable(&fake);
        fake
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("set exec permissions");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {
    // Windows keys executability off the file extension, not a mode bit.
}

#[cfg(unix)]
fn deny_execute(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
        .expect("clear exec permissions");
}

#[cfg(not(unix))]
fn deny_execute(_path: &Path) {
    // Windows has no exec bit; the `.exe` written here carries garbage
    // rather than a PE header, so CreateProcess fails with
    // ERROR_BAD_EXE_FORMAT (os error 193) — the same "cannot execute" class.
}
