//! Dev-update E2E harness — real `zfb dev` edit→serve regression net
//! (issue #1018), extended with the lazy dev-render contract (#1027).
//!
//! ## Why this test exists
//!
//! No other test runs the real `zfb dev` binary and edits a file: the
//! crate-level dev-loop tests stub the renderer, and the existing edit
//! tests assert only status codes — a stale-serve regression (server
//! keeps answering 200 with OLD content after an edit) passes silently.
//! This harness encodes the dev render contract end-to-end: file edit
//! on disk → real `notify` watcher → orchestrator tick → V8 re-render
//! (eager own-route) or stale-mark (lazy remainder) → dev server serves
//! the NEW content over HTTP, re-rendering stale routes on first
//! request.
//!
//! ## The lazy contract under test (#1025/#1026/#1027)
//!
//! Since the #1027 activation flip, `zfb dev` defaults to LAZY
//! rendering: a watcher tick eagerly re-renders only the edited
//! entry's own routes and marks the rest of the fan-out stale; a stale
//! route re-renders on its first request (GET or HEAD), blocking the
//! request until the fresh bytes are on disk. The lazy scenarios below
//! therefore assert at TWO levels:
//!
//! - **disk-negative**: after the tick's SSE event, the stale sibling's
//!   file under `.zfb-build/dev-pages/` still holds the OLD bytes —
//!   proof the tick did NOT render it eagerly;
//! - **request-then-disk-positive**: an HTTP request serves the fresh
//!   marker and the file now holds it — proof the request itself
//!   triggered the render + write-through.
//!
//! `ZFB_DEV_EAGER=1` (the escape hatch) restores the fully-eager
//! behaviour; the second spawning test runs the original eager
//! scenarios verbatim under it and additionally proves the full
//! fan-out (the shared-component edit reaches a sibling's file on disk
//! WITHOUT any HTTP request — exactly what lazy mode must not do).
//!
//! ## Determinism strategy (no fixed-sleep assertions)
//!
//! - **Readiness** is an HTTP poll of `GET /` with a deadline — the
//!   ready banner is parsed only to discover the ephemeral port
//!   (`--port 0`), never as a readiness signal.
//! - **Watcher liveness** is proven by a handshake before any real
//!   scenario edit: subscribe to the SSE endpoint first, then write
//!   fresh-named warmup content files until the first SSE event
//!   arrives (adaptation of the FSEvents dead-window mitigation in
//!   `crates/zfb-server/tests/watch_add_confirm.rs`). Fresh names are
//!   required: re-writing one path only fires Modify, and a single
//!   write could itself land in the watcher's startup dead window.
//! - **Tick completion** is signalled by SSE events on a connection
//!   subscribed BEFORE the edit; the **assertion** is always a poll
//!   with a deadline for a unique, never-reused marker string —
//!   against HTTP for request-triggered renders, against the on-disk
//!   file for eager renders (an HTTP poll would itself trigger the
//!   lazy render and could not distinguish the two). Trailing warmup
//!   ticks can alias the SSE signal, but they can never alias a marker
//!   assertion — each scenario's marker appears exactly once, written
//!   by that scenario's edit. Disk-NEGATIVE checks are alias-immune
//!   too: in lazy mode no tick ever writes the sibling's marker to
//!   disk, only a request can.
//!
//! ## Spawn / teardown discipline (from `build_terminates.rs`)
//!
//! The binary is spawned in its own process group with stdout/stderr
//! redirected to temp files (never `Command::output()` — it deadlocks
//! on long-lived processes). `DevServerGuard` group-kills on Drop, so
//! success, assertion-failure, and watchdog-timeout paths all reap the
//! whole tree. The overall watchdog caps each test at a wall-clock
//! deadline and fails loudly with both captured logs.
//!
//! ## Concurrency
//!
//! This file contains TWO spawning tests (lazy default + eager escape
//! hatch), serialized through the `SERIAL` mutex per the
//! watch_add_confirm.rs:96 pattern — two concurrent `zfb dev` boots
//! (V8 + esbuild) would starve each other on CI. The external
//! `flock`-based lock used by the workspace test runner serializes the
//! test binary against other E2E suites; `SERIAL` serializes the tests
//! within this binary.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary};

/// Serializes the spawning tests in this file (each boots a full V8 +
/// esbuild dev session; running them concurrently would double memory
/// and CPU and produce flaky boot deadlines).
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Overall wall-clock watchdog for the lazy-default test. A clean run
/// takes roughly boot (V8 + esbuild, ~10-30s debug) plus a dozen render
/// ticks; an overrun is unambiguous evidence of a hang.
const OVERALL_DEADLINE_LAZY: Duration = Duration::from_secs(280);

