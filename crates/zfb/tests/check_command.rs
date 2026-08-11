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

use zfb_test_utils::{locate_esbuild, zfb_binary};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn combined_output(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
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

// ---------------------------------------------------------------------------
// SSR route-contract guard confirmation (#2356, epic #2351)
//
// End-to-end confirmation against the real `zfb` binary that `zfb check`
// and `zfb build` behave per the epic's compatibility contract, WITH a
// negative control: a detector that fired on every `prerender = false`
// route would satisfy every positive assertion below and be worse than no
// detector at all, so the negative fixture is asserted just as hard as the
// positive one.
//
// Fixtures:
// - `ssr-request-param-positive/pages/api/submit.tsx` is the exact broken
//   shape #2350 reported: `export default async function Handler(request:
//   Request): Promise<Response>` on a `prerender = false` route.
// - `ssr-request-param-negative/` carries two routes that must never fire
//   the gate: a correct zero-parameter API handler, and a correct dynamic
//   route (`[slug].tsx`) whose handler destructures `{ params }`.
//
// `zfb check` is exercised directly against the checked-in fixture
// (`--skip-tsc`, no build needed — matches the other tests in this file).
// `zfb build` additionally needs an adapter (any `prerender = false` route
// fails the build's own "no adapter configured" precondition otherwise,
// independent of this detector) and a real esbuild pass, so those two
// tests copy the fixture into a tempdir and scaffold a minimal stand-in
// adapter plus the embedded runtime's `node_modules` — see
// `scaffold_build_fixture` below.
// ---------------------------------------------------------------------------

#[test]
fn ssr_request_param_positive_check_fails_and_names_offender() {
    let dir = fixture("ssr-request-param-positive");
    let output = Command::new(zfb_binary!())
        .args(["check", "--skip-tsc"])
        .current_dir(&dir)
        .output()
        .expect("spawn zfb");
    assert!(
        !output.status.success(),
        "expected non-zero exit for the broken (request: Request) shape, got status={:?}\n{}",
        output.status,
        combined_output(&output),
    );
    let combined = combined_output(&output);
    assert!(
        combined.contains("api/submit.tsx"),
        "expected the offending file to be named:\n{combined}",
    );
    assert!(
        combined.contains("/api/submit"),
        "expected the offending route to be named:\n{combined}",
    );
    assert!(
        combined.contains("SSR route contract violation"),
        "expected the check summary to label this its own finding kind:\n{combined}",
    );
}

/// The negative control. Correct code — a zero-parameter handler and a
/// destructured `{ params }` handler, both on `prerender = false` routes —
/// must produce no diagnostic and a clean exit. This is as important as the
/// positive test above: a detector that fires on every SSR route would pass
/// the positive test too.
#[test]
fn ssr_request_param_negative_check_is_silent() {
    let dir = fixture("ssr-request-param-negative");
    let output = Command::new(zfb_binary!())
        .args(["check", "--skip-tsc"])
        .current_dir(&dir)
        .output()
        .expect("spawn zfb");
    let combined = combined_output(&output);
    assert!(
        output.status.success(),
        "expected success for correctly-shaped SSR handlers, got status={:?}\n{combined}",
        output.status,
    );
    assert!(
        !combined.contains("SSR route contract"),
        "correct handlers must not trip the detector:\n{combined}",
    );
    assert!(
        combined.contains("no errors"),
        "expected the normal success message, got:\n{combined}",
    );
}

/// Copy `src` into `dst` recursively. Used to scaffold a real,
/// buildable copy of a checked-in fixture into a tempdir (the build tests
/// below add a `node_modules` symlink and a `package.json` alongside the
/// copy, which must not touch the checked-in fixture).
#[cfg(unix)]
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// `pnpm exec` (the adapter dispatch mechanism, `crates/zfb-build/src/adapter.rs`)
/// needs a real `pnpm` on `PATH`.
#[cfg(unix)]
fn pnpm_available() -> bool {
    Command::new("pnpm")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Copy the named fixture into a fresh tempdir and wire it up to be a real,
/// buildable project:
///
/// - The embedded `node_modules` snapshot (`@takazudo/zfb-runtime`, `preact`,
///   …) is extracted and symlinked into the copy's root — a project-level
///   `node_modules` directory (needed below for `pnpm exec` resolution)
///   otherwise shadows zfb's embedded-vendor fallback and esbuild can no
///   longer resolve the runtime imports (confirmed by hand while building
///   this fixture: with a bare, empty `node_modules/.bin/` only, `zfb build`
///   fails with `Could not resolve "@takazudo/zfb-runtime/server"` etc.).
/// - A minimal stand-in adapter is written to `node_modules/.bin/fake-adapter`.
///   It only needs to satisfy the `<bin> bundle <input> --outdir <dir>
///   [--asset <path>]...` CLI contract by exiting 0 — this confirmation
///   exercises `zfb build`'s own SSR route-contract warning and exit code,
///   not any real adapter's deploy output shape, so the real
///   `@takazudo/zfb-adapter-cloudflare` package (Wrangler tooling, real Wasm
///   passes) would be needlessly heavy here.
/// - A `package.json` is written at the copy's root because `pnpm exec`
///   refuses to run at all ("no package found in this workspace") without
///   one — confirmed by hand; it needs no `dependencies`/`bin` field since
///   `pnpm exec` resolves directly from `node_modules/.bin/`.
///
/// Returns `(scaffold_tempdir, node_modules_tempdir, project_root)` — both
/// tempdirs must stay alive for the duration of the `zfb build` invocation.
#[cfg(unix)]
fn scaffold_build_fixture(name: &str) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let scaffold = tempfile::tempdir().expect("create scaffold tempdir");
    let root = scaffold.path().join("project");
    copy_dir_recursive(&fixture(name), &root).expect("copy fixture into scaffold tempdir");

    let (node_modules_handle, node_modules) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let bin_dir = node_modules.join(".bin");
    std::fs::create_dir_all(&bin_dir).expect("create node_modules/.bin");
    let adapter_bin = bin_dir.join("fake-adapter");
    std::fs::write(
        &adapter_bin,
        "#!/bin/sh\n\
         # Minimal stand-in adapter for check_command.rs's SSR route-contract\n\
         # guard confirmation (#2356). Only needs to succeed: this test\n\
         # exercises zfb build's own warning + exit code, not a real\n\
         # adapter's deploy output.\n\
         exit 0\n",
    )
    .expect("write fake-adapter stub");
    let mut perms = std::fs::metadata(&adapter_bin)
        .expect("stat fake-adapter stub")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&adapter_bin, perms).expect("chmod +x fake-adapter stub");

    std::fs::write(
        root.join("package.json"),
        "{\"name\":\"ssr-route-contract-guard-fixture\",\"version\":\"0.0.0\",\"private\":true}\n",
    )
    .expect("write package.json");

    std::os::unix::fs::symlink(&node_modules, root.join("node_modules"))
        .expect("symlink node_modules into scaffolded project root");

    (scaffold, node_modules_handle, root)
}

