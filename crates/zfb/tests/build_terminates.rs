//! End-to-end watchdog — assert that `zfb build` terminates.
//!
//! ## Why this test exists
//!
//! No existing test asserts that `zfb build` actually returns to the caller.
//! This watchdog drives the real `zfb` binary against an adapter + SSR-route
//! fixture and FAILs if the process does not exit within a wall-clock deadline.
//!
//! The discriminating bugs fixed in wave 1 (#651/#652/#653) were:
//!
//! - **Candidate A (output() hang):** `run_capturing` in `adapter.rs` called
//!   `Command::output()`, which blocks until every write-end of stdout/stderr
//!   pipes is closed.  The stub adapter intentionally detaches a grandchild
//!   process that keeps those inherited fds open.  Before fix #651 the build
//!   hung at the `wait_stdout_to_eof` point indefinitely.  After the fix
//!   (redirect stdout/stderr to temp files + `wait()` on the direct child only)
//!   the build exits the moment the direct child exits, regardless of the
//!   grandchild.
//!
//! - **Candidate B (V8 Drop hang):** The embedded V8 host was not joined
//!   before the process exited, leading to a hang in the V8 destructor (#652).
//!
//! - **Candidate C (postBuild timeout):** A pathological postBuild plugin
//!   could block the build indefinitely (#653).
//!
//! ## Why process-group kill is required
//!
//! The fixture adapter script deliberately detaches a grandchild (`sleep 60`)
//! that outlives the adapter process.  On timeout, killing only the direct
//! `zfb` child would leave the grandchild alive, and the test process itself
//! would stay hung waiting for that grandchild's inherited fds to close.
//! `kill(-pgid, SIGKILL)` reaps the **entire process group** — the only
//! reliable exit path when an arbitrary grandchild may be holding fds open.
//!
//! The same group-kill is issued after a successful exit to collect the
//! deliberately-lingering sleep grandchild so it does not leak into CI.
//!
//! ## Fixture shape
//!
//! Scaffolded inline (no on-disk fixture directory):
//!
//! - `zfb.config.json` — `adapter: "zfb-adapter-stub"`, `framework: "preact"`.
//! - `pages/index.tsx` — `export const prerender = false` (SSR route), renders
//!   a minimal HTML page.  This is enough to trigger both the V8-on path
//!   (build.rs decides V8-on when the prerender-false set is non-empty) and the
//!   adapter dispatch (build.rs:1739).
//! - `node_modules/` — symlinked to the binary-embedded `@takazudo` packages
//!   (same technique used by `css_modules_components_build.rs`'s
//!   `corp_shape_with_real_node_modules_and_no_tsconfig_paths_builds`).
//!   This allows `detect_project_node_modules` to resolve the framework packages
//!   while still giving us a writable `node_modules/.bin/` for the stub adapter.
//! - `node_modules/.bin/zfb-adapter-stub` — a `#!/bin/sh` script that writes
//!   `dist/_worker.js`, detaches a `sleep 60` grandchild holding the inherited
//!   stdout/stderr fds, then exits 0.  This is the exact shape Candidate A
//!   was sensitive to.
//! - `node_modules/zfb-adapter-stub/package.json` — declares the `bin` entry
//!   so `pnpm exec zfb-adapter-stub` resolves the script.
//! - `package.json` at the fixture root — required by `pnpm exec` to
//!   identify the local workspace root and discover `node_modules/.bin/`.
//!
//! Note: because the project has a real `node_modules/` present,
//! `detect_project_node_modules` returns `Some(...)` and the build uses it for
//! framework-package resolution (no embedded_node_modules() extraction needed).
//! The stub adapter entry lives within that same tree.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Wall-clock deadline for the build.
///
/// 120 seconds is generous: a clean build with V8 + esbuild takes ~10-20 s on
/// a loaded dev machine. A 120 s overrun is unambiguous evidence of a hang.
const BUILD_TIMEOUT: Duration = Duration::from_secs(120);