/// Overall watchdog for the eager escape-hatch test (original scenario
/// set only).
const OVERALL_DEADLINE_EAGER: Duration = Duration::from_secs(200);

/// Deadline for the dev server to print its ready banner and answer
/// `GET /` with 200. Boot bundles the SSR worker and renders every
/// route eagerly, so this is the slowest single phase.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-scenario deadline for a marker to appear (in served HTTP bytes
/// or in the on-disk file) after an edit (issue spec: 30-60s window,
/// 100ms interval).
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);

/// Deadline for the watcher-live handshake and for each per-scenario
/// SSE tick signal.
const SSE_DEADLINE: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-loop-basic")
}

/// Recursive directory copy (same shape as
/// `content_snapshot_no_deferred.rs`).
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
/// process group, so the dev server (and anything it spawned) is
/// reaped on success, panic, and watchdog-timeout paths alike.
struct DevServerGuard {
    child: std::process::Child,
    /// PGID == child PID (the child was spawned with `process_group(0)`).
    pgid: libc::pid_t,
}

impl DevServerGuard {
    /// `Some(status)` if the dev server already exited (it must not —
    /// except for the recognized skip conditions probed at boot).
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

/// Extract the port from the dev ready banner, e.g.
/// `→ ready on http://localhost:34567/`. Tolerates ANSI styling around
/// the URL (digits stop at the first non-digit byte either way) and
/// skips unrelated `http://` occurrences in earlier log lines by
/// scanning every match until one yields a parseable port.
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(idx) = rest.find("http://") {
        let candidate = &rest[idx + "http://".len()..];
        // Confine the colon search to this URL token: stop at the first
        // whitespace so a port-less URL can't borrow a colon from a
        // later log line.
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

    /// The dev server's HTML write root — the same directory the tick
    /// fan-out, the request-time lazy renderer, and `serve_page`'s disk
    /// leg all agree on (`dev_html_root_for` in commands/dev.rs).
    fn html_root(&self) -> PathBuf {
        self.root.join(".zfb-build").join("dev-pages")
    }
}

/// Spawn `zfb dev --port 0` over a fresh copy of the fixture, in its
/// own process group, with stdout/stderr captured to files (NEVER
/// pipes: a piped child that outgrows the ~64KB OS pipe buffer blocks
/// on write and masquerades as a hang — build_terminates.rs pattern).
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
    // macOS /tmp is a symlink to /private/tmp; notify reports canonical
    // paths, so every path the dev process and this test compare must
    // agree on the canonical form (watch_add_confirm.rs:266-271).
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
    // Strip any inherited lazy-render switches (codex review, #1027):
    // a shell/CI environment that exports ZFB_DEV_EAGER=1 would flip
    // the lazy-default session eager, and an inherited
    // ZFB_LAZY_DEV_RENDER=1 would override the escape-hatch session's
    // ZFB_DEV_EAGER (the precise override wins by design). Each test
    // must control the mode exclusively through `extra_env`.
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // New process group (PGID == child PID) so kill(-pgid, SIGKILL)
    // reaps the dev server plus any helper process it spawned.
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

/// Poll `url` until the body contains `needle` (and the status is 200).
/// Panics with both dev-server logs after `deadline`.
///
/// NOTE (lazy mode): each poll iteration IS a request, and a request
/// against a stale route triggers the request-time render. Use
/// [`poll_until_file_contains`] instead when the assertion must prove
/// an EAGER render (i.e. the bytes must appear without any HTTP
/// traffic to the route).
async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last_observation = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == 200 && body.contains(needle) {
                    return;
                }
                last_observation = format!("status {status}, body:\n{body}");
            }
            Err(e) => last_observation = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve a body containing {needle:?} within {}s.\n\
         Last observation: {last_observation}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll `url` until it answers with `status`. Panics with logs after
/// `deadline`.
async fn poll_until_status(
    client: &reqwest::Client,
    url: &str,
    status: u16,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last_observation = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let got = resp.status().as_u16();
                if got == status {
                    return;
                }
                last_observation = format!("status {got}");
            }
            Err(e) => last_observation = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not answer {status} within {}s.\n\
         Last observation: {last_observation}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll the ON-DISK file until it contains `needle` — no HTTP traffic,
