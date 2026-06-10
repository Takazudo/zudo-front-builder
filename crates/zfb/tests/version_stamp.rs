//! Integration test for the ZFB_RELEASE_VERSION compile-time version stamp.
//!
//! `option_env!("ZFB_RELEASE_VERSION")` is resolved at compile time, not
//! runtime, so we cannot simply spawn the already-built binary with the env
//! var set and expect a different version string. Instead, this test invokes
//! `cargo run` as a subprocess so a fresh compilation picks up the env var.
//!
//! The test is marked `#[ignore]` because it performs a full Rust recompile,
//! which is slow. Run it explicitly with:
//!   cargo test --package zfb --test version_stamp -- --ignored
//! or
//!   ZFB_RELEASE_VERSION=0.99.0-test cargo test --package zfb --test version_stamp -- --ignored

use std::process::Command;

/// Compile and run `zfb --version` with `ZFB_RELEASE_VERSION` set to a
/// well-known test value and assert the output reflects that value.
///
/// Uses an isolated `CARGO_TARGET_DIR` to avoid holding the parent build's
/// target-dir lock and to prevent stomping the parent's compiled artifacts.
#[test]
#[ignore = "performs a full Rust recompile; run with --ignored"]
fn version_stamp_from_env() {
    let test_version = "0.99.0-test";

    // Use a temporary target dir so this sub-build does not collide with the
    // parent cargo invocation's file lock on the shared target directory.
    let tmp = tempfile::tempdir().expect("failed to create temp dir for target isolation");
    let isolated_target = tmp.path().join("target");

    // env!("CARGO") is the path to the cargo binary that invoked this test,
    // guaranteed to exist and match the toolchain in use.
    let output = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--package", "zfb", "--", "--version"])
        .env("ZFB_RELEASE_VERSION", test_version)
        .env("CARGO_TARGET_DIR", &isolated_target)
        // Suppress build.rs binary downloads: binaries already staged in the
        // workspace-relative crates/zfb/binaries/ directory are reused, but
        // setting these escape hatches prevents build.rs from attempting a
        // network fetch if the staged binaries are missing in CI.
        .env("ZFB_ESBUILD_BIN", "skip")
        .env("ZFB_TAILWIND_BIN", "skip")
        .output()
        .expect("failed to spawn cargo run for version_stamp test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "cargo run failed\nstdout: {stdout}\nstderr: {stderr}"
    );

    let expected = format!("zfb {test_version}");
    assert!(
        stdout.trim().contains(&expected),
        "expected --version to contain '{expected}', got: '{}'",
        stdout.trim()
    );
}
