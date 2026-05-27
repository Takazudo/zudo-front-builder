//! Compile-time + runtime sanity check for `locate_esbuild()`.
//!
//! Verifies that:
//! 1. The function compiles and links correctly.
//! 2. It does not panic under any circumstances (returning `None` is fine).
//!
//! This test does NOT check for the macro — `CARGO_BIN_EXE_zfb` is only
//! set when compiling integration tests for the `zfb` binary crate; using
//! `zfb_binary!()` here would cause a compile error.

use zfb_test_utils::locate_esbuild;

#[test]
fn locate_esbuild_does_not_panic() {
    // Returning None is perfectly valid — the test environment may not have
    // esbuild installed. We only assert the call completes without panicking.
    let _result = locate_esbuild();
    // If we reach here, the function ran successfully.
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