/// so a success proves the tick itself wrote the bytes (eager render).
async fn poll_until_file_contains(
    path: &Path,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if fs::read_to_string(path)
            .map(|s| s.contains(needle))
            .unwrap_or(false)
        {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] file {} did not contain {needle:?} within {}s (no HTTP \
         request was made to this route — the tick was expected to write it \
         eagerly).\nCurrent content: {:?}\n{}",
        path.display(),
        deadline.as_secs(),
        fs::read_to_string(path).unwrap_or_default(),
        session.logs(),
    );
}

/// Single-shot disk assertion: the file must NOT contain `needle`.
/// Alias-immune in lazy mode: no tick ever writes a lazily-deferred
/// route's marker to disk, only a request can — so this holds no
/// matter which tick the preceding SSE event belonged to.
fn assert_file_lacks(path: &Path, needle: &str, phase: &str, session: &DevSession) {
    let content = fs::read_to_string(path).unwrap_or_default();
    assert!(
        !content.contains(needle),
        "[{phase}] file {} already contains {needle:?} before any HTTP \
         request — the tick rendered it eagerly, which the lazy contract \
         forbids for this route.\n{}",
        path.display(),
        session.logs(),
    );
}

/// Subscribe to the dev server's SSE live-reload endpoint. Must be
/// called BEFORE the edit whose tick it is meant to observe: the
/// broadcast channel only delivers events sent after subscription.
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

enum ScenarioOutcome {
    Completed,
    /// The dev binary refused to start for a recognized environmental
    /// reason (no embedded V8 / no esbuild) — skip, per the
    /// `content_snapshot_no_deferred.rs` convention.
    Skipped,
}

/// Which render contract the spawned session runs under. Drives the
/// mode-specific halves of scenario 4 (the shared-component edit):
/// eager mode proves the full fan-out reaches a sibling's disk file
/// with zero HTTP traffic; lazy mode proves the exact opposite (the
/// file stays stale until the first request).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DevMode {
    Lazy,
    Eager,
}

// ---------------------------------------------------------------------------
// Test 1 — the lazy default (issue #1027): shared scenarios + the lazy
// contract block.
// ---------------------------------------------------------------------------

