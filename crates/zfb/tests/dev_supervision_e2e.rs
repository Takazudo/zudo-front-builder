//! Dev Supervision E2E (issue #2101, Dev Supervision epic #2099 Sub #2101,
//! depends on #2100's fault-injection knobs; flipped green by #2102, the
//! supervision-link sub; confirmed by #2105, the epic's closing sub).
//!
//! ## Why this test exists
//!
//! `zfb dev`'s orchestrator drain loop (`BuildOrchestrator::run_with_boot`)
//! used to run inside a fire-and-forget `tokio::spawn` (`boot_handle` in
//! `crates/zfb/src/commands/dev.rs`) that nothing ever joined or awaited
//! for its outcome — only `boot_handle.abort()` was called, on the OTHER
//! branch of the `tokio::select!` (when the HTTP server itself stops). If
//! that task panicked or silently returned, the process noticed nothing:
//! the HTTP listener stayed open and kept answering 200s with stale
//! content forever. This file is the regression net proving the dev
//! process actually goes down when its watcher/orchestrator task dies,
//! using the deterministic fault knobs #2100 added
//! (`ZFB_DEV_TEST_ORCH_PANIC_ON_TICK`, `ZFB_DEV_TEST_ORCH_STOP_MS`) instead
//! of a real (unreliable, hard-to-trigger-on-demand) production bug.
//!
//! ## Flipped green by #2102 — assertions unchanged
//!
//! Both assertions below were written in DESIRED POST-FIX form from the
//! start and are byte-identical to how #2101 wrote them — flipping (this
//! sub, #2105) only removed the `#[ignore]` attribute and updated
//! names/comments, per zudo-test-wisdom's flip protocol. The contract:
//! a dead orchestrator task takes the whole `zfb dev` process down
//! (non-zero exit, listener actually dropped) with a stderr message
//! naming the dead task — delivered by #2102's supervision `select!` arm
//! (`res = &mut boot_handle`, `crates/zfb/src/commands/dev.rs`).
//!
//! **Verified RED before #2102** (2026-07, #2101): with either fault
//! armed and no supervision link in place, the fault's own `fault fired:`
//! marker line (see #2100's doc comments in
//! `crates/zfb-build/src/orchestrator.rs` for exact wording) reliably
//! appeared in captured stderr — proving the watcher delivered the event
//! and the fault seam actually fired — yet `GET /` against the still-live
//! port kept answering 200, and the child process never exited within the
//! generous deadline below.
//!
//! **Revert-proof, performed by #2105** (2026-07): with #2102's
//! supervision arm temporarily commented out in
//! `crates/zfb/src/commands/dev.rs` (the `res = &mut boot_handle => { ... }`
//! `select!` arm), both tests below reproduced the EXACT pre-fix RED
//! symptom in this same harness — the fault's `fault fired:` marker
//! appeared in captured stderr, `GET /` kept answering 200, and the
//! process never exited within `FAULT_EXIT_DEADLINE`, so `wait_for_exit`
//! timed out and the diagnostic panic fired. The arm was then restored and
//! `git diff` confirmed clean before committing. Captured evidence from
//! that run (2026-07-28, `ZFB_ESBUILD_BIN=<path> cargo test -p zfb --test
//! dev_supervision_e2e -- --test-threads=1`, arm disabled):
//!
//! ```text
//! test orch_panic_on_tick_takes_the_whole_process_down ... FAILED
//! thread '...' panicked at crates/zfb/tests/dev_supervision_e2e.rs:...:
//! process never exited within the deadline.
//! live-server probe: manual GET http://localhost:PORT/ still answered
//! status 200 (first 200 bytes: "<!doctype html>...<h1>dev-loop-basic</h1>...")
//! fault fired marker ("[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK")
//! observed in captured output: true
//!
//! test orch_stop_ms_takes_the_whole_process_down ... FAILED
//! thread '...' panicked at crates/zfb/tests/dev_supervision_e2e.rs:...:
//! process never exited within the deadline.
//! live-server probe: manual GET http://localhost:PORT/ still answered
//! status 200 (first 200 bytes: "<!doctype html>...<h1>dev-loop-basic</h1>...")
//! fault fired marker ("[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_STOP_MS")
//! observed in captured output: true
//!
//! test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
//! ```
//!
//! With the arm restored (`git diff` on `crates/zfb/src/commands/dev.rs`
//! confirmed empty before committing), all three tests in this file pass
//! again (2026-07-28, same command without `-- <test names>`):
//!
//! ```text
//! running 3 tests
//! test graceful_sigint_exits_zero_and_persists_the_graph ... ok
//! test orch_panic_on_tick_takes_the_whole_process_down ... ok
//! test orch_stop_ms_takes_the_whole_process_down ... ok
//!
//! test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
//! ```
//!
//! Original captured evidence from the pre-#2102 RED baseline, #2101's own
//! run (2026-07-28, `ZFB_ESBUILD_BIN=<path> cargo test -p zfb --test
//! dev_supervision_e2e -- --ignored --test-threads=1`, both tests FAILED
//! as expected, total run time 70.06s for the pair):
//!
//! ```text
//! test orch_panic_on_tick_takes_the_whole_process_down ...
//! thread '...' panicked at crates/zfb/tests/dev_supervision_e2e.rs:559:9:
//! process never exited within the deadline.
//! live-server probe: manual GET http://localhost:59696/ still answered
//! status 200 (first 200 bytes: "<!doctype html>...<h1>dev-loop-basic</h1>...")
//! fault fired marker ("[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK")
//! observed in captured output: true
//! ...
//! [zfb-timing] fault armed: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK
//! [zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK
//!
//! thread 'tokio-rt-worker' panicked at crates/zfb-build/src/orchestrator.rs:1526:21:
//! ZFB_DEV_TEST_ORCH_PANIC_ON_TICK fault injection
//! FAILED
//!
//! test orch_stop_ms_takes_the_whole_process_down ...
//! thread '...' panicked at crates/zfb/tests/dev_supervision_e2e.rs:673:9:
//! process never exited within the deadline.
//! live-server probe: manual GET http://localhost:59704/ still answered
//! status 200 (first 200 bytes: "<!doctype html>...<h1>dev-loop-basic</h1>...")
//! fault fired marker ("[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_STOP_MS")
//! observed in captured output: true
//! ...
//! [zfb-timing] fault armed: ZFB_DEV_TEST_ORCH_STOP_MS 6000ms
//! [zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_STOP_MS
//! FAILED
//!
//! test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
//! ```
//!
//! Both diagnostics confirm the RED baseline is the correct one, not a
//! harness bug: the fault fired (marker observed), the live-server probe
//! still got a 200 with the fixture's normal body, and the deadline was
//! reached with no exit — "fault fired but nothing supervises it", never
//! "fault never fired".
//!
//! ## Harness
//!
//! Cloned from `crates/zfb/tests/dev_serve_e2e.rs`: `spawn_dev`,
//! `DevServerGuard` (group-SIGKILL-on-drop), `parse_ready_port`, the
//! per-file `SERIAL` mutex, and `CrossBinaryE2eLock` adoption (this binary
//! spawns a real `zfb dev` and belongs in nextest's `e2e-heavy` group,
//! flock-adopting, alongside its siblings). `spawn_dev` strips every
//! inherited `ZFB_DEV_*` env var by default (so tests never cross-
//! contaminate); this file explicitly RE-INJECTS the two fault knobs via
//! `extra_env` after that strip.
//!
//! Both tests prove serving AND watcher liveness BEFORE triggering their
//! fault, via `zfb_test_utils::watcher_live_handshake` — reusing the
//! promoted #1338 helper rather than re-inlining the pattern:
//!
//! - **`ZFB_DEV_TEST_ORCH_STOP_MS`** is time-based (its deadline is
//!   established only after the boot hook completes — see #2100's doc
//!   comment on `orch_stop_ms_decision`), so the handshake's `signal_seen`
//!   is the ordinary SSE-page-event predicate from `dev_serve_e2e.rs`: a
//!   ticked warmup file is written and its SSE event observed, which both
//!   proves the watcher is live and proves that starting point has been
//!   reached, comfortably inside the STOP_MS window. The fault fires only
//!   later, once the channel goes idle past the deadline.
//! - **`ZFB_DEV_TEST_ORCH_PANIC_ON_TICK`** fires on the very NEXT
//!   dispatched tick after boot — there is no tick that can happen first
//!   without ALSO being the trigger. So proving watcher liveness and
//!   triggering the fault are the same action here: the handshake's
//!   `signal_seen` predicate watches captured stderr for the fault's own
//!   `fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK` marker instead of an
//!   SSE event (no SSE event will ever arrive — the tick panics before
//!   rendering). Observing that marker is strictly stronger proof of
//!   watcher liveness than an SSE event would be: it proves a real
//!   `notify` filesystem event reached the orchestrator's channel and was
//!   drained into a dispatched tick.
//!
//! ## RED-timeout diagnostics
//!
//! On a timeout waiting for process exit, the failure message actively
//! probes the still-live server (`GET /`) and reports whether the fault's
//! `fault fired:` marker appeared in captured output — so a failure reads
//! as either "fault never fired" (a harness/knob problem) or "fault fired
//! but nothing supervises it" (the actual #2101/#2102 defect), never an
//! ambiguous "the process did not exit".

