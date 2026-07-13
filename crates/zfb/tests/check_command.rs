//! Integration test for `zfb check`.
//!
//! Spawns the built `zfb` binary as a subprocess against fixture projects,
//! including passing paths and deliberate schema and TypeScript failures, and
//! asserts the expected statuses and diagnostics.
//!
//! Most fixtures pass `--skip-tsc` so schema validation can run without a
//! TypeScript installation. The deterministic failure fixture and the Wasm
//! fixture exercise the tsc subprocess directly; the latter uses the real
//! compiler.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use zfb_test_utils::zfb_binary;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn check_passes_on_valid_fixture() {
    let dir = fixture("check-good");
    let output = Command::new(zfb_binary!())
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
    let output = Command::new(zfb_binary!())
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

/// Exercise the tsc failure-folding path in `zfb check`.
///
/// All existing tests pass `--skip-tsc`, so this is the only test that
/// drives the tsc subprocess code path to a non-zero exit.
///
/// Fixture: `check-tsc-fail/` contains a committed shell stub at
/// `node_modules/.bin/tsc` that always exits 1, matching the tsc
/// convention for "type errors found". The stub is deterministic and
/// runs in any POSIX shell (CI uses Ubuntu/Linux). Schema validation
/// passes (the content is valid) so the only failure is tsc.
///
/// Rationale: this exercises `run_tsc → locate_tsc` (which prefers
/// `node_modules/.bin/tsc`) and the `tsc_failed = true` branch that
/// folds tsc failure into the check summary.
#[test]
fn check_fails_when_tsc_exits_nonzero() {
    let dir = fixture("check-tsc-fail");

    // Sanity: the stub must exist and be executable.
    let stub = dir.join("node_modules/.bin/tsc");
    assert!(
        stub.exists(),
        "tsc stub must exist at {}: run `git submodule update` or check that \
         the fixture was committed correctly",
        stub.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&stub).unwrap().permissions();
        assert!(
            perms.mode() & 0o111 != 0,
            "tsc stub at {} must be executable; fix permissions in the fixture",
            stub.display()
        );
    }

    let output = Command::new(zfb_binary!())
        .args(["check"])
        // No --skip-tsc: we want the tsc subprocess to run.
        .current_dir(&dir)
        .output()
        .expect("spawn zfb");

    assert!(
        !output.status.success(),
        "expected non-zero exit when tsc exits 1, got status={:?}",
        output.status,
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    // `zfb check` must report a tsc-related failure in its summary.
    // `render_summary(0, true)` produces "check failed: type errors".
    assert!(
        combined.contains("type errors") || combined.contains("check failed"),
        "expected 'type errors' or 'check failed' in output:\n{combined}",
    );
}

/// Prove that the SDK's ambient `*.wasm` declaration reaches a normal project
/// through its root entry point. This invokes the real TypeScript compiler;
/// the fixture rejects `any` and requires the imported default to be exactly
/// `WebAssembly.Module`.
#[test]
fn check_passes_with_sdk_wasm_import_types() {
    let dir = fixture("check-wasm-import");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/zfb must have a repository root")
        .to_path_buf();
    let typescript_bin_dir = repo_root.join("packages/zfb/node_modules/.bin");
    assert!(
        typescript_bin_dir.exists(),
        "TypeScript must be installed at {}; run pnpm install first",
        typescript_bin_dir.display(),
    );
    let mut path_entries = vec![typescript_bin_dir];
    if let Some(existing) = env::var_os("PATH") {
        path_entries.extend(env::split_paths(&existing));
    }
    let path = env::join_paths(path_entries)
        .expect("TypeScript bin directory and inherited PATH entries must be valid");

    let output = Command::new(zfb_binary!())
        .arg("check")
        .current_dir(&dir)
        .env("PATH", path)
        .output()
        .expect("spawn zfb");
    assert!(
        output.status.success(),
        "expected Wasm fixture to typecheck, got status={:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("no errors"),
        "expected success message, got:\n{combined}",
    );
}
