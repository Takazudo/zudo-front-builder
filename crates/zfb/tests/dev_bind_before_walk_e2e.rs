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
//! The dev binary reads TEST-ONLY env vars (`ZFB_DEV_TEST_SLOW_DIGEST_MS`,
//! `ZFB_DEV_TEST_SLOW_ISLANDS_MS`) and sleeps that long inside the deferred
//! boot task, immediately BEFORE the step being pinned (the digest walk / the
//! islands build). Each guard injects a sleep LONGER than `BANNER_DEADLINE`
//! (`SLOW_DIGEST_MS` / `SLOW_ISLANDS_MS`).
//!
//! Falsifiability is anchored to the banner-vs-bind ordering, NOT to a
//! banner-relative response window. The ready banner is printed immediately
//! after a successful `TcpListener::bind`, so in the correct (post-bind)
//! ordering it lands at boot time, far under `BANNER_DEADLINE`. Reverting the
//! restructure (running the slow step BEFORE the bind — the pre-#1166 /
//! pre-#1170 ordering) delays the banner by the injected sleep
//! (> `BANNER_DEADLINE`), so `wait_for_banner_port` times out and the test
//! fails.
//!
//! A banner-RELATIVE window can NOT catch a pre-bind step, because the banner
//! floats with the bind: an earlier shared `SLOW_MS = 12_000` (< the 90s
//! `BANNER_DEADLINE`) let `wait_for_banner_port` patiently wait out a 12s
//! pre-bind digest, after which the `< SLOW_MS` window still passed — so the
//! two #1166 guards proved nothing about bind ordering. Issue #1174 replaced
//! `SLOW_MS` with `SLOW_DIGEST_MS` (> `BANNER_DEADLINE`) to close that gap, the
//! same fix #1170 already applied to the islands guard.
//!
//! Each guard additionally asserts the serve path answers `GET /__zfb/reload`
//! (200 the instant the router is mounted, independent of any render / digest /
//! islands build) while the slow step is still in flight — belt-and-braces
//! proof the serve path does not block on the deferred work.
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
//! 5. `dev_binds_and_serves_before_slow_bundle_step` — the
//!    bind-before-bundle guard (issue #1182). The eager esbuild dev bundle
//!    (`assemble_and_bundle_dev`) used to run synchronously before the bind
//!    even in boot-lazy mode with a servable `dist/` (the residual of #1161);
//!    it now runs on the deferred boot task in that mode. Runs boot-lazy with
//!    a seeded servable `dist/` and pins the bundle's position past bind via
//!    TWO co-located `ZFB_DEV_TEST_SLOW_BUNDLE_MS` seams (deferred + eager) so
//!    an un-defer revert is caught.
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

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Serializes the spawning tests in this file: each boots a full V8 +
/// esbuild dev session; running them concurrently would double memory
/// and CPU and produce flaky boot deadlines (dev_serve_e2e.rs:96). Each
/// spawning test also acquires `CrossBinaryE2eLock` BEFORE this mutex to
/// serialize against sibling e2e binaries too (issue #1339) — see
/// `zfb-test-utils/src/cross_binary_lock.rs` for the lock-ordering
/// rationale.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Deadline for the dev server to print its ready banner. Boot now does
/// NOT block on the digest/render (that is the whole point), so the
/// banner — which still follows a successful bind — lands fast. Kept
/// generous for cold V8 + esbuild boot.
const BANNER_DEADLINE: Duration = Duration::from_secs(90);

