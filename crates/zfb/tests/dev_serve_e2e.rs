//! Dev-update E2E harness — real `zfb dev` edit→serve regression net
//! (issue #1018).
//!
//! ## Why this test exists
//!
//! No other test runs the real `zfb dev` binary and edits a file: the
//! crate-level dev-loop tests stub the renderer, and the existing edit
//! tests assert only status codes — a stale-serve regression (server
//! keeps answering 200 with OLD content after an edit) passes silently.
//! This harness encodes the CURRENT eager render contract end-to-end:
//! file edit on disk → real `notify` watcher → orchestrator tick → V8
//! re-render → dev server serves the NEW content over HTTP. A later
//! sub-issue extends it with lazy-render assertions once the
//! architecture flips.
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
//!   subscribed BEFORE the edit; the **assertion** is always an HTTP
//!   poll with a deadline for a unique, never-reused marker string.
//!   Trailing warmup ticks can alias the SSE signal, but they can
//!   never alias a marker assertion — each scenario's marker appears
//!   on disk exactly once, written by that scenario's edit.
//!
//! ## Spawn / teardown discipline (from `build_terminates.rs`)
//!
//! The binary is spawned in its own process group with stdout/stderr
//! redirected to temp files (never `Command::output()` — it deadlocks
//! on long-lived processes). `DevServerGuard` group-kills on Drop, so
//! success, assertion-failure, and watchdog-timeout paths all reap the
//! whole tree. The overall watchdog caps the test at a wall-clock
//! deadline and fails loudly with both captured logs.
//!
//! ## Concurrency
//!
//! This file contains exactly ONE spawning test, so no `SERIAL` mutex
//! is needed (watch_add_confirm.rs:96 pattern). If a second spawning
//! test is ever added here, serialize them with a
//! `static SERIAL: LazyLock<tokio::sync::Mutex<()>>`.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary};

/// Overall wall-clock watchdog for the whole test. A clean run takes
/// roughly boot (V8 + esbuild, ~10-30s debug) plus a few render ticks;
/// an overrun is unambiguous evidence of a hang.
const OVERALL_DEADLINE: Duration = Duration::from_secs(170);

/// Deadline for the dev server to print its ready banner and answer
/// `GET /` with 200. Boot bundles the SSR worker and renders every
/// route eagerly, so this is the slowest single phase.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-scenario deadline for a marker to appear in served HTML after
/// an edit (issue spec: 30-60s window, 100ms interval).
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
/// the URL (digits stop at the first non-digit byte either way).
fn parse_ready_port(log: &str) -> Option<u16> {
    let idx = log.find("http://")?;
    let rest = &log[idx + "http://".len()..];
    let colon = rest.find(':')?;
    let digits: String = rest[colon + 1..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Poll `url` until the body contains `needle` (and the status is 200).
/// Panics with both dev-server logs after `deadline`.
async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
    needle: &str,
    deadline: Duration,
    phase: &str,
    stdout_path: &Path,
    stderr_path: &Path,
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
        dump_logs(stdout_path, stderr_path),
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

/// The full edit→serve loop against a real `zfb dev` process.
///
/// One dev session, sequential scenarios (amortizes the V8 + esbuild
/// boot). Each scenario uses a unique, never-reused marker so watcher
/// edit-coalescing can never alias one scenario's assertion with
/// another's.
#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_edit_to_serve_loop() {
    // --- skip-guard: esbuild ---
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_serve_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    // --- fixture: copy into a tempdir, canonicalized (macOS /tmp is a
    //     symlink to /private/tmp; notify reports canonical paths, so
    //     every path the dev process and this test compare must agree
    //     on the canonical form — watch_add_confirm.rs:266-271). ---
    let tmp = tempfile::tempdir().expect("create tempdir for dev-loop-basic fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");

    // --- spawn `zfb dev --port 0` in its own process group ---
    //
    // stdout/stderr go to temp files, NEVER pipes: this test reads the
    // banner while the process keeps running, and a piped child that
    // outgrows the ~64KB OS pipe buffer would block on write and
    // masquerade as a hang (build_terminates.rs pattern).
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    // New process group (PGID == child PID) so kill(-pgid, SIGKILL)
    // reaps the dev server plus any helper process it spawned.
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    let mut guard = DevServerGuard { child, pgid };

    // --- overall watchdog: cap the whole run; the guard's Drop
    //     group-kills on every exit path, including this timeout. ---
    let scenarios = run_scenarios(&root, &mut guard, &stdout_path, &stderr_path);
    match tokio::time::timeout(OVERALL_DEADLINE, scenarios).await {
        Ok(ScenarioOutcome::Completed) => {}
        Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] dev E2E did not finish within {}s — this indicates a hang. \
                 Process group {pgid} will be killed.\n{}",
                OVERALL_DEADLINE.as_secs(),
                dump_logs(&stdout_path, &stderr_path),
            );
        }
    }
}

enum ScenarioOutcome {
    Completed,
    /// The dev binary refused to start for a recognized environmental
    /// reason (no embedded V8 / no esbuild) — skip, per the
    /// `content_snapshot_no_deferred.rs` convention.
    Skipped,
}

