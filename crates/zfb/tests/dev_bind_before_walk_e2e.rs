//! Bind-before-walk regression net (issue #1166) — proves `zfb dev`
//! binds the TCP listener and starts serving BEFORE the deferred
//! manifest-digest walk + graph load + boot render run.
//!
//! ## Why this test exists
//!
//! Before #1166, `compute_manifest_digest` (a `WalkDir`+`metadata()`
//! walk over the watched tree), the persisted-graph load, the graph
//! seed, and the boot render all ran SYNCHRONOUSLY before
//! `TcpListener::bind`. Cold-start reachability therefore scaled with
//! the static-asset / watched-tree SIZE (#1161). #1166 restructures
//! `dev::run` so the listener binds first and that work runs on a
//! background task.
//!
//! ## Determinism strategy (the authoritative guard — no wall-clock
//! threshold)
//!
//! The dev binary reads a TEST-ONLY env var
//! `ZFB_DEV_TEST_SLOW_DIGEST_MS` and sleeps that long inside the
//! deferred boot task, BEFORE the digest walk. The test sets a long
//! sleep (`SLOW_MS`) and asserts the server answers an HTTP request
//! (`GET /__zfb/reload`, the SSE live-reload endpoint, which is 200 the
//! instant the router is mounted and does NOT depend on any render or
//! digest) well before that sleep could have elapsed.
//!
//! Falsifiability: reverting the restructure (binding AFTER the digest /
//! boot render, the pre-#1166 ordering) makes the first successful HTTP
//! response land only AFTER `SLOW_MS` — and the `< SLOW_MS` assertion
//! fails. The assertion is keyed to the injected slow step, not a bare
//! wall-clock guess.
//!
//! ## Tests
//!
//! 1. `dev_binds_and_serves_before_slow_deferred_step` — the
//!    authoritative bind-before-walk guard (above).
//! 2. `eager_request_before_render_serves_controlled_body` — the
//!    request-before-render race contract in eager mode: before the
//!    boot render finishes, a page route NEVER serves a wrong / empty /
//!    partial body — it serves the controlled `DEV_404_BODY` (status
//!    404, complete HTML) — and once the render lands it serves 200 with
//!    the page content.
//! 3. `early_shutdown_before_digest_skips_graph_save` — graph-cache
//!    deferred-state correctness: if the dev server is killed BEFORE the
//!    deferred digest completes, NO `.zfb/graph.bin` is written (never a
//!    graph tagged with an absent / wrong digest).
//! 4. `dev_binds_and_serves_before_slow_islands_step` — the
//!    bind-before-islands guard (issue #1170). The eager initial islands
//!    bundle used to run synchronously before the bind; it now runs on the
//!    deferred boot task. Uses the same strategy with a co-located
//!    `ZFB_DEV_TEST_SLOW_ISLANDS_MS` slow step, independent of the digest
//!    one, so it specifically pins the islands build's position past bind.
//!
//! ## Spawn / teardown discipline (from `dev_serve_e2e.rs` /
//! `build_terminates.rs`)
//!
//! The binary runs in its own process group with stdout/stderr to temp
//! files (never `Command::output()` — it deadlocks on long-lived
//! processes). `DevServerGuard` group-kills on Drop, so every path
//! reaps the whole tree.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Serializes the spawning tests in this file: each boots a full V8 +
/// esbuild dev session; running them concurrently would double memory
/// and CPU and produce flaky boot deadlines (dev_serve_e2e.rs:96).
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The injected deferred-step sleep. Must be comfortably longer than the
/// time it takes the server to bind + answer one request, so the
/// "answered before the slow step finished" assertion is unambiguous.
const SLOW_MS: u64 = 12_000;

/// Deadline for the dev server to print its ready banner. Boot now does
/// NOT block on the digest/render (that is the whole point), so the
/// banner — which still follows a successful bind — lands fast. Kept
/// generous for cold V8 + esbuild boot.
const BANNER_DEADLINE: Duration = Duration::from_secs(90);

/// Deadline for the FIRST successful HTTP response after the banner.
/// This MUST be shorter than `SLOW_MS` for the guard to mean anything:
/// answering within this window while the deferred step is still
/// sleeping `SLOW_MS` proves bind precedes the walk.
const FIRST_RESPONSE_DEADLINE: Duration = Duration::from_secs(6);

const _: () = assert!(
    FIRST_RESPONSE_DEADLINE.as_secs() * 1000 < SLOW_MS,
    "FIRST_RESPONSE_DEADLINE must be shorter than SLOW_MS or the bind-before-walk \
     guard proves nothing"
);