/// The injected digest slow-step for the two bind-before-digest guards
/// (Test 1 + Test 3, issue #1166). Deliberately set LONGER than
/// `BANNER_DEADLINE`, exactly like `SLOW_ISLANDS_MS` below. The ready banner is
/// printed immediately after a successful bind, so in the correct (post-bind)
/// ordering it lands at boot time, far under `BANNER_DEADLINE`. If the digest
/// walk — and this co-located sleep — were moved back BEFORE the bind (the
/// pre-#1166 ordering these tests guard), the banner would be delayed by
/// `SLOW_DIGEST_MS` (> `BANNER_DEADLINE`) and `wait_for_banner_port` would time
/// out and fail the test.
///
/// This replaced an earlier shared `SLOW_MS = 12_000` (< `BANNER_DEADLINE`):
/// because a 12s pre-bind sleep stayed under the 90s banner deadline,
/// `wait_for_banner_port` patiently waited it out and the banner-relative
/// `< SLOW_MS` window still passed — so the guards were NOT falsifiable for a
/// pre-bind digest (issue #1174). Anchoring to the banner-vs-bind ordering
/// fixes that. A green run never waits this out — it answers fast and Drop
/// SIGKILLs the group — so the large value is free.
const SLOW_DIGEST_MS: u64 = 120_000;

const _: () = assert!(
    SLOW_DIGEST_MS > BANNER_DEADLINE.as_secs() * 1000,
    "SLOW_DIGEST_MS must exceed BANNER_DEADLINE — otherwise a pre-bind digest walk \
     would still print the banner within the deadline and the guards prove nothing"
);

/// The injected islands slow-step for the bind-before-islands guard (issue
/// #1170). Deliberately set LONGER than `BANNER_DEADLINE` — same rationale as
/// `SLOW_DIGEST_MS` above, but pinning the islands build's position rather than
/// the digest's. If the islands build — and this co-located sleep — were moved
/// back BEFORE the bind, the banner would be delayed by `SLOW_ISLANDS_MS`
/// (> `BANNER_DEADLINE`) and `wait_for_banner_port` would time out and fail the
/// test. A green run never waits this out — it answers fast and Drop SIGKILLs
/// the group — so the large value is free.
const SLOW_ISLANDS_MS: u64 = 120_000;

const _: () = assert!(
    SLOW_ISLANDS_MS > BANNER_DEADLINE.as_secs() * 1000,
    "SLOW_ISLANDS_MS must exceed BANNER_DEADLINE — otherwise a pre-bind islands build \
     would still print the banner within the deadline and the guard proves nothing"
);

/// The injected dev-bundle slow-step for the bind-before-bundle guard (issue
/// #1182). Same rationale as `SLOW_DIGEST_MS` / `SLOW_ISLANDS_MS`, pinning the
/// EAGER esbuild dev bundle (`assemble_and_bundle_dev`) — the dominant,
/// size-scaling pre-bind cost #1166/#1170 left behind. Deliberately set LONGER
/// than `BANNER_DEADLINE`: the binary reads `ZFB_DEV_TEST_SLOW_BUNDLE_MS` from
/// TWO co-located seams — one before the DEFERRED boot bundle (which runs after
/// bind, so in the correct ordering the banner lands fast), one before the
/// EAGER pre-bind bundle (which fires only if the deferral is reverted, delaying
/// the banner past `BANNER_DEADLINE` so `wait_for_banner_port` times out and
/// fails). A green run never waits this out — it answers fast and Drop SIGKILLs
/// the group — so the large value is free.
const SLOW_BUNDLE_MS: u64 = 120_000;

const _: () = assert!(
    SLOW_BUNDLE_MS > BANNER_DEADLINE.as_secs() * 1000,
    "SLOW_BUNDLE_MS must exceed BANNER_DEADLINE — otherwise an un-deferred pre-bind dev \
     bundle would still print the banner within the deadline and the guard proves nothing"
);

/// Deadline for the FIRST successful HTTP response after the banner. The banner
/// timeout is the PRIMARY bind-ordering signal (see `SLOW_DIGEST_MS`); this is
/// the belt-and-braces serve-path check — answering within this window while
/// the deferred step is still sleeping proves the serve path does not block on
/// the deferred digest / islands build. MUST stay shorter than the injected
/// slow steps.
const FIRST_RESPONSE_DEADLINE: Duration = Duration::from_secs(6);

