//! Integration test for `zfb check`.
//!
//! Spawns the built `zfb` binary as a subprocess against two fixture
//! projects (one passing, one with a deliberate schema violation) and
//! asserts the command exits 0/1 and prints the expected violation.
//!
//! tsc is not invoked — the fixtures pass `--skip-tsc` so the test
//! does NOT depend on a TypeScript install. Schema-only is the
//! interesting axis here; the tsc subprocess is exercised by manual
//! smoke tests.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn zfb_binary() -> PathBuf {
    // `CARGO_BIN_EXE_<bin>` is set by Cargo when building integration
    // tests for the same package — the value is the absolute path to
    // the built binary, no `cargo run` overhead.
    PathBuf::from(env!("CARGO_BIN_EXE_zfb"))
}

#[test]
fn check_passes_on_valid_fixture() {
    let dir = fixture("check-good");
    let output = Command::new(zfb_binary())
        .args(["check", "--skip-tsc"])
        .current_dir(&dir)
        .output()
        .expect("spawn zfb");
    assert!(
        output.status.success(),
        "expected success, got status={:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("no errors"),
        "expected success message, got:\n{combined}",
    );
}

#[test]
fn check_fails_with_schema_violation() {
    let dir = fixture("check-bad-schema");
    let output = Command::new(zfb_binary())
        .args(["check", "--skip-tsc"])
        .current_dir(&dir)
        .output()
        .expect("spawn zfb");
    assert!(
        !output.status.success(),
        "expected non-zero exit, got status={:?}",
        output.status,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");
    // The bad entry violates `sidebar_position: number` — error text
    // must name the file, the field, and the type mismatch.
    assert!(
        combined.contains("bad.md"),
        "expected file path in error output:\n{combined}",
    );
    assert!(
        combined.contains("sidebar_position"),
        "expected field name in error output:\n{combined}",
    );
    assert!(
        combined.contains("expected number"),
        "expected type mismatch text in error output:\n{combined}",
    );
    // The summary line names the failure category.
    assert!(
        combined.contains("schema violation"),
        "expected summary line:\n{combined}",
    );
}