/// The epic's compatibility guarantee (#2351): `zfb build` on the broken
/// `(request: Request)` shape must still SUCCEED — the finding is a
/// warning, never a build failure, or existing projects break on upgrade.
/// Asserts the exit code explicitly, not just the warning text.
#[test]
#[cfg(unix)]
fn ssr_request_param_positive_build_warns_and_succeeds() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[check_command] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !pnpm_available() {
        eprintln!("[check_command] pnpm is not available; skipping.");
        return;
    }

    let (_scaffold, _node_modules, root) = scaffold_build_fixture("ssr-request-param-positive");
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn zfb build");
    let combined = combined_output(&output);
    assert!(
        output.status.success(),
        "zfb build must succeed on the broken SSR handler shape (warn-only, per the epic's \
         compatibility guarantee), got status={:?}\n{combined}",
        output.status,
    );
    assert!(
        combined.contains("/api/submit"),
        "expected the build's warning to name the offending route:\n{combined}",
    );
    assert!(
        combined.contains("api/submit.tsx"),
        "expected the build's warning to name the offending file:\n{combined}",
    );
}

/// The negative control for `zfb build`, paired with the positive test
/// above: correctly-shaped handlers on `prerender = false` routes must
/// build clean and silent with respect to this diagnostic.
#[test]
#[cfg(unix)]
fn ssr_request_param_negative_build_is_silent_and_succeeds() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[check_command] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !pnpm_available() {
        eprintln!("[check_command] pnpm is not available; skipping.");
        return;
    }

    let (_scaffold, _node_modules, root) = scaffold_build_fixture("ssr-request-param-negative");
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn zfb build");
    let combined = combined_output(&output);
    assert!(
        output.status.success(),
        "zfb build must succeed for correctly-shaped SSR handlers, got status={:?}\n{combined}",
        output.status,
    );
    assert!(
        !combined.contains("SSR route contract"),
        "correct handlers must not trip the detector's warning:\n{combined}",
    );
}