#![cfg(unix)]

use std::fs;
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{
    locate_esbuild, watcher_live_handshake, zfb_binary, CrossBinaryE2eLock, HandshakeOpts,
};

/// Serializes the spawning tests in this file (each boots a full V8 +
/// esbuild dev session; running them concurrently would double memory
/// and CPU and produce flaky boot deadlines) — same pattern as
/// `dev_serve_e2e.rs`.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Overall wall-clock watchdog per test.
const OVERALL_DEADLINE: Duration = Duration::from_secs(180);

/// Deadline for the dev server to print its ready banner and answer
/// `GET /` with 200.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Deadline for the watcher-live handshake (SSE-event flavor, used by the
/// STOP_MS test) and its warmup marker cadence.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);

/// `ZFB_DEV_TEST_ORCH_STOP_MS` value used by the silent-stop test — large
/// enough that boot + the SSE handshake comfortably finish first (typical
/// local/CI boot is well under this), small enough to keep the test fast.
const STOP_MS: u64 = 6_000;

/// Desired post-fix deadline for the process to exit once its fault has
/// fired — generous (this is a hard failure to reap, not a race).
const FAULT_EXIT_DEADLINE: Duration = Duration::from_secs(20);

/// Deadline for the post-exit "port refuses new connections" check.
const PORT_CLOSED_DEADLINE: Duration = Duration::from_secs(10);