const _: () = assert!(
    FIRST_RESPONSE_DEADLINE.as_secs() * 1000 < SLOW_DIGEST_MS,
    "FIRST_RESPONSE_DEADLINE must be shorter than SLOW_DIGEST_MS or the serve-path \
     belt-and-braces check proves nothing"
);

const _: () = assert!(
    FIRST_RESPONSE_DEADLINE.as_secs() * 1000 < SLOW_ISLANDS_MS,
    "FIRST_RESPONSE_DEADLINE must be shorter than SLOW_ISLANDS_MS or Test 4's serve-path \
     belt-and-braces check proves nothing"
);

const _: () = assert!(
    FIRST_RESPONSE_DEADLINE.as_secs() * 1000 < SLOW_BUNDLE_MS,
    "FIRST_RESPONSE_DEADLINE must be shorter than SLOW_BUNDLE_MS or Test 5's serve-path \
     belt-and-braces check proves nothing"
);

/// Test 3 reap budget: after SIGINT (sent while the digest is still sleeping),
/// the GRACEFUL shutdown must read the still-`None` digest slot, skip the save,
/// and exit within this window. A shutdown that instead blocked on the
/// in-flight digest would take ~`SLOW_DIGEST_MS`. Decoupled from
/// `SLOW_DIGEST_MS` on purpose: now that the digest sleep exceeds
/// `BANNER_DEADLINE` (issue #1174), keying the reap assertion to it would let
/// the force-kill fallback below mask a hung shutdown (it would SIGKILL at
/// `SHUTDOWN_FORCE_KILL_AFTER`, landing the reap under the digest sleep and so
/// passing a `< SLOW_DIGEST_MS` check even when the shutdown never skipped).
const GRACEFUL_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(12);

/// Test 3 force-kill fallback: if SIGINT does not reap the group within this
/// window, SIGKILL it so a hung process can't hang the test. MUST stay LONGER
/// than `GRACEFUL_SHUTDOWN_DEADLINE` — otherwise a shutdown that blocked on the
/// digest would be force-killed early and its reap would fall under the
/// deadline, masking the regression instead of failing it.
const SHUTDOWN_FORCE_KILL_AFTER: Duration = Duration::from_secs(20);

const _: () = assert!(
    GRACEFUL_SHUTDOWN_DEADLINE.as_secs() < SHUTDOWN_FORCE_KILL_AFTER.as_secs(),
    "GRACEFUL_SHUTDOWN_DEADLINE must be shorter than SHUTDOWN_FORCE_KILL_AFTER or the \
     force-kill fallback would mask a shutdown that blocked on the digest"
);