async fn run_scenarios(
    root: &Path,
    guard: &mut DevServerGuard,
    stdout_path: &Path,
    stderr_path: &Path,
) -> ScenarioOutcome {
    // ------------------------------------------------------------------
    // Phase A: discover the ephemeral port from the ready banner.
    //
    // The banner is parsed ONLY for the port number. Readiness itself is
    // proven by the HTTP poll in phase B.
    // ------------------------------------------------------------------
    let boot_start = Instant::now();
    let port = loop {
        if let Some(status) = guard.try_exit_status() {
            let combined = format!("{}{}", read_log(stdout_path), read_log(stderr_path));
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[dev_serve_e2e] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild unavailable); skipping test.\n{}",
                    dump_logs(stdout_path, stderr_path),
                );
                return ScenarioOutcome::Skipped;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the \
                 ready banner.\n{}",
                dump_logs(stdout_path, stderr_path),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(stdout_path)) {
            assert_ne!(
                port,
                0,
                "ready banner printed port 0 — the `--port 0` actual-bound-port fix \
                 (#1018) regressed: the banner must echo listener.local_addr(), not \
                 the requested port.\n{}",
                dump_logs(stdout_path, stderr_path),
            );
            break port;
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            dump_logs(stdout_path, stderr_path),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let base = format!("http://127.0.0.1:{port}");

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
                dump_logs(stdout_path, stderr_path),
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
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(&client, &base).await;
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let root = root.to_path_buf();
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
             (the watch stream never started delivering events — a watcher-layer \
             problem, not a render regression).\n{}",
            SSE_DEADLINE.as_secs(),
            dump_logs(stdout_path, stderr_path),
        );
    }

    // ------------------------------------------------------------------
    // Scenario 1 — baseline: both fixture routes serve their V1 body
    // markers. Polled (not single-shot) because warmup ticks may still
    // be re-rendering when we get here.
    // ------------------------------------------------------------------
    poll_until_contains(
        &client,
        &format!("{base}/posts/a/"),
        "V1-MARKER-A",
        SCENARIO_DEADLINE,
        "scenario 1: baseline /posts/a/",
        stdout_path,
        stderr_path,
    )
    .await;
    poll_until_contains(
        &client,
        &format!("{base}/posts/b/"),
        "V1-MARKER-B",
        SCENARIO_DEADLINE,
        "scenario 1: baseline /posts/b/",
        stdout_path,
        stderr_path,
    )
    .await;

    // ------------------------------------------------------------------
    // Scenario 2 — body edit: rewrite a.md's body in place (truncate +
    // write, the editor-save shape) and require the NEW marker in the
    // served HTML. This is the stale-serve regression this harness
    // exists to catch: before the per-tick renderer reload fix the
    // server kept answering 200 with the V1 body forever.
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            root.join("content/posts/a.md"),
            "---\ntitle: Alpha\ndate: 2026-01-01\n---\n\nV2-MARKER-A body for the alpha post.\n",
        )
        .expect("edit a.md body");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after body edit");
        // A trailing warmup tick could in principle deliver the first
        // event on this connection, but every tick that re-renders pages
        // emits `page` first (outcome_to_events ordering), so the name
        // assertion holds either way; the marker poll below is the
        // authoritative assertion.
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "a content body edit must broadcast an SSE `page` event.\n{}",
            dump_logs(stdout_path, stderr_path),
        );
        poll_until_contains(
            &client,
            &format!("{base}/posts/a/"),
            "V2-MARKER-A",
            SCENARIO_DEADLINE,
            "scenario 2: body edit /posts/a/",
            stdout_path,
            stderr_path,
        )
        .await;
    }

    // ------------------------------------------------------------------
    // Scenario 3 — frontmatter edit: change a.md's title (used by the
    // index listing) and require the index page to reflect it — the
    // full fan-out path under today's eager contract.
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            root.join("content/posts/a.md"),
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
            dump_logs(stdout_path, stderr_path),
        );
        poll_until_contains(
            &client,
            &format!("{base}/"),
            "V3-TITLE-MARKER-A",
            SCENARIO_DEADLINE,
            "scenario 3: frontmatter edit reaches index listing",
            stdout_path,
            stderr_path,
        )
        .await;
    }

    // ------------------------------------------------------------------
    // Scenario 4 — shared component edit: rewrite the .tsx module both
    // pages import and require BOTH routes to reflect the new marker.
    // ------------------------------------------------------------------
    {
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            root.join("components/shared-note.tsx"),
            "export function SharedNote() {\n  \
             return <p data-testid=\"shared-note\">SHARED-MARKER-V4</p>;\n}\n",
        )
        .expect("edit shared component");
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after component edit");
        assert!(
            ev.is_some(),
            "a shared-component edit must broadcast an SSE event.\n{}",
            dump_logs(stdout_path, stderr_path),
        );
        poll_until_contains(
            &client,
            &format!("{base}/"),
            "SHARED-MARKER-V4",
            SCENARIO_DEADLINE,
            "scenario 4: component edit reaches index",
            stdout_path,
            stderr_path,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/posts/b/"),
            "SHARED-MARKER-V4",
            SCENARIO_DEADLINE,
            "scenario 4: component edit reaches /posts/b/",
            stdout_path,
            stderr_path,
        )
        .await;
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
            dump_logs(stdout_path, stderr_path),
        );
        assert!(
            !body.contains("V2-MARKER-A"),
            "/posts/b/ must not contain a.md's body marker (cross-route bleed); \
             got:\n{body}",
        );
    }

    ScenarioOutcome::Completed
}