/// Deadline for the process to exit after a real, graceful `SIGINT` —
/// same value used by `cold_rewrite_prewarm_e2e.rs` and
/// `dev_content_aggregate_cold_boot_e2e.rs` for the identical shutdown
/// shape.
const GRACEFUL_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-loop-basic")
}

/// Recursive directory copy (same shape as `dev_serve_e2e.rs::copy_dir`).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

/// Owns the spawned `zfb dev` process. Drop group-kills the entire
/// process group, so the dev server (and anything it spawned) is reaped
/// on success, panic, and watchdog-timeout paths alike.
struct DevServerGuard {
    child: std::process::Child,
    /// PGID == child PID (the child was spawned with `process_group(0)`).
    pgid: libc::pid_t,
}

impl DevServerGuard {
    /// `Some(status)` once the dev server has exited.
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("try_wait on `zfb dev`")
    }
}

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        // Best-effort: ESRCH (already gone) is harmless.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

/// Read a captured output file back (best-effort) for failure messages.
fn read_log(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

fn dump_logs(stdout_path: &Path, stderr_path: &Path) -> String {
    format!(
        "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
        read_log(stdout_path),
        read_log(stderr_path),
    )
}

/// Extract the port from the dev ready banner (same parser as
/// `dev_serve_e2e.rs::parse_ready_port`).
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(idx) = rest.find("http://") {
        let candidate = &rest[idx + "http://".len()..];
        let token: &str = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.find(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[idx + "http://".len()..];
    }
    None
}

/// One spawned dev session: fixture root + process guard + log paths.
struct DevSession {
    root: PathBuf,
    guard: DevServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn logs(&self) -> String {
        dump_logs(&self.stdout_path, &self.stderr_path)
    }
}

/// Spawn `zfb dev --port 0` over a fresh copy of the fixture, in its own
/// process group, with stdout/stderr captured to FILES (never pipes — a
/// piped child that outgrows the OS pipe buffer blocks on write and
/// masquerades as a hang, `build_terminates.rs` pattern).
///
/// Strips every inherited `ZFB_DEV_*` fault/mode knob (matching
/// `dev_serve_e2e.rs::spawn_dev`) — INCLUDING the two #2100 fault-injection
/// knobs this file itself needs, since the harness-wide strip would
/// otherwise silently swallow an env var the caller's shell happens to
/// export. `extra_env` is applied AFTER the strip, so this file's own
/// tests must pass `ZFB_DEV_TEST_ORCH_PANIC_ON_TICK` /
/// `ZFB_DEV_TEST_ORCH_STOP_MS` there explicitly to re-inject them.
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
    // macOS /tmp is a symlink to /private/tmp; notify reports canonical
    // paths, so every path the dev process and this test compare must
    // agree on the canonical form (watch_add_confirm.rs pattern).
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");

    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        // The two #2100 fault-injection knobs this file drives — stripped
        // here so only this file's own `extra_env` (below) can arm them.
        .env_remove("ZFB_DEV_TEST_ORCH_PANIC_ON_TICK")
        .env_remove("ZFB_DEV_TEST_ORCH_STOP_MS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // New process group (PGID == child PID) so kill(-pgid, SIGKILL) reaps
    // the dev server plus any helper process it spawned.
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        root,
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Subscribe to the dev server's SSE live-reload endpoint. Must be called
/// BEFORE the edit whose tick it is meant to observe.
async fn subscribe_sse(client: &reqwest::Client, base: &str) -> reqwest::Response {
    let resp = client
        .get(format!("{base}/__zfb/reload"))
        .send()
        .await
        .expect("subscribe to /__zfb/reload");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "SSE endpoint /__zfb/reload must answer 200"
    );
    resp
}

/// Phases A-B only: discover the ephemeral port from the ready banner,
/// then HTTP-poll `GET /` until 200. Deliberately stops short of any
/// watcher-live handshake — a caller-specific handshake (see the two
/// tests below) decides how to prove liveness, since the panic-fault test
/// must fold that proof into the very edit that triggers the fault.
async fn boot_and_wait_ready(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
    let boot_start = Instant::now();
    let port = loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path)
            );
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[dev_supervision_e2e] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild unavailable); skipping test.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the \
                 ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "ready banner printed port 0 — the `--port 0` actual-bound-port fix \
                 regressed.\n{}",
                session.logs(),
            );
            break port;
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        // Whole-request timeout (codex review, #2101): without this, a
        // connection that accepts but never finishes sending headers/body
        // (e.g. a hung dev process mid-panic) can leave a `.send()`/`.text()`
        // call parked indefinitely — none of `OVERALL_DEADLINE`'s elapsed
        // checks run until the in-flight request itself resolves.
        .timeout(Duration::from_secs(15))
        .build()
        .expect("build reqwest client");

    let start = Instant::now();
    loop {
        match client.get(format!("{base}/")).send().await {
            Ok(resp) if resp.status().as_u16() == 200 => break,
            _ => {}
        }
        assert!(
            start.elapsed() < BOOT_DEADLINE,
            "GET / never answered 200 within {}s after the ready banner.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    Some((base, client))
}

/// Writes a fresh-named warmup content file into the fixture's watched
/// `content/posts/` directory — the same operation class
/// `watcher_live_handshake`'s docs call out as required (a brand-new
/// create, never a re-write of one path).
fn write_warmup_marker(root: &Path, idx: u32) {
    let warmup = root.join(format!("content/posts/__warmup-{idx}.md"));
    let _ = fs::write(
        &warmup,
        format!("---\ntitle: warmup {idx}\n---\n\nwarmup body {idx}\n"),
    );
}

/// Poll until the child has exited, or the deadline elapses.
async fn wait_for_exit(
    session: &mut DevSession,
    deadline: Duration,
) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            return Some(status);
        }
        if start.elapsed() >= deadline {
            return None;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RED-timeout diagnostics (mandatory per the sub's review amendment): on
/// a wait-for-exit timeout, probe the still-live server with a manual GET
/// and report whether the fault's `fault fired:` marker line appears in
/// captured output, so a failure reads as either "fault never fired" or
/// "fault fired but nothing supervises it" — never an ambiguous "the
/// process did not exit".
async fn diagnose_non_exit(
    client: &reqwest::Client,
    base: &str,
    fired_marker: &str,
    session: &DevSession,
) -> String {
    let probe = match client.get(format!("{base}/")).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            format!(
                "manual GET {base}/ still answered status {status} \
                 (first 200 bytes: {:?})",
                body.chars().take(200).collect::<String>(),
            )
        }
        Err(e) => format!("manual GET {base}/ failed: {e}"),
    };
    let logs = session.logs();
    let fired = logs.contains(fired_marker);
    format!(
        "process never exited within the deadline.\n\
         live-server probe: {probe}\n\
         fault fired marker ({fired_marker:?}) observed in captured output: {fired}\n\
         {logs}"
    )
}