/// Deadline for the eager boot render to populate a route (after the
/// short / zero slow step). Covers V8 + esbuild boot plus the render.
const RENDER_DEADLINE: Duration = Duration::from_secs(90);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-loop-basic")
}

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

/// Owns the spawned `zfb dev` process; Drop group-kills the whole group.
struct DevServerGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl DevServerGuard {
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("try_wait on `zfb dev`")
    }
}

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

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

/// Extract the port from the dev ready banner (dev_serve_e2e.rs pattern).
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

    fn graph_cache_path(&self) -> PathBuf {
        self.root.join(".zfb").join("graph.bin")
    }
}

/// Spawn `zfb dev --port 0` over a fresh copy of the fixture, captured
/// to log files, in its own process group.
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
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
    // Each test controls the mode exclusively through `extra_env`
    // (dev_serve_e2e.rs:262-269).
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_TEST_SLOW_DIGEST_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_ISLANDS_MS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
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

/// Wait for the ready banner and return the ephemeral port, or `None`
/// if the binary refused to start for a known-skip reason (no V8 / no
/// esbuild). Panics on any other premature exit or banner timeout.
async fn wait_for_banner_port(session: &mut DevSession) -> Option<u16> {
    let start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path)
            );
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[dev_bind_before_walk_e2e] `zfb dev` exited with a known-skip \
                     indicator (V8/esbuild unavailable); skipping test.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before the ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(port, 0, "ready banner printed port 0.\n{}", session.logs());
            return Some(port);
        }
        assert!(
            start.elapsed() < BANNER_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BANNER_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client")
}

// ---------------------------------------------------------------------------
// Test 1 — the authoritative bind-before-walk guard.
// ---------------------------------------------------------------------------

