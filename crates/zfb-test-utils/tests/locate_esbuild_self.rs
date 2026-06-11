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

use std::path::PathBuf;

use zfb_test_utils::{candidate_workspace_roots, classify_failed_lookup, locate_esbuild, SkipKind};

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