/// Scaffold the adapter+SSR-route fixture into `root`.
///
/// Returns a `tempfile::TempDir` handle for the extracted embedded node_modules
/// tree — it must be kept alive for the duration of the test run.
///
/// # Fixture structure
///
/// - `zfb.config.json` — adapter + preact framework.
/// - `pages/index.tsx` — `export const prerender = false` (SSR route).
/// - `node_modules/` — symlinked to the extracted embedded `@takazudo` tree so
///   `detect_project_node_modules` returns `Some(...)` and esbuild can resolve
///   `@takazudo/zfb-runtime`, `preact`, etc.
/// - `node_modules/.bin/zfb-adapter-stub` (mode 0o755) — stub adapter bin that
///   writes `dist/_worker.js` and detaches a `sleep 60` grandchild.
/// - `node_modules/zfb-adapter-stub/package.json` — declares the bin entry.
fn scaffold_fixture(root: &std::path::Path) -> tempfile::TempDir {
    // Config: adapter + preact framework.
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "adapter": "zfb-adapter-stub" }
"#,
    )
    .unwrap();

    // package.json at project root — required by `pnpm exec` to locate the
    // local node_modules/.bin when run from this directory.
    fs::write(
        root.join("package.json"),
        r#"{ "name": "build-terminates-fixture", "version": "0.0.1", "private": true }
"#,
    )
    .unwrap();

    // SSR route: prerender = false so the build enters V8-on mode and runs
    // the adapter dispatch step (build.rs:1739).
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export const prerender = false;

export default function Page() {
  return (
    <html lang="en">
      <head><title>build-terminates fixture</title></head>
      <body><p>hello</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // Extract the binary-embedded @takazudo packages into a tempdir, then
    // symlink node_modules/ into the fixture root pointing at that tree.
    // This mirrors the technique in css_modules_components_build.rs's
    // `corp_shape_with_real_node_modules_and_no_tsconfig_paths_builds`.
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

    // Stub adapter bin.
    //
    // The script:
    //   1. Creates the outdir and writes a minimal dist/_worker.js so zfb does
    //      not reject the adapter output.
    //   2. Spawns a `sleep 60` grandchild that inherits stdout/stderr, then
    //      immediately detaches it (& in sh) and exits 0.
    //
    // This is the exact shape Candidate A (#651) was sensitive to: a grandchild
    // holds the inherited pipe write-ends open, so Command::output() would block
    // until that grandchild exits (~60 s later).  The fixed run_capturing() uses
    // temp files + wait() on the direct child only, so the grandchild is
    // irrelevant to the build's exit.
    //
    // Note: the symlinked node_modules/ is read-only (embedded tree), so we
    // create the .bin and stub package entries through the symlink target, which
    // works because embedded_nm_path itself is a freshly-extracted tempdir we own.
    let bin_dir = embedded_nm_path.join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let bin_path = bin_dir.join("zfb-adapter-stub");
    // argv from DefaultAdapterRunner: bundle <input> --outdir <outdir>
    //   $1 = "bundle"  $2 = <input>  $3 = "--outdir"  $4 = <outdir>
    fs::write(
        &bin_path,
        r#"#!/bin/sh
# stub adapter for build_terminates e2e (#654)
# argv: bundle <input> --outdir <outdir>
OUTDIR="$4"
mkdir -p "$OUTDIR"
printf 'export default { fetch(r){return new Response("ok")} }\n' > "$OUTDIR/_worker.js"
# Written for fidelity with the real adapter's Workers Static Assets output
# shape; the test only asserts the build terminates, not on filenames.
printf '_worker.js\n' > "$OUTDIR/.assetsignore"
# Detach a grandchild that holds inherited stdout/stderr fds open.
# Pre-fix code (Command::output()) would hang here waiting for this fd to close.
# Fixed code (temp-file redirect + wait-on-direct-child) is unaffected.
sleep 60 &
exit 0
"#,
    )
    .unwrap();
    fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755)).unwrap();

    // Package metadata so pnpm exec can resolve the bin name.
    let pkg_dir = embedded_nm_path.join("zfb-adapter-stub");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("package.json"),
        r#"{
  "name": "zfb-adapter-stub",
  "version": "0.0.1",
  "bin": {
    "zfb-adapter-stub": "../.bin/zfb-adapter-stub"
  }
}
"#,
    )
    .unwrap();

    nm_handle
}