const _: () = assert!(
    SHUTDOWN_FORCE_KILL_AFTER.as_secs() * 1000 < SLOW_DIGEST_MS,
    "SHUTDOWN_FORCE_KILL_AFTER must stay under SLOW_DIGEST_MS — the force-kill must cap a \
     hung shutdown's reap WELL below the digest sleep so it overshoots \
     GRACEFUL_SHUTDOWN_DEADLINE and fails; if the force-kill landed at/after the digest \
     sleep the masking this whole decoupling guards against could re-emerge"
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
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_DIGEST_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_ISLANDS_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_BUNDLE_MS");
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

/// The authoritative bind-before-walk guard. The injected digest slow-step
/// (`SLOW_DIGEST_MS` > `BANNER_DEADLINE`) sleeps immediately before the digest
/// walk, so in the correct ordering the banner lands at boot time; a pre-bind
/// digest walk would delay the banner past `BANNER_DEADLINE` →
/// `wait_for_banner_port` times out → the test fails (issue #1166;
/// falsifiability fixed in #1174). Also proves the serve path answers while the
/// slow step is still in flight.
#[tokio::test(flavor = "multi_thread")]
async fn dev_binds_and_serves_before_slow_deferred_step() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let slow = SLOW_DIGEST_MS.to_string();
    // Eager mode keeps the heaviest deferred work (the full boot render)
    // on the background task too — the strongest demonstration that the
    // bind no longer waits on any of it.
    let spawn_t = Instant::now();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_DIGEST_MS", slow.as_str()),
            ("ZFB_DEV_EAGER", "1"),
        ],
    );

    // Guarantee 1 (bind precedes the digest walk): the banner follows the bind,
    // so it lands at boot time in the correct ordering. A pre-bind digest walk
    // would delay the banner by SLOW_DIGEST_MS (> BANNER_DEADLINE) and the wait
    // below would time out — that timeout IS the regression signal (the
    // pre-#1166 ordering).
    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    // Explicit, clearly-messaged form of the same guarantee (in case the
    // deadlines are ever retuned): the banner — and the bind it follows —
    // landed well before the injected digest slow-step could have.
    let banner_elapsed = spawn_t.elapsed();
    assert!(
        (banner_elapsed.as_millis() as u64) < SLOW_DIGEST_MS,
        "ready banner appeared {}ms after spawn, but the injected digest slow-step is \
         {}ms — the digest walk (and its sleep) ran BEFORE the bind/banner \
         (bind-before-walk regression, the pre-#1166 ordering).\n{}",
        banner_elapsed.as_millis(),
        SLOW_DIGEST_MS,
        session.logs(),
    );

    // Guarantee 2 (serve path is not blocked by the deferred walk): prove the
    // server actually SERVES while the slow step is still in flight.
    let base = format!("http://localhost:{port}");
    let client = client();

    // `GET /__zfb/reload` is the SSE live-reload endpoint: 200 the
    // instant the router is mounted, independent of any render/digest.
    // We send the request with a short per-request timeout so a never-
    // bound port can't hang the test; success within
    // FIRST_RESPONSE_DEADLINE (< SLOW_DIGEST_MS) is the proof.
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
        "the dev server did not answer GET {url} within {}s of the banner while the \
         digest slow-step ({}ms) was in flight — the serve path is blocking on the \
         deferred digest walk (issue #1166).\n{}",
        FIRST_RESPONSE_DEADLINE.as_secs(),
        SLOW_DIGEST_MS,
        session.logs(),
    );

    // Belt-and-braces: the whole answered-request round trip finished
    // strictly before the injected slow step could have.
    assert!(
        request_start.elapsed().as_millis() < SLOW_DIGEST_MS as u128,
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
    let _e2e_lock = CrossBinaryE2eLock::acquire();
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
///
/// Uses `SLOW_DIGEST_MS` (> `BANNER_DEADLINE`) so the bind-before-digest
/// ordering is itself falsifiable via the banner timeout (issue #1174). The
/// reap budget is a SEPARATE constant (`GRACEFUL_SHUTDOWN_DEADLINE`, with the
/// force-kill fallback at `SHUTDOWN_FORCE_KILL_AFTER`) so that bumping the
/// digest sleep past the banner deadline can't let the force-kill mask a
/// shutdown that blocked on the digest. (Verified against `dev.rs`: SIGINT
/// `boot_handle.abort()`s the boot task rather than joining it, so the longer
/// digest sleep does not slow the graceful reap; the still-`None` digest slot
/// is what drives the skip-save.)
#[tokio::test(flavor = "multi_thread")]
async fn early_shutdown_before_digest_skips_graph_save() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let slow = SLOW_DIGEST_MS.to_string();
    let spawn_t = Instant::now();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[("ZFB_DEV_TEST_SLOW_DIGEST_MS", slow.as_str())],
    );

    // Bind precedes the digest walk: a pre-bind digest (and its co-located
    // sleep) would delay the banner past BANNER_DEADLINE and time out
    // wait_for_banner_port — that timeout IS the bind-before-walk regression
    // signal (the pre-#1166 ordering).
    let Some(port) = wait_for_banner_port(&mut session).await else {
        return;
    };
    let banner_elapsed = spawn_t.elapsed();
    assert!(
        (banner_elapsed.as_millis() as u64) < SLOW_DIGEST_MS,
        "ready banner appeared {}ms after spawn, but the injected digest slow-step is \
         {}ms — the digest walk ran BEFORE the bind/banner (bind-before-walk \
         regression, the pre-#1166 ordering).\n{}",
        banner_elapsed.as_millis(),
        SLOW_DIGEST_MS,
        session.logs(),
    );

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

    // SIGINT the group NOW (well within SLOW_DIGEST_MS, before the digest
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
        if reap_start.elapsed() > SHUTDOWN_FORCE_KILL_AFTER {
            // Force kill if SIGINT didn't take; the assertion below still
            // catches it (a force-kill lands the reap at ~SHUTDOWN_FORCE_KILL_AFTER,
            // which exceeds GRACEFUL_SHUTDOWN_DEADLINE → fails).
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
            let _ = session.guard.child.wait();
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // The graceful shutdown must have skipped fast (read the None digest slot
    // and exited), NOT blocked on the in-flight digest. A blocked shutdown
    // would not exit until ~SLOW_DIGEST_MS; the force-kill fallback caps that
    // at SHUTDOWN_FORCE_KILL_AFTER (> GRACEFUL_SHUTDOWN_DEADLINE), so either
    // way a non-skipping shutdown overshoots this deadline. Keyed to a
    // dedicated reap budget — NOT SLOW_DIGEST_MS — precisely so the long digest
    // sleep can't push the bar above the force-kill window and hide a hang.
    assert!(
        reap_start.elapsed() < GRACEFUL_SHUTDOWN_DEADLINE,
        "graceful shutdown after SIGINT took {}ms (>= {}s) — the shutdown path did \
         not skip-and-exit while the digest was still sleeping; this test would not \
         prove the skip-save path.\n{}",
        reap_start.elapsed().as_millis(),
        GRACEFUL_SHUTDOWN_DEADLINE.as_secs(),
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
/// and sleeps that long right before the deferred islands build. This test
/// sets `SLOW_ISLANDS_MS` (> `BANNER_DEADLINE`) and checks TWO things:
///
/// 1. **Bind precedes the islands build.** The ready banner is printed
///    immediately after a successful bind, so in the correct (post-bind)
///    ordering it lands at boot time — well under `BANNER_DEADLINE`. If the
///    islands build (and its co-located sleep) were moved back BEFORE the
///    bind, the banner would be delayed by `SLOW_ISLANDS_MS`
///    (> `BANNER_DEADLINE`) and `wait_for_banner_port` would time out → the
///    test fails. Anchoring to the banner-vs-bind ordering — NOT a
///    banner-relative response window — is what makes this falsifiable: a
///    banner-relative deadline does NOT catch a pre-bind build, because the
///    banner floats with the bind.
/// 2. **The serve path does not block on the deferred islands build.** The
///    server answers `GET /__zfb/reload` (200 the instant the router is
///    mounted) within `FIRST_RESPONSE_DEADLINE` of the banner while the
///    islands slow-step is still in flight on the boot task.
///
/// Independent of `ZFB_DEV_TEST_SLOW_DIGEST_MS` (which pins the digest's
/// position); this pins the islands build's. The `dev-loop-basic` fixture
/// ships no `"use client"` islands, so the build produces no bundle — but the
/// injected sleep runs unconditionally right before it, so the guard holds
/// regardless of whether the project has islands.
#[tokio::test(flavor = "multi_thread")]
async fn dev_binds_and_serves_before_slow_islands_step() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let slow = SLOW_ISLANDS_MS.to_string();
    // Eager mode runs the full boot render on the background task too, so the
    // islands slow step sits behind the render — the bind precedes BOTH.
    let spawn_t = Instant::now();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_ISLANDS_MS", slow.as_str()),
            ("ZFB_DEV_EAGER", "1"),
        ],
    );

    // Guarantee 1 (bind precedes islands): the banner follows the bind, so it
    // lands at boot time in the correct ordering. A pre-bind islands build
    // would delay the banner by SLOW_ISLANDS_MS (> BANNER_DEADLINE) and the
    // wait below would time out — that timeout IS the regression signal.
    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    // Explicit, clearly-messaged form of the same guarantee (in case the
    // deadlines are ever retuned): the banner — and the bind it follows —
    // landed well before the injected islands slow-step could have.
    let banner_elapsed = spawn_t.elapsed();
    assert!(
        (banner_elapsed.as_millis() as u64) < SLOW_ISLANDS_MS,
        "ready banner appeared {}ms after spawn, but the injected islands slow-step is \
         {}ms — the islands build (and its sleep) ran BEFORE the bind/banner \
         (bind-before-islands regression, issue #1170).\n{}",
        banner_elapsed.as_millis(),
        SLOW_ISLANDS_MS,
        session.logs(),
    );

    let base = format!("http://localhost:{port}");
    let client = client();

    // Guarantee 2 (serve path is not blocked by the deferred build):
    // `GET /__zfb/reload` is 200 the instant the router is mounted,
    // independent of any render / islands build. Answering within
    // FIRST_RESPONSE_DEADLINE of the banner while the islands slow-step is
    // still in flight proves the serve path does not wait on the build.
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
        "the dev server did not answer GET {url} within {}s of the banner while the \
         islands slow-step was in flight — the serve path is blocking on the deferred \
         islands build (issue #1170).\n{}",
        FIRST_RESPONSE_DEADLINE.as_secs(),
        session.logs(),
    );

    // The deferred slow step is still sleeping; Drop group-kills it.
}