/// The full edit→serve loop against a real `zfb dev` process running
/// the LAZY default. One dev session, sequential scenarios (amortizes
/// the V8 + esbuild boot). Each scenario uses a unique, never-reused
/// marker so watcher edit-coalescing can never alias one scenario's
/// assertion with another's.
#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_edit_to_serve_loop() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_serve_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create tempdir for dev-loop-basic fixture");
    let mut session = spawn_dev(&tmp, &esbuild, &[]);
    let pgid = session.guard.pgid;

    let scenarios = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };
        run_shared_scenarios(&session, &base, &client, DevMode::Lazy).await;
        run_lazy_scenarios(&session, &base, &client).await;
        ScenarioOutcome::Completed
    };
    // Bind before matching so the timed-out future (which borrows
    // `session`) is dropped before the Err arm reads the logs.
    let outcome = tokio::time::timeout(OVERALL_DEADLINE_LAZY, scenarios).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) => {}
        Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] dev E2E (lazy) did not finish within {}s — this indicates a \
                 hang. Process group {pgid} will be killed.\n{}",
                OVERALL_DEADLINE_LAZY.as_secs(),
                session.logs(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 2 — the ZFB_DEV_EAGER=1 escape hatch (issue #1027 scenario h):
// the original eager scenarios pass verbatim, plus the eager fan-out
// disk proof that MUST fail under lazy mode.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_eager_escape_hatch() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_serve_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create tempdir for dev-loop-basic fixture");
    let mut session = spawn_dev(&tmp, &esbuild, &[("ZFB_DEV_EAGER", "1")]);
    let pgid = session.guard.pgid;

    let scenarios = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };
        run_shared_scenarios(&session, &base, &client, DevMode::Eager).await;
        ScenarioOutcome::Completed
    };
    // Bind before matching so the timed-out future (which borrows
    // `session`) is dropped before the Err arm reads the logs.
    let outcome = tokio::time::timeout(OVERALL_DEADLINE_EAGER, scenarios).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) => {}
        Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] dev E2E (eager escape hatch) did not finish within {}s — \
                 this indicates a hang. Process group {pgid} will be killed.\n{}",
                OVERALL_DEADLINE_EAGER.as_secs(),
                session.logs(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Boot + handshake
// ---------------------------------------------------------------------------

/// Phases A-C: ready-banner port discovery, HTTP readiness, and the
/// watcher-live handshake. Returns `None` when the binary exited with a
/// recognized environmental skip indicator.
async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
    // ------------------------------------------------------------------
    // Phase A: discover the ephemeral port from the ready banner.
    //
    // The banner is parsed ONLY for the port number. Readiness itself is
    // proven by the HTTP poll in phase B.
    // ------------------------------------------------------------------
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
                    "[dev_serve_e2e] `zfb dev` exited with a known-skip indicator \
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
                 (#1018) regressed: the banner must echo listener.local_addr(), not \
                 the requested port.\n{}",
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
    // Connect via `localhost` — the same name `zfb dev` resolved to pick
    // its bind address. Hard-coding 127.0.0.1 would target the wrong
    // address family on IPv6-only hosts where `resolve_addr` falls back
    // to binding ::1 (reqwest/hyper tries every resolved address, so
    // this matches whichever family the listener actually bound).
    let base = format!("http://localhost:{port}");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client");

    // ------------------------------------------------------------------
    // Phase B: readiness — HTTP-poll GET / until 200 (same pattern as
    // tests/smoke/node-free/run.sh, minus the shell).
    // ------------------------------------------------------------------
    {
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
    }

    // ------------------------------------------------------------------
    // Phase C: watcher-live handshake.
    //
    // Subscribe to SSE FIRST, then write fresh-named warmup content
    // files until the first SSE event arrives — proving the watch
    // stream is past its startup dead window before any real scenario
    // edit (watch_add_confirm.rs:334-391 adaptation, out-of-process).
    // Warmup slugs (`__warmup-N`) never collide with asserted routes.
    //
    // Lazy-mode note: a warmup CREATE tick renders nothing eagerly —
    // its SSE `page` event exists only because `pages_stale` is part of
    // the event gate (#1027). The handshake therefore doubles as an
    // end-to-end check of the stale-tick → SSE path: if the gate
    // regresses, no warmup event ever arrives and this handshake fails
    // the whole test fast.
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(&client, &base).await;
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let root = session.root.to_path_buf();
            let stop = Arc::clone(&stop);
            tokio::spawn(async move {
                let mut i = 0u32;
                while !stop.load(Ordering::SeqCst) {
                    let warmup = root.join(format!("content/posts/__warmup-{i}.md"));
                    let _ = fs::write(
                        &warmup,
                        format!("---\ntitle: warmup {i}\n---\n\nwarmup body {i}\n"),
                    );
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            })
        };
        let first = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream during watcher-live handshake");
        stop.store(true, Ordering::SeqCst);
        let _ = writer.await;
        assert!(
            first.is_some(),
            "watcher never became live: no warmup-induced SSE event within {}s \
             (either the watch stream never started delivering events, or — in \
             lazy mode — a stale-marking tick no longer reaches the SSE `page` \
             gate, #1027).\n{}",
            SSE_DEADLINE.as_secs(),
            session.logs(),
        );
    }

    Some((base, client))
}

// ---------------------------------------------------------------------------
// Scenarios 1-5 — shared between the lazy default and the eager escape
// hatch (the "original eager scenarios" of #1018; they must pass
// verbatim under both contracts).
// ---------------------------------------------------------------------------

