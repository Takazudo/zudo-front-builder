//! Cheap CLI contract tests for embedded toolchain version reporting.
//!
//! These tests spawn the already-built binary via `zfb_binary!()` so they do
//! not recompile zfb or boot its V8/esbuild build pipeline. The heavy
//! `version_stamp` test separately verifies the release-version environment
//! override with an isolated `cargo run`.

use std::process::Command;

use zfb_test_utils::zfb_binary;
use zfb_toolchain_pins::{EXPECTED_ESBUILD_VERSION, EXPECTED_TAILWIND_VERSION};

#[test]
fn long_version_reports_embedded_toolchain_and_short_version_stays_single_line() {
    let long = Command::new(zfb_binary!())
        .arg("--version")
        .output()
        .expect("spawn zfb --version");
    assert!(
        long.status.success(),
        "zfb --version failed with status {:?}\nstdout: {}\nstderr: {}",
        long.status,
        String::from_utf8_lossy(&long.stdout),
        String::from_utf8_lossy(&long.stderr),
    );
    let long_stdout = String::from_utf8_lossy(&long.stdout);
    assert!(
        long_stdout.contains("zfb "),
        "long version must retain the zfb version prefix, got: {long_stdout:?}"
    );
    assert!(
        long_stdout.contains(EXPECTED_TAILWIND_VERSION),
        "long version must report embedded Tailwind CSS {EXPECTED_TAILWIND_VERSION}, got: {long_stdout:?}"
    );
    assert!(
        long_stdout.contains(EXPECTED_ESBUILD_VERSION),
        "long version must report embedded esbuild {EXPECTED_ESBUILD_VERSION}, got: {long_stdout:?}"
    );
    assert!(
        long_stdout.lines().count() >= 3,
        "long version must be multi-line, got: {long_stdout:?}"
    );

    let short = Command::new(zfb_binary!())
        .arg("-V")
        .output()
        .expect("spawn zfb -V");
    assert!(
        short.status.success(),
        "zfb -V failed with status {:?}\nstdout: {}\nstderr: {}",
        short.status,
        String::from_utf8_lossy(&short.stdout),
        String::from_utf8_lossy(&short.stderr),
    );
    let short_stdout = String::from_utf8_lossy(&short.stdout);
    assert_eq!(
        short_stdout.lines().count(),
        1,
        "short version must stay single-line, got: {short_stdout:?}"
    );
    assert!(
        short_stdout.starts_with("zfb "),
        "short version must retain the zfb version prefix, got: {short_stdout:?}"
    );
}