// ---------------------------------------------------------------------------
// Test 5 — bind-before-bundle guard (issue #1182).
// ---------------------------------------------------------------------------

/// Bind-before-bundle guard (issue #1182). The eager esbuild dev bundle
/// (`assemble_and_bundle_dev`, via `boot_dev_renderer`) used to run
/// SYNCHRONOUSLY before `TcpListener::bind`; on a large project its
/// content-snapshot embed + esbuild over the route graph was the dominant
/// pre-bind cost (the residual of #1161 that #1166/#1170 left behind), so
/// first-accept took ~140–250s even in boot-lazy mode with a servable `dist/`.
/// #1182 defers it past the bind, on the deferred boot task, gated on boot-lazy
/// + a servable `dist/` seed.
///
/// ## Mode — boot-lazy + servable `dist/` (the ONLY mode the deferral applies)
///
/// The deferral gate is `ZFB_DEV_BOOT_LAZY=1` (with lazy rendering on, the
/// default) AND `dist_is_servable_seed(dist/)`. So this test sets
/// `ZFB_DEV_BOOT_LAZY=1` and seeds a minimal servable `dist/index.html` BEFORE
/// spawning. A hand-written seed (not a full `zfb build`) is deliberate: the
/// gate only checks for a servable `index.html`, and a real build would add the
/// multi-minute V8 + esbuild cost this very test exists to keep OFF the bind
/// critical path — far too heavy for the PR gate. Without the seed, boot-lazy
/// falls back to eager and the bundle would NOT be deferred (the test would be
/// vacuous).
///
/// ## Falsifiability — two co-located seams, anchored to banner-vs-bind ordering
///
/// The binary reads `ZFB_DEV_TEST_SLOW_BUNDLE_MS` from TWO seams co-located
/// with the boot bundle: one before the DEFERRED bundle (deferred boot task,
/// after bind) and one before the EAGER bundle (`boot_dev_renderer`, before
/// bind). `SLOW_BUNDLE_MS` (> `BANNER_DEADLINE`).
///
/// - Correct (deferred) ordering: the boot takes the scaffold path, so only the
///   DEFERRED seam fires — after the bind/banner. The banner lands at boot time,
///   far under `BANNER_DEADLINE`. ✅
/// - Reverted ordering (the bug): the boot bundle runs eagerly before bind, so
///   the EAGER seam fires before bind, delaying the banner by `SLOW_BUNDLE_MS`
///   (> `BANNER_DEADLINE`) — `wait_for_banner_port` times out and the test
///   fails. ❌ A single deferred-only seam would NOT catch an un-defer revert
///   (the deferred block simply wouldn't run), which is exactly why the eager
///   twin exists.
///
/// Belt-and-braces: `GET /__zfb/reload` answers 200 within
/// `FIRST_RESPONSE_DEADLINE` while the deferred bundle slow-step is still in
/// flight — proof the serve path does not block on the deferred bundle.
#[tokio::test(flavor = "multi_thread")]
async fn dev_binds_and_serves_before_slow_bundle_step() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");

    // Seed a servable `dist/` so the deferral gate (dist_is_servable_seed) is
    // satisfied. `spawn_dev` copies the fixture INTO this tempdir without
    // clearing it (and `dev-loop-basic` ships no `dist/`), so the seed written
    // here survives the copy. The dev server serves this `dist/index.html` for
    // `/` until the deferred renderer publishes.
    let dist = tmp.path().join("dist");
    fs::create_dir_all(&dist).expect("create dist seed dir");
    fs::write(
        dist.join("index.html"),
        "<!doctype html><html><head><title>seed</title></head>\
         <body>servable-dist-seed</body></html>",
    )
    .expect("write dist seed index.html");

    let slow = SLOW_BUNDLE_MS.to_string();
    // Boot-lazy + servable seed => the eager dev bundle is DEFERRED past bind.
    // No ZFB_DEV_EAGER (that would force eager rendering off the lazy path and
    // disable boot-lazy / the deferral entirely).
    let spawn_t = Instant::now();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_BUNDLE_MS", slow.as_str()),
            ("ZFB_DEV_BOOT_LAZY", "1"),
        ],
    );

    // Guarantee 1 (bind precedes the dev bundle): the banner follows the bind,
    // so it lands at boot time in the correct (deferred) ordering. An
    // un-deferred pre-bind bundle would fire the eager seam before bind, delay
    // the banner by SLOW_BUNDLE_MS (> BANNER_DEADLINE), and time out the wait
    // below — that timeout IS the bind-before-bundle regression signal.
    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip (no V8 / no esbuild)
    };
    // Explicit, clearly-messaged form of the same guarantee (in case the
    // deadlines are ever retuned): the banner — and the bind it follows —
    // landed well before the injected bundle slow-step could have.
    let banner_elapsed = spawn_t.elapsed();
    assert!(
        (banner_elapsed.as_millis() as u64) < SLOW_BUNDLE_MS,
        "ready banner appeared {}ms after spawn, but the injected dev-bundle slow-step is \
         {}ms — the eager dev bundle (and its sleep) ran BEFORE the bind/banner \
         (bind-before-bundle regression, issue #1182).\n{}",
        banner_elapsed.as_millis(),
        SLOW_BUNDLE_MS,
        session.logs(),
    );

    let base = format!("http://localhost:{port}");
    let client = client();

    // Guarantee 2 (serve path is not blocked by the deferred bundle):
    // `GET /__zfb/reload` is 200 the instant the router is mounted, independent
    // of any bundle / render. Answering within FIRST_RESPONSE_DEADLINE of the
    // banner while the deferred bundle slow-step is still in flight proves the
    // serve path does not wait on the bundle.
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
        "the dev server did not answer GET {url} within {}s of the banner while the \
         dev-bundle slow-step was in flight — the serve path is blocking on the deferred \
         dev bundle (issue #1182).\n{}",
        FIRST_RESPONSE_DEADLINE.as_secs(),
        session.logs(),
    );

    // The deferred bundle slow step is still sleeping; Drop group-kills it.
}