async fn run_shared_scenarios(
    session: &DevSession,
    base: &str,
    client: &reqwest::Client,
    mode: DevMode,
) {
    // ------------------------------------------------------------------
    // Scenario 1 — baseline: both fixture routes serve their V1 body
    // markers and the feed serves the fixture titles (the boot render
    // is eager in BOTH modes — lazy mode keeps the initial build eager
    // via the boot latch, #1025). Polled (not single-shot) because
    // warmup ticks may still be re-rendering when we get here.
    // ------------------------------------------------------------------
    poll_until_contains(
        client,
        &format!("{base}/posts/a/"),
        "V1-MARKER-A",
        SCENARIO_DEADLINE,
        "scenario 1: baseline /posts/a/",
        session,
    )
    .await;
    poll_until_contains(
        client,
        &format!("{base}/posts/b/"),
        "V1-MARKER-B",
        SCENARIO_DEADLINE,
        "scenario 1: baseline /posts/b/",
        session,
    )
    .await;
    poll_until_contains(
        client,
        &format!("{base}/feed.xml"),
        "Beta",
        SCENARIO_DEADLINE,
        "scenario 1: baseline /feed.xml lists post titles",
        session,
    )
    .await;

    // ------------------------------------------------------------------
    // Scenario 2 — body edit: rewrite a.md's body in place (truncate +
    // write, the editor-save shape) and require the NEW marker in the
    // served HTML. This is the stale-serve regression this harness
    // exists to catch: before the per-tick renderer reload fix the
    // server kept answering 200 with the V1 body forever.
    //
    // #1027 (d): the edited entry's OWN route renders eagerly in BOTH
    // modes (in lazy mode it is the tick's eager set). Proven by a DISK
    // poll — the marker must reach the file with zero HTTP traffic to
    // the route; an HTTP poll could not distinguish eager from lazy
    // because the request itself would trigger the render.
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(client, base).await;
        fs::write(
            session.root.join("content/posts/a.md"),
            "---\ntitle: Alpha\ndate: 2026-01-01\n---\n\nV2-MARKER-A body for the alpha post.\n",
        )
        .expect("edit a.md body");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after body edit");
        // A trailing warmup tick could in principle deliver the first
        // event on this connection, but every tick that re-renders or
        // stales pages emits `page` first (outcome_to_events ordering),
        // so the name assertion holds either way; the marker polls
        // below are the authoritative assertions.
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "a content body edit must broadcast an SSE `page` event.\n{}",
            session.logs(),
        );
        poll_until_file_contains(
            &session.html_root().join("posts/a/index.html"),
            "V2-MARKER-A",
            SCENARIO_DEADLINE,
            "scenario 2: body edit eagerly renders the edited entry's own route",
            session,
        )
        .await;
        poll_until_contains(
            client,
            &format!("{base}/posts/a/"),
            "V2-MARKER-A",
            SCENARIO_DEADLINE,
            "scenario 2: body edit /posts/a/",
            session,
        )
        .await;
    }

    // ------------------------------------------------------------------
    // Scenario 3 — frontmatter edit: change a.md's title (used by the
    // index listing) and require the index page to reflect it. Eager
    // mode re-renders the full fan-out; lazy mode marks the index stale
    // and the HTTP poll's own GET triggers the request-time render.
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(client, base).await;
        fs::write(
            session.root.join("content/posts/a.md"),
            "---\ntitle: V3-TITLE-MARKER-A\ndate: 2026-01-01\n---\n\n\
             V2-MARKER-A body for the alpha post.\n",
        )
        .expect("edit a.md frontmatter");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after frontmatter edit");
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "a frontmatter edit must broadcast an SSE `page` event.\n{}",
            session.logs(),
        );
        poll_until_contains(
            client,
            &format!("{base}/"),
            "V3-TITLE-MARKER-A",
            SCENARIO_DEADLINE,
            "scenario 3: frontmatter edit reaches index listing",
            session,
        )
        .await;
    }

    // ------------------------------------------------------------------
    // Scenario 4 — shared component edit: rewrite the .tsx module both
    // pages import and require BOTH routes to reflect the new marker.
    //
    // This is where the two contracts diverge (#1027 b/h), so the disk
    // assertion is mode-specific:
    //
    // - Eager: the tick's full fan-out must push the marker into the
    //   sibling's file with ZERO HTTP traffic to it — the escape-hatch
    //   guarantee. This disk poll FAILS under lazy mode (the file would
    //   stay stale forever without a request), so it doubles as the
    //   discriminator proving ZFB_DEV_EAGER=1 actually flipped the
    //   contract.
    // - Lazy: a `.tsx` tick has no content hint, so it renders NOTHING
    //   eagerly; the sibling's file must still hold the old bytes after
    //   the tick's SSE event, and only the first request makes it
    //   fresh (write-through asserted on disk afterwards).
    // ------------------------------------------------------------------
    {
        let sibling_file = session.html_root().join("posts/b/index.html");
        let sse = subscribe_sse(client, base).await;
        fs::write(
            session.root.join("components/shared-note.tsx"),
            "export function SharedNote() {\n  \
             return <p data-testid=\"shared-note\">SHARED-MARKER-V4</p>;\n}\n",
        )
        .expect("edit shared component");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after component edit");
        // Lazy mode: the tick writes no pages — the `page` event exists
        // purely because `pages_stale` is in the SSE gate (#1027).
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "a shared-component edit must broadcast an SSE `page` event \
             (in lazy mode via the pages_stale gate).\n{}",
            session.logs(),
        );
        match mode {
            DevMode::Eager => {
                poll_until_file_contains(
                    &sibling_file,
                    "SHARED-MARKER-V4",
                    SCENARIO_DEADLINE,
                    "scenario 4 (eager): component edit fans out to /posts/b/ on disk \
                     without any HTTP request",
                    session,
                )
                .await;
            }
            DevMode::Lazy => {
                assert_file_lacks(
                    &sibling_file,
                    "SHARED-MARKER-V4",
                    "scenario 4 (lazy): sibling file must stay stale until requested",
                    session,
                );
            }
        }
        poll_until_contains(
            client,
            &format!("{base}/"),
            "SHARED-MARKER-V4",
            SCENARIO_DEADLINE,
            "scenario 4: component edit reaches index",
            session,
        )
        .await;
        poll_until_contains(
            client,
            &format!("{base}/posts/b/"),
            "SHARED-MARKER-V4",
            SCENARIO_DEADLINE,
            "scenario 4: component edit reaches /posts/b/",
            session,
        )
        .await;
        if mode == DevMode::Lazy {
            // Write-through: the request that served the marker also
            // wrote it back to the html root (#1024 unified write path).
            let on_disk = fs::read_to_string(&sibling_file).unwrap_or_default();
            assert!(
                on_disk.contains("SHARED-MARKER-V4"),
                "scenario 4 (lazy): the request-time render must write through \
                 to {} — got:\n{on_disk}\n{}",
                sibling_file.display(),
                session.logs(),
            );
        }
    }

    // ------------------------------------------------------------------
    // Scenario 5 — negative guard: the never-edited post still serves
    // 200 with its original body, and no marker bled across routes.
    // Single-shot: scenario 4 already proved /posts/b/'s latest render
    // landed, so there is no tick left in flight for this route.
    // ------------------------------------------------------------------
    {
        let resp = client
            .get(format!("{base}/posts/b/"))
            .send()
            .await
            .expect("GET /posts/b/ for negative guard");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "untouched route /posts/b/ must still answer 200"
        );
        let body = resp.text().await.unwrap_or_default();
        assert!(
            body.contains("V1-MARKER-B"),
            "untouched route /posts/b/ must still serve its original body marker; \
             got:\n{body}\n{}",
            session.logs(),
        );
        assert!(
            !body.contains("V2-MARKER-A"),
            "/posts/b/ must not contain a.md's body marker (cross-route bleed); \
             got:\n{body}",
        );
    }
}