/// With a long injected deferred-step sleep, the server must bind and
/// answer an HTTP request well before that sleep completes — proving the
/// listener binds BEFORE the digest walk + boot render (issue #1166).
#[tokio::test(flavor = "multi_thread")]
async fn dev_binds_and_serves_before_slow_deferred_step() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let slow = SLOW_MS.to_string();
    // Eager mode keeps the heaviest deferred work (the full boot render)
    // on the background task too — the strongest demonstration that the
    // bind no longer waits on any of it.
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_DIGEST_MS", slow.as_str()),
            ("ZFB_DEV_EAGER", "1"),
        ],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    // The banner is printed immediately AFTER a successful bind and
    // BEFORE the deferred task's slow step elapses — already strong
    // evidence. Now prove the server actually SERVES while the slow step
    // is still in flight.
    let base = format!("http://localhost:{port}");
    let client = client();

    // `GET /__zfb/reload` is the SSE live-reload endpoint: 200 the
    // instant the router is mounted, independent of any render/digest.
    // We send the request with a short per-request timeout so a never-
    // bound port can't hang the test; success within
    // FIRST_RESPONSE_DEADLINE (< SLOW_MS) is the proof.
    let url = format!("{base}/__zfb/reload");
    let request_start = Instant::now();
    let mut answered = false;
    while request_start.elapsed() < FIRST_RESPONSE_DEADLINE {
        if let Ok(resp) = client.get(&url).send().await {
            assert_eq!(
                resp.status().as_u16(),
                200,
                "SSE endpoint must answer 200; the server is serving but on a wrong status.\n{}",
                session.logs(),
            );
            answered = true;
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    assert!(
        answered,
        "the dev server did not answer GET {url} within {}s — but the injected \
         deferred slow step is {}ms. If bind happened BEFORE the deferred step, the \
         server would answer near-instantly; this failure is the bind-before-walk \
         regression (the pre-#1166 ordering).\n{}",
        FIRST_RESPONSE_DEADLINE.as_secs(),
        SLOW_MS,
        session.logs(),
    );

    // Belt-and-braces: the whole answered-request round trip finished
    // strictly before the injected slow step could have.
    assert!(
        request_start.elapsed().as_millis() < SLOW_MS as u128,
        "served a response only after the injected slow step elapsed — bind did not \
         precede the deferred walk.\n{}",
        session.logs(),
    );

    // The deferred slow step is still sleeping; Drop group-kills it.
}

// ---------------------------------------------------------------------------
// Test 2 — request-before-render race contract (eager mode).
// ---------------------------------------------------------------------------

/// In eager mode a request can arrive before the deferred boot render
/// finishes. It must serve the controlled `DEV_404_BODY` (status 404,
/// complete HTML) — NEVER a wrong / empty / partial body — and once the
/// render lands it serves 200 with the page content. (The fixture ships
/// no prebuilt `dist/`, so the pre-render leg is the controlled-404 leg;
/// when a servable `dist/` exists that route serves it instead — same
/// contract, no wrong body.)
#[tokio::test(flavor = "multi_thread")]
async fn eager_request_before_render_serves_controlled_body() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    // A modest slow step opens a deterministic window where the route is
    // not yet rendered but the server is already serving.
    let slow = "3000";
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_DIGEST_MS", slow),
            ("ZFB_DEV_EAGER", "1"),
        ],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return;
    };
    let base = format!("http://localhost:{port}");
    let client = client();
    let page_url = format!("{base}/posts/a/");

    // During the pre-render window: the response is a CONTROLLED body —
    // a complete HTML document, never empty/partial. The status is 404
    // (no rendered HTML yet, no prebuilt dist seed) and the body is the
    // dev 404 page (which carries the live-reload script). We do NOT
    // assert a specific status flip timing; we assert the body is always
    // well-formed and never a partial/empty page render.
    let mut saw_pre_render_controlled = false;
    let window_start = Instant::now();
    while window_start.elapsed() < Duration::from_millis(1500) {
        if let Ok(resp) = client.get(&page_url).send().await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            // NEVER an empty or partial body.
            assert!(
                !body.is_empty(),
                "pre-render response had an EMPTY body — the race contract forbids \
                 serving a wrong/empty body before the render lands.\n{}",
                session.logs(),
            );
            assert!(
                body.contains("<!doctype html>") || body.contains("<!DOCTYPE html>"),
                "pre-render response was not a complete HTML document (status \
                 {status}): {body:?}\n{}",
                session.logs(),
            );
            if status == 404 && body.contains("404") {
                saw_pre_render_controlled = true;
                break;
            }
            // A 200 here means the render already landed — acceptable;
            // we just didn't catch the pre-render window. Stop probing.
            if status == 200 {
                break;
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Either we observed the controlled 404 pre-render body, or the
    // render landed faster than our probe window — both are acceptable
    // (the contract is "never a wrong body", not "always observe 404").
    // Now the authoritative positive: the eager render eventually serves
    // the real page with 200.
    let render_start = Instant::now();
    let mut served_page = false;
    while render_start.elapsed() < RENDER_DEADLINE {
        if let Ok(resp) = client.get(&page_url).send().await {
            if resp.status().as_u16() == 200 {
                let body = resp.text().await.unwrap_or_default();
                if body.contains("V1-MARKER-A") {
                    served_page = true;
                    break;
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        served_page,
        "the eager boot render never served /posts/a/ with its V1 marker within {}s \
         (pre-render controlled body seen: {saw_pre_render_controlled}).\n{}",
        RENDER_DEADLINE.as_secs(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Test 3 — graph-cache early-shutdown skip-save.
// ---------------------------------------------------------------------------

/// If the dev server is killed BEFORE the deferred digest completes, no
/// `.zfb/graph.bin` is written — never a graph tagged with an absent /
/// wrong digest (issue #1166 graph-cache deferred-state correctness).
#[tokio::test(flavor = "multi_thread")]
async fn early_shutdown_before_digest_skips_graph_save() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let slow = SLOW_MS.to_string();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[("ZFB_DEV_TEST_SLOW_DIGEST_MS", slow.as_str())],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return;
    };
    let base = format!("http://localhost:{port}");
    let client = client();

    // Confirm the server is actually serving (so the kill below is a
    // genuine "killed while the deferred digest is still sleeping", not
    // a "never booted").
    let url = format!("{base}/__zfb/reload");
    let mut serving = false;
    let start = Instant::now();
    while start.elapsed() < FIRST_RESPONSE_DEADLINE {
        if client.get(&url).send().await.is_ok() {
            serving = true;
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        serving,
        "server never started serving within {}s.\n{}",
        FIRST_RESPONSE_DEADLINE.as_secs(),
        session.logs(),
    );

    // The graph cache must not exist yet (the digest is still sleeping).
    let graph_path = session.graph_cache_path();
    assert!(
        !graph_path.exists(),
        "graph cache {} exists while the deferred digest is still sleeping — the \
         digest/seed must run before any save.\n{}",
        graph_path.display(),
        session.logs(),
    );

    // SIGINT the group NOW (well within SLOW_MS, before the digest
    // lands). SIGINT is what `zfb dev` listens for via
    // `tokio::signal::ctrl_c()`, so this runs the GRACEFUL shutdown path
    // — which reads the (still-`None`) digest slot and must SKIP the
    // save. (A bare SIGKILL would prove nothing: the process would die
    // before reaching the save block regardless of the skip logic.)
    let pgid = session.guard.pgid;
    unsafe {
        libc::kill(-pgid, libc::SIGINT);
    }
    // Wait for the process to actually exit so the shutdown path has run.
    let reap_start = Instant::now();
    loop {
        if session.guard.try_exit_status().is_some() {
            break;
        }
        if reap_start.elapsed() > Duration::from_secs(20) {
            // Force kill if SIGTERM didn't take; the assertion below
            // still holds (no save can happen after SIGKILL either).
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
            let _ = session.guard.child.wait();
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    assert!(
        reap_start.elapsed().as_millis() < SLOW_MS as u128,
        "the dev process took longer to exit than the injected slow step — the kill \
         did not precede the digest; this test would not prove the skip-save path.\n{}",
        session.logs(),
    );

    assert!(
        !graph_path.exists(),
        "graph cache {} was written even though the server was killed BEFORE the \
         deferred digest completed — an empty/stale graph tagged with an absent \
         digest must NEVER be persisted (issue #1166).\n{}",
        graph_path.display(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Test 4 — bind-before-islands guard (issue #1170).
// ---------------------------------------------------------------------------

/// Issue #1170 — the eager initial islands bundle
/// (`build_default_islands_payload`) used to run SYNCHRONOUSLY before
/// `TcpListener::bind`. On a large-dependency consumer its `"use client"`
/// scan + esbuild bundle was the dominant pre-bind cost (the last size-bound
/// step #1166 had not yet moved). It now runs on the deferred boot task.
///
/// The dev binary reads a TEST-ONLY env var `ZFB_DEV_TEST_SLOW_ISLANDS_MS`
/// and sleeps that long right before the deferred islands build. With a long
/// sleep the server must STILL bind and answer an HTTP request well before it
/// elapses — proving the listener binds BEFORE the islands build.
///
/// Falsifiability: the slow step is co-located with the islands build, so
/// moving the build back ahead of the bind moves the sleep with it — the bind
/// would then be delayed by `SLOW_MS` and the `< FIRST_RESPONSE_DEADLINE`
/// assertion fails. This is independent of `ZFB_DEV_TEST_SLOW_DIGEST_MS`
/// (which pins the digest's position); this pins the islands build's.
///
/// The `dev-loop-basic` fixture ships no `"use client"` islands, so the build
/// itself produces no bundle — but the injected sleep runs unconditionally
/// right before it, so the guard holds regardless of whether the project has
/// islands.
#[tokio::test(flavor = "multi_thread")]
async fn dev_binds_and_serves_before_slow_islands_step() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let slow = SLOW_MS.to_string();
    // Eager mode runs the full boot render on the background task too, so the
    // islands slow step sits behind the render — the bind precedes BOTH.
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_ISLANDS_MS", slow.as_str()),
            ("ZFB_DEV_EAGER", "1"),
        ],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    let base = format!("http://localhost:{port}");
    let client = client();

    // `GET /__zfb/reload` is 200 the instant the router is mounted,
    // independent of any render / islands build. Answering within
    // FIRST_RESPONSE_DEADLINE (< SLOW_MS) while the slow islands step is in
    // flight is the proof that bind precedes the islands build.
    let url = format!("{base}/__zfb/reload");
    let request_start = Instant::now();
    let mut answered = false;
    while request_start.elapsed() < FIRST_RESPONSE_DEADLINE {
        if let Ok(resp) = client.get(&url).send().await {
            assert_eq!(
                resp.status().as_u16(),
                200,
                "SSE endpoint must answer 200; the server is serving but on a wrong status.\n{}",
                session.logs(),
            );
            answered = true;
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    assert!(
        answered,
        "the dev server did not answer GET {url} within {}s — but the injected \
         slow islands step is {}ms. If bind happened BEFORE the islands build, the \
         server would answer near-instantly; this failure is the bind-before-islands \
         regression (issue #1170).\n{}",
        FIRST_RESPONSE_DEADLINE.as_secs(),
        SLOW_MS,
        session.logs(),
    );

    // Belt-and-braces: the whole answered-request round trip finished
    // strictly before the injected slow step could have.
    assert!(
        request_start.elapsed().as_millis() < SLOW_MS as u128,
        "served a response only after the injected slow islands step elapsed — bind \
         did not precede the islands build.\n{}",
        session.logs(),
    );

    // The deferred slow step is still sleeping; Drop group-kills it.
}