/// Check whether `pnpm` is available on PATH without spawning a shell.
fn pnpm_available() -> bool {
    Command::new("pnpm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Assert that `zfb build` exits within [`BUILD_TIMEOUT`] on an adapter +
/// SSR-route fixture that deliberately detaches a grandchild holding the
/// inherited stdout/stderr fds open.
///
/// The test fails (not hangs) on a regression because the watchdog kills the
/// **entire process group** on timeout — the grandchild is in the same group
/// and is collected together with the zfb build process.
#[test]
fn zfb_build_terminates_with_adapter_and_ssr_route() {
    // --- skip-guards ---

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_terminates] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    if !pnpm_available() {
        eprintln!("[build_terminates] pnpm not found on PATH; skipping.");
        return;
    }

    // --- fixture ---

    let tmp = tempfile::tempdir().expect("create tempdir for build_terminates fixture");
    let root = tmp.path().to_path_buf();
    // _nm_handle must stay alive for the duration of the test run so the
    // embedded node_modules tempdir is not cleaned up before zfb build finishes.
    let _nm_handle = scaffold_fixture(&root);

    // --- spawn zfb build in its own process group ---
    //
    // pre_exec runs after fork() but before execvp() in the child, so it
    // inherits the child PID.  setpgid(0, 0) moves the child into a new
    // process group (PGID == child PID).  Any grandchild the adapter spawns
    // inherits that PGID, so kill(-pgid, SIGKILL) reaps the whole tree.
    //
    // stdout/stderr are redirected to temp files rather than pipes: this
    // watchdog only drains output *after* the child exits, so a piped child
    // that wrote more than the ~64KB OS pipe buffer would block on write and
    // masquerade as a hang — on the very test meant to certify termination.
    // Files have no backpressure; we read them back for the failure message.
    let stdout_path = root.join(".zfb-build-stdout.log");
    let stderr_path = root.join(".zfb-build-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("build")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    // Safety: setpgid(0, 0) is async-signal-safe (POSIX).
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn().expect("spawn `zfb build`");
    let pgid = child.id() as libc::pid_t; // PGID == child PID after setpgid(0,0)

    // Read a captured output file back (best-effort) for assertion messages.
    let read_log = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();

    // --- watchdog loop ---

    let start = Instant::now();
    let exit_status = loop {
        match child.try_wait().expect("try_wait on `zfb build`") {
            Some(status) => break status,
            None => {
                if start.elapsed() > BUILD_TIMEOUT {
                    // Kill the entire process group so neither the direct child
                    // nor any grandchild (e.g. the detached `sleep 60`) is left
                    // alive to hold fds open.
                    unsafe {
                        libc::kill(-pgid, libc::SIGKILL);
                    }
                    let _ = child.wait();

                    panic!(
                        "`zfb build` did not exit within {}s — this indicates a hang. \
                         Process group {} was killed.\n\
                         --- stdout ---\n{}\n--- stderr ---\n{}",
                        BUILD_TIMEOUT.as_secs(),
                        pgid,
                        read_log(&stdout_path),
                        read_log(&stderr_path),
                    );
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    };

    // --- reap the process group after success ---
    //
    // The stub adapter leaves a detached `sleep 60` grandchild alive in the
    // same process group.  Kill the group now so it does not leak into CI.
    // This is best-effort: if the grandchild has already exited the signal is
    // harmless (ESRCH).
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }

    assert!(
        exit_status.success(),
        "`zfb build` exited non-zero — adapter+SSR-route fixture should build cleanly.\n\
         status: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        exit_status,
        read_log(&stdout_path),
        read_log(&stderr_path),
    );
}