// ---------------------------------------------------------------------------
// Scenarios 6-10 — the lazy contract block (#1027 a/c/f/g/e). Only run
// under the lazy default.
// ---------------------------------------------------------------------------

async fn run_lazy_scenarios(session: &DevSession, base: &str, client: &reqwest::Client) {
    let html_root = session.html_root();

    // ------------------------------------------------------------------
    // Scenario 6 (#1027 a) — frontmatter edit, stale sibling: editing
    // a.md's title eagerly re-renders only a's own routes; the index
    // (which lists the title) is marked stale. After the tick's SSE
    // event the index file on disk must still hold the OLD title
    // (disk-negative, alias-immune), and the first GET serves the new
    // one — the poll's first iteration after the stale mark commits IS
    // that first request-triggered render.
    // ------------------------------------------------------------------
    {
        let index_file = html_root.join("index.html");
        let sse = subscribe_sse(client, base).await;
        fs::write(
            session.root.join("content/posts/a.md"),
            "---\ntitle: V5-LAZY-TITLE-A\ndate: 2026-01-01\n---\n\n\
             V2-MARKER-A body for the alpha post.\n",
        )
        .expect("edit a.md frontmatter (lazy scenario)");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after lazy frontmatter edit");
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "a frontmatter edit must broadcast an SSE `page` event even when \
             the affected sibling routes are only marked stale (#1027).\n{}",
            session.logs(),
        );
        assert_file_lacks(
            &index_file,
            "V5-LAZY-TITLE-A",
            "scenario 6: index file must stay stale until requested",
            session,
        );
        poll_until_contains(
            client,
            &format!("{base}/"),
            "V5-LAZY-TITLE-A",
            SCENARIO_DEADLINE,
            "scenario 6: stale index serves the new title on request",
            session,
        )
        .await;
        let on_disk = fs::read_to_string(&index_file).unwrap_or_default();
        assert!(
            on_disk.contains("V5-LAZY-TITLE-A"),
            "scenario 6: the request-time render must write through to {} — \
             got:\n{on_disk}\n{}",
            index_file.display(),
            session.logs(),
        );

        // --------------------------------------------------------------
        // Scenario 7 (#1027 c) — feed-style non-HTML route, same tick:
        // /feed.xml lists titles, so the frontmatter edit above also
        // staled it. No browser tab ever subscribes for a feed — the
        // request-time render is its ONLY refresh path.
        // --------------------------------------------------------------
        let feed_file = html_root.join("feed.xml");
        assert_file_lacks(
            &feed_file,
            "V5-LAZY-TITLE-A",
            "scenario 7: feed file must stay stale until requested",
            session,
        );
        poll_until_contains(
            client,
            &format!("{base}/feed.xml"),
            "V5-LAZY-TITLE-A",
            SCENARIO_DEADLINE,
            "scenario 7: stale feed serves the new title on request",
            session,
        )
        .await;
        let feed_resp = client
            .get(format!("{base}/feed.xml"))
            .send()
            .await
            .expect("GET /feed.xml for content-type check");
        let ct = feed_resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ct.starts_with("application/xml"),
            "request-rendered feed must keep its non-HTML Content-Type, got {ct:?}"
        );
        let feed_body = feed_resp.text().await.unwrap_or_default();
        assert!(
            feed_body.contains("V5-LAZY-TITLE-A"),
            "feed body must carry the new title after the request-time render; \
             got:\n{feed_body}"
        );
        assert!(
            !feed_body.contains("<script"),
            "non-HTML output must not get the livereload script injected; \
             got:\n{feed_body}"
        );
        let feed_on_disk = fs::read_to_string(&feed_file).unwrap_or_default();
        assert!(
            feed_on_disk.contains("V5-LAZY-TITLE-A"),
            "scenario 7: the request-time render must write through to {} — \
             got:\n{feed_on_disk}",
            feed_file.display(),
        );
    }

    // ------------------------------------------------------------------
    // Scenario 8 (#1027 f) — deleted content file: its route 404s and
    // is NOT lazily resurrected. Create a doomed post (watch-ADD
    // discovery materialises the route; the first GET lazily renders
    // it), then delete the file: the route table swap drops the route,
    // the vanish diff prunes its HTML, and later requests must keep
    // 404ing — a reverse-lookup miss, never a resurrection render.
    // ------------------------------------------------------------------
    {
        let doomed_src = session.root.join("content/posts/doomed.md");
        let doomed_out = html_root.join("posts/doomed/index.html");
        fs::write(
            &doomed_src,
            "---\ntitle: DOOMED-TITLE-1\ndate: 2026-01-03\n---\n\nDOOMED-MARKER-1 body.\n",
        )
        .expect("create doomed.md");
        poll_until_contains(
            client,
            &format!("{base}/posts/doomed/"),
            "DOOMED-MARKER-1",
            SCENARIO_DEADLINE,
            "scenario 8: created post route materialises and serves on request",
            session,
        )
        .await;

        let sse = subscribe_sse(client, base).await;
        fs::remove_file(&doomed_src).expect("delete doomed.md");
        // The deletion tick stales the listing routes (index/feed lose
        // the doomed entry), so the SSE `page` gate fires via
        // pages_stale even though nothing was written eagerly.
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after content deletion");
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "deleting a content file must broadcast an SSE `page` event so \
             open listings reload (#1027).\n{}",
            session.logs(),
        );
        poll_until_status(
            client,
            &format!("{base}/posts/doomed/"),
            404,
            SCENARIO_DEADLINE,
            "scenario 8: deleted post route 404s",
            session,
        )
        .await;
        // NOT lazily resurrected: repeated requests stay 404 and no
        // request re-materialises the pruned file on disk.
        for attempt in 0..3 {
            let resp = client
                .get(format!("{base}/posts/doomed/"))
                .send()
                .await
                .expect("GET deleted route");
            assert_eq!(
                resp.status().as_u16(),
                404,
                "deleted route must stay 404 (attempt {attempt}) — a 200 here \
                 means the lazy path resurrected a vanished route.\n{}",
                session.logs(),
            );
        }
        assert!(
            !doomed_out.exists(),
            "scenario 8: pruned output {} must not be re-created by requests \
             against the deleted route.\n{}",
            doomed_out.display(),
            session.logs(),
        );
        // The stale listing drops the doomed entry on its next request.
        poll_until_contains(
            client,
            &format!("{base}/"),
            "V5-LAZY-TITLE-A",
            SCENARIO_DEADLINE,
            "scenario 8: index still serves after the deletion tick",
            session,
        )
        .await;
        let index_body = client
            .get(format!("{base}/"))
            .send()
            .await
            .expect("GET / after deletion")
            .text()
            .await
            .unwrap_or_default();
        assert!(
            !index_body.contains("DOOMED-TITLE-1"),
            "index listing must drop the deleted post after its lazy \
             re-render; got:\n{index_body}"
        );
    }

    // ------------------------------------------------------------------
    // Scenario 9 (#1027 g) — HEAD refreshes a stale route: a HEAD
    // request runs the same render-on-request hook as GET (the
    // GET/HEAD-gated leg in serve_page), so it must refresh the stale
    // file on disk while its own response carries no body.
    // ------------------------------------------------------------------
    {
        let a_file = html_root.join("posts/a/index.html");
        let sse = subscribe_sse(client, base).await;
        fs::write(
            session.root.join("components/shared-note.tsx"),
            "export function SharedNote() {\n  \
             return <p data-testid=\"shared-note\">SHARED-MARKER-V8</p>;\n}\n",
        )
        .expect("edit shared component (HEAD scenario)");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after component edit (HEAD scenario)");
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "the component edit must broadcast an SSE `page` event.\n{}",
            session.logs(),
        );
        assert_file_lacks(
            &a_file,
            "SHARED-MARKER-V8",
            "scenario 9: /posts/a/ must stay stale until the HEAD request",
            session,
        );
        // HEAD-poll: each iteration issues a HEAD (the lazy trigger) and
        // checks the DISK for the refresh — never a GET, so the GET
        // below really is the first GET after the HEAD-triggered render.
        let start = Instant::now();
        loop {
            let resp = client
                .head(format!("{base}/posts/a/"))
                .send()
                .await
                .expect("HEAD /posts/a/");
            assert_eq!(
                resp.status().as_u16(),
                200,
                "HEAD on an existing route must answer 200.\n{}",
                session.logs(),
            );
            let head_body = resp.text().await.unwrap_or_default();
            assert!(
                head_body.is_empty(),
                "a HEAD response must carry no body; got {} bytes:\n{head_body}",
                head_body.len(),
            );
            if fs::read_to_string(&a_file)
                .map(|s| s.contains("SHARED-MARKER-V8"))
                .unwrap_or(false)
            {
                break;
            }
            assert!(
                start.elapsed() < SCENARIO_DEADLINE,
                "scenario 9: HEAD requests did not refresh the stale file {} \
                 within {}s.\nCurrent content: {:?}\n{}",
                a_file.display(),
                SCENARIO_DEADLINE.as_secs(),
                fs::read_to_string(&a_file).unwrap_or_default(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        // The subsequent (first) GET serves the refreshed bytes.
        let resp = client
            .get(format!("{base}/posts/a/"))
            .send()
            .await
            .expect("GET /posts/a/ after HEAD refresh");
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap_or_default();
        assert!(
            body.contains("SHARED-MARKER-V8"),
            "scenario 9: the GET after a HEAD-triggered refresh must serve the \
             fresh marker; got:\n{body}\n{}",
            session.logs(),
        );
    }

    // ------------------------------------------------------------------
    // Scenario 10 (#1027 e) — closing negative guard, single-shot: the
    // never-edited post's FIRST request after the scenario-9 tick must
    // serve 200 with its original body PLUS the latest shared marker
    // (fresh-on-first-request for the stale sibling), and no foreign
    // markers. Single-shot is safe: scenario 9's HEAD loop proved the
    // tick's stale marks are committed (the refresh happened), so
    // /posts/b/'s staleness is already observable.
    // ------------------------------------------------------------------
    {
        let resp = client
            .get(format!("{base}/posts/b/"))
            .send()
            .await
            .expect("GET /posts/b/ for closing negative guard");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "untouched route /posts/b/ must answer 200"
        );
        let body = resp.text().await.unwrap_or_default();
        assert!(
            body.contains("V1-MARKER-B"),
            "untouched route /posts/b/ must keep its original body marker; \
             got:\n{body}\n{}",
            session.logs(),
        );
        assert!(
            body.contains("SHARED-MARKER-V8"),
            "the FIRST request to the stale sibling must already serve the \
             latest shared-component marker (lazy fresh-on-first-request); \
             got:\n{body}\n{}",
            session.logs(),
        );
        assert!(
            !body.contains("V2-MARKER-A"),
            "/posts/b/ must not contain a.md's body marker (cross-route bleed); \
             got:\n{body}",
        );
        assert!(
            !body.contains("DOOMED-MARKER-1"),
            "/posts/b/ must not contain the deleted post's marker; got:\n{body}",
        );
    }
}