/// Attempts a raw TCP connect to `127.0.0.1:port`, polling until it is
/// refused (or the deadline elapses). Returns `true` once refused —
/// proof the listener was actually dropped, not just that the process is
/// hanging around with a dead accept loop.
async fn wait_for_port_refused(port: u16, deadline: Duration) -> bool {
    let start = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_err() {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Desired post-fix assertions shared by both fault tests, once the
/// process has (per today's RED behavior, eventually) exited: non-zero
/// status, port refuses new connections, and stderr names the dead task.
fn assert_desired_supervision_contract(
    status: std::process::ExitStatus,
    port_refused: bool,
    logs: &str,
) {
    assert!(
        !status.success(),
        "the dev process must exit with a NON-ZERO status when its \
         watcher/orchestrator task dies (got {status:?}).\n{logs}"
    );
    assert!(
        port_refused,
        "the port must refuse new connections after the process exits \
         (proving the listener was actually dropped, not left with a \
         dead accept loop).\n{logs}"
    );
    let lower = logs.to_lowercase();
    let names_a_component = lower.contains("orchestrator") || lower.contains("watcher");
    // Deliberately does NOT include "panic"/"panicked": the raw Rust panic
    // trace already prints "thread '...' panicked at
    // crates/zfb-build/src/orchestrator.rs:...", so those two words alone
    // would make this assertion pass on today's bare, unsupervised panic
    // output — a false positive that can't distinguish "a real supervision
    // message" from "the ordinary panic backtrace happened to mention the
    // file path" (codex review, #2101). A genuine supervision message must
    // use words the raw panic trace does not.
    let names_termination = lower.contains("died")
        || lower.contains("stopped")
        || lower.contains("terminated")
        || lower.contains("exiting")
        || lower.contains("shutting down")
        || lower.contains("shutdown");
    assert!(
        names_a_component && names_termination,
        "stderr must contain a DISTINCT supervision message naming the dead \
         task (mentioning \"watcher\"/\"orchestrator\" alongside a \
         termination word like \"died\"/\"stopped\"/\"terminated\"/\
         \"exiting\"/\"shutdown\") — not just the raw panic backtrace, which \
         already mentions \"orchestrator.rs\" and \"panicked\" on its own.\n{logs}"
    );
}

// ---------------------------------------------------------------------------
// Test 1 — ZFB_DEV_TEST_ORCH_PANIC_ON_TICK: a tick-time panic must take the
// whole process down.
// ---------------------------------------------------------------------------

/// Issue #2101, depends on #2100; flipped green by #2102 (the
/// supervision-link sub), confirmed by #2105.
///
/// Boots a real `zfb dev` with `ZFB_DEV_TEST_ORCH_PANIC_ON_TICK` armed.
/// The fault fires on the very NEXT dispatched tick after boot, so there
/// is no tick that can happen first without also being the trigger:
/// proving watcher liveness and triggering the fault are the SAME action
/// here. `watcher_live_handshake`'s `signal_seen` watches captured stderr
/// for the fault's own `fault fired:` marker instead of an SSE event (no
/// SSE event ever arrives — the tick panics before rendering); observing
/// that marker is strictly stronger proof of liveness than an SSE event,
/// since it proves a real filesystem event reached the orchestrator's
/// channel and was drained into a dispatched tick.
///
/// **Verified RED before #2102** (2026-07): with the fault armed and no
/// supervision link, the marker fired (captured in stderr) but `GET /`
/// kept answering 200 and the process never exited — see the module-level
/// doc comment above for the captured evidence, and its revert-proof
/// section for #2105's confirmation that disabling #2102's arm reproduces
/// the exact same symptom today.
#[tokio::test(flavor = "multi_thread")]
async fn orch_panic_on_tick_takes_the_whole_process_down() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_supervision_e2e] esbuild not found; skipping test.");
        return;
    };
    let _serial = SERIAL.lock().await;

    let overall_start = Instant::now();
    let tmp = tempfile::tempdir().expect("create tempdir");
    let mut session = spawn_dev(&tmp, &esbuild, &[("ZFB_DEV_TEST_ORCH_PANIC_ON_TICK", "1")]);

    let Some((base, client)) = boot_and_wait_ready(&mut session).await else {
        return; // recognized environmental skip
    };

    const FIRED_MARKER: &str = "[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK";

    // Prove watcher liveness AND trigger the fault in one handshake: each
    // fresh-named marker write dispatches a tick, and the very first one
    // hits the armed panic.
    let stdout_path = session.stdout_path.clone();
    let stderr_path = session.stderr_path.clone();
    let root = session.root.clone();
    let result = watcher_live_handshake(
        HandshakeOpts::new(HANDSHAKE_DEADLINE).with_marker_interval(Duration::from_millis(400)),
        {
            let root = root.clone();
            move |idx| write_warmup_marker(&root, idx)
        },
        move || dump_logs(&stdout_path, &stderr_path).contains(FIRED_MARKER),
    )
    .await;
    assert!(
        result.live,
        "watcher never became live: no tick was ever dispatched (the \
         ZFB_DEV_TEST_ORCH_PANIC_ON_TICK fault never fired) within {}s — \
         {} markers written.\n{}",
        HANDSHAKE_DEADLINE.as_secs(),
        result.markers_written,
        session.logs(),
    );

    // Desired post-fix behavior: the dead orchestrator task takes the
    // whole process down.
    let status = wait_for_exit(&mut session, FAULT_EXIT_DEADLINE).await;
    let Some(status) = status else {
        let diag = diagnose_non_exit(&client, &base, FIRED_MARKER, &session).await;
        panic!("{diag}");
    };
    let port: u16 = base
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("recover port from base URL");
    let port_refused = wait_for_port_refused(port, PORT_CLOSED_DEADLINE).await;
    assert_desired_supervision_contract(status, port_refused, &session.logs());

    assert!(
        overall_start.elapsed() < OVERALL_DEADLINE,
        "test exceeded its overall {}s watchdog.\n{}",
        OVERALL_DEADLINE.as_secs(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Test 2 — ZFB_DEV_TEST_ORCH_STOP_MS: a silent channel-close-shaped stop
// must ALSO take the whole process down.
// ---------------------------------------------------------------------------

/// Issue #2101, depends on #2100; flipped green by #2102 (the
/// supervision-link sub), confirmed by #2105.
///
/// Boots a real `zfb dev` with `ZFB_DEV_TEST_ORCH_STOP_MS` armed (its
/// deadline is established only after the boot hook completes — see
/// #2100's doc comment on `orch_stop_ms_decision`). Proves watcher
/// liveness with the ordinary SSE-page-event handshake from
/// `dev_serve_e2e.rs` — a fresh warmup marker's SSE event is observed
/// well inside the `STOP_MS` window, which both proves the watcher is
/// live and proves the STOP_MS deadline's own starting point (boot hook
/// completion) has been reached. The fault then fires later on its own,
/// once the channel goes idle past the deadline — the same silent
/// `Ok(())`-return shape an ordinary watcher-dropped-elsewhere exit
/// takes, which is exactly the shape that must ALSO take the whole
/// process down, not just quietly end the drain loop.
///
/// **Verified RED before #2102** (2026-07): with the fault armed and no
/// supervision link, the marker fired (captured in stderr) but `GET /`
/// kept answering 200 and the process never exited — see the module-level
/// doc comment above for the captured evidence, and its revert-proof
/// section for #2105's confirmation that disabling #2102's arm reproduces
/// the exact same symptom today.
#[tokio::test(flavor = "multi_thread")]
async fn orch_stop_ms_takes_the_whole_process_down() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_supervision_e2e] esbuild not found; skipping test.");
        return;
    };
    let _serial = SERIAL.lock().await;

    let overall_start = Instant::now();
    let tmp = tempfile::tempdir().expect("create tempdir");
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[("ZFB_DEV_TEST_ORCH_STOP_MS", STOP_MS.to_string().as_str())],
    );

    let Some((base, client)) = boot_and_wait_ready(&mut session).await else {
        return; // recognized environmental skip
    };

    const FIRED_MARKER: &str = "[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_STOP_MS";

    // Phase: ordinary SSE-page-event watcher-live handshake (proves
    // liveness AND that the STOP_MS deadline's starting point — boot hook
    // completion — has been reached), comfortably inside the STOP_MS
    // window. Adapted from `dev_serve_e2e.rs`'s `boot_and_handshake` Phase
    // C: subscribe to SSE first, then write fresh-named warmup files on a
    // background timer until the first SSE event arrives.
    {
        let sse = subscribe_sse(&client, &base).await;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let root = session.root.clone();
            let stop = std::sync::Arc::clone(&stop);
            let mut idx = 0u32;
            tokio::spawn(async move {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    write_warmup_marker(&root, idx);
                    idx += 1;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            })
        };
        let first = zfb_test_utils::next_sse_event_name(sse, HANDSHAKE_DEADLINE)
            .await
            .expect("read SSE stream during watcher-live handshake");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = writer.await;
        assert!(
            first.is_some(),
            "watcher never became live: no warmup-induced SSE event within {}s \
             (either the watch stream never started delivering events, or the \
             STOP_MS deadline fired before the boot hook's starting point was \
             even reached).\n{}",
            HANDSHAKE_DEADLINE.as_secs(),
            session.logs(),
        );
    }

    // Now wait past STOP_MS for the fault to fire on its own (channel
    // idle, no further edits), then assert the desired post-fix
    // supervision contract.
    let status = wait_for_exit(
        &mut session,
        Duration::from_millis(STOP_MS) + FAULT_EXIT_DEADLINE,
    )
    .await;
    let Some(status) = status else {
        let diag = diagnose_non_exit(&client, &base, FIRED_MARKER, &session).await;
        panic!("{diag}");
    };
    let port: u16 = base
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("recover port from base URL");
    let port_refused = wait_for_port_refused(port, PORT_CLOSED_DEADLINE).await;
    assert_desired_supervision_contract(status, port_refused, &session.logs());
    assert!(
        session.logs().contains(FIRED_MARKER),
        "process exited but the ZFB_DEV_TEST_ORCH_STOP_MS fault's own \
         `fault fired:` marker never appeared — the exit must be caused BY \
         this fault, not some unrelated crash.\n{}",
        session.logs(),
    );

    assert!(
        overall_start.elapsed() < OVERALL_DEADLINE,
        "test exceeded its overall {}s watchdog.\n{}",
        OVERALL_DEADLINE.as_secs(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Test 3 — a real SIGINT (not a guard-drop SIGKILL) must exit 0 and persist
// the graph on the way out.
// ---------------------------------------------------------------------------

/// Issue #2105 (Dev Supervision epic #2099, the epic's closing confirm
/// sub) — the explicit Ctrl+C/SIGINT exit-0 regression test #2102
/// deliberately deferred. #2102's own PR did a local MANUAL SIGINT check
/// (documented in its commit message) but correctly avoided claiming
/// `dev_serve_e2e.rs` proves this: that file's `DevServerGuard` always
/// group-SIGKILLs the child on `Drop` and never sends a graceful signal at
/// all, so nothing in this crate's existing suite exercised the Ctrl+C
/// path via a real signal before this test.
///
/// Sends a REAL `SIGINT` to the dev server's process group (the exact
/// signal `tokio::signal::ctrl_c()` in `run()`'s `select!` listens for —
/// see `crates/zfb/src/commands/dev.rs`), not a guard-drop `SIGKILL`, and
/// asserts:
///
/// 1. clean exit — `ExitStatus::success()` (status code 0), within a
///    generous deadline;
/// 2. the port refuses new connections afterward (the listener was
///    genuinely dropped via `serve_with_listener`'s graceful shutdown, not
///    left with a dead accept loop);
/// 3. `.zfb/graph.bin` is freshly written on the way out — the concrete,
///    file-system-level signal #2102's own manual run used to confirm the
///    graceful shutdown tail actually ran (`run()`'s final "persist the
///    graph one more time before exit" step, `crates/zfb/src/commands/dev.rs`).
///    This is deliberately a FILE-level check rather than a captured-log
///    check: none of the three graceful-teardown steps this test's name
///    alludes to (`DevRenderSession::shutdown_explicit`, `PluginHost::shutdown`,
///    the final `save_to_disk` call) emit an info-level line on their
///    SUCCESS path today — `dev.rs` only logs on FAILURE for each of them
///    (e.g. `"graph persistence: shutdown write to {} failed (ignored)"`).
///    A freshly-written `graph.bin` is the strongest signal actually
///    available that the graceful tail — not just the process — ran to
///    completion; asserting on absent log text would either be
///    unfalsifiable (asserting nothing appeared) or dishonestly claim log
///    lines this codebase does not emit.
///
/// The fixture carries no prior `.zfb/graph.bin` (a fresh `tempfile`
/// copy), and a warmup-marker + SSE handshake (identical to Test 2's
/// liveness proof) both proves the watcher is live and forces the graph
/// to gain a genuinely new entry before shutdown — so a stale/absent file
/// can never be mistaken for a fresh one.
#[tokio::test(flavor = "multi_thread")]
async fn graceful_sigint_exits_zero_and_persists_the_graph() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_supervision_e2e] esbuild not found; skipping test.");
        return;
    };
    let _serial = SERIAL.lock().await;

    let overall_start = Instant::now();
    let tmp = tempfile::tempdir().expect("create tempdir");
    let mut session = spawn_dev(&tmp, &esbuild, &[]);

    let Some((base, client)) = boot_and_wait_ready(&mut session).await else {
        return; // recognized environmental skip
    };

    let graph_path = session.root.join(".zfb").join("graph.bin");
    assert!(
        !graph_path.exists(),
        "a fresh tempdir copy of the fixture must not already carry \
         `.zfb/graph.bin` — otherwise a stale file could be mistaken for a \
         freshly-written one below.\n{}",
        session.logs(),
    );

    // Prove watcher liveness AND force the graph to gain a new entry
    // before shutdown (same SSE-handshake shape as Test 2), so the
    // post-exit `graph.bin` freshness check below cannot be satisfied by
    // an empty/never-touched graph.
    {
        let sse = subscribe_sse(&client, &base).await;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let root = session.root.clone();
            let stop = std::sync::Arc::clone(&stop);
            let mut idx = 0u32;
            tokio::spawn(async move {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    write_warmup_marker(&root, idx);
                    idx += 1;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            })
        };
        let first = zfb_test_utils::next_sse_event_name(sse, HANDSHAKE_DEADLINE)
            .await
            .expect("read SSE stream during watcher-live handshake");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = writer.await;
        assert!(
            first.is_some(),
            "watcher never became live: no warmup-induced SSE event within \
             {}s.\n{}",
            HANDSHAKE_DEADLINE.as_secs(),
            session.logs(),
        );
    }

    // The real signal `tokio::signal::ctrl_c()` listens for in `run()`'s
    // `select!` — never a guard-drop `SIGKILL` (see `DevServerGuard::drop`
    // above, which this test deliberately does NOT rely on for the
    // assertions below; `Drop` still runs afterward as a safety net).
    unsafe {
        libc::kill(-session.guard.pgid, libc::SIGINT);
    }

    let status = wait_for_exit(&mut session, GRACEFUL_SHUTDOWN_DEADLINE).await;
    let Some(status) = status else {
        let diag = diagnose_non_exit(&client, &base, "SIGINT", &session).await;
        panic!("{diag}");
    };
    assert!(
        status.success(),
        "the dev process must exit with status 0 after a graceful SIGINT \
         (got {status:?}).\n{}",
        session.logs(),
    );

    let port: u16 = base
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("recover port from base URL");
    let port_refused = wait_for_port_refused(port, PORT_CLOSED_DEADLINE).await;
    assert!(
        port_refused,
        "the port must refuse new connections after a graceful SIGINT exit \
         (proving the listener was actually dropped).\n{}",
        session.logs(),
    );

    // Freshness is established by EXISTENCE, not by comparing mtime against
    // `before_signal` (codex review, #2105): this session's `.zfb/graph.bin`
    // was asserted ABSENT above, from a fresh `tempfile` copy of the
    // fixture that carries no prior graph at all — so the file existing
    // now is already sufficient proof the graceful persist step created it
    // during this shutdown. A `modified >= before_signal` comparison would
    // add nothing but flakiness risk on filesystems with coarse mtime
    // granularity (e.g. a rounded-down write could land just under the
    // high-resolution `before_signal` instant and fail a genuinely correct
    // shutdown).
    assert!(
        graph_path.exists(),
        "`.zfb/graph.bin` must exist after a graceful SIGINT shutdown — the \
         concrete signal #2102's own manual verification used to confirm \
         the graceful shutdown tail (session shutdown, plugin-host \
         shutdown, graph persist) actually ran.\n{}",
        session.logs(),
    );

    assert!(
        overall_start.elapsed() < OVERALL_DEADLINE,
        "test exceeded its overall {}s watchdog.\n{}",
        OVERALL_DEADLINE.as_secs(),
        session.logs(),
    );
}
