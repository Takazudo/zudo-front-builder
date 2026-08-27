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
//! Each guard additionally asserts the serve path answers the SSE live-reload
//! request
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
//! 6. `cold_lazy_binds_and_serves_dev_404_during_held_bundle_window` — the
//!    Cold (seedless) counterpart to Test 5's held-window guard (issue
//!    #1806/#1808): `ZFB_DEV_BOOT_LAZY=cold`, NO `dist/` seed, holds the
//!    deferred bundle open via `ZFB_DEV_TEST_SLOW_BUNDLE_MS`. Falsifies an
//!    accidental eager fallback (the banner must still land fast) and
//!    proves every route serves the controlled `DEV_404_BODY` — WITH the
//!    live-reload script — during the window, never a connection error or
//!    eager-rendered content (Cold has no `dist/` to fall back on at all).
//! 7. `cold_lazy_deferred_publish_broadcasts_sse_and_serves_fresh_200` — a
//!    Cold session, no seed: proves the deferred publish's stale-mark
//!    actually reaches the browser as a real SSE `page` event on
//!    the SSE live-reload stream (not just "GET eventually returns 200" — that alone
//!    would only prove render-on-request, not the broadcast an already-open
//!    tab needs to self-heal), then that the FIRST `GET /` after the event
//!    serves 200 with freshly rendered content — a 200 from nothing. A
//!    brief `ZFB_DEV_TEST_SLOW_BUNDLE_MS` hold (`SSE_SUBSCRIBE_HOLD_MS`)
//!    guarantees the SSE subscription registers before the publish, since
//!    the broadcast has no replay buffer.
//! 8. `cold_lazy_broken_bundle_recovers_after_source_fix` — the live e2e
//!    proof of the #1809 cold-bootstrap recovery mechanism: `pages/index.tsx`
//!    is corrupted in the prepared root BEFORE `zfb dev` is spawned (no
//!    wall-clock race against the boot task), so the FIRST deferred bundle
//!    fails; asserts the controlled 404 plus the error-level cold-lazy
//!    failure message, then restores the valid source and asserts recovery
//!    — an SSE `page` event, the "cold-lazy bootstrap recovered" info
//!    line, and a first-request 200.
//! 9. `dev_200_document_declares_and_serves_islands_module` — readiness guard
//!    for the boot publication window: readiness stays false while the
//!    injected islands slow step is held, remains false after the staged entry
//!    is written but before the document generation, then turns true only once
//!    the rendered 200 document and its browser-requested islands entry are
//!    both available.
//! 10. `dev_retains_previous_islands_chunk_generations_for_open_documents` —
//!     real watcher ticks prove content-hashed lazy chunks survive a
//!     document-only tick and two later islands generations, then are pruned
//!     on the third later islands generation.
//! 11. `dev_tick_client_script_publication_add_remove_ordering` — real watcher
//!     ticks prove a newly-added client entry is published before the page
//!     write, and a removed entry remains served through the transition page
//!     write before a later publication prunes it.
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

use zfb_test_utils::{
    locate_esbuild, next_sse_event_name, open_sse, probe_module_entries, zfb_binary,
    CrossBinaryE2eLock,
};

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

/// Test 7 (`cold_lazy_deferred_publish_broadcasts_sse_and_serves_fresh_200`)
/// SSE-subscribe hold: reusing the SAME `ZFB_DEV_TEST_SLOW_BUNDLE_MS` seam as
/// `SLOW_BUNDLE_MS` above, but for a different purpose. `SLOW_BUNDLE_MS` must
/// exceed `BANNER_DEADLINE` (it's a bind-ordering guard); this constant has no
/// such requirement — it only needs to outlast the test process reading the
/// ready banner from the log file and completing the SSE HTTP
/// handshake, done right after the banner and BEFORE the deferred boot
/// task's bundle call (which sleeps for this long immediately before
/// publishing). Without this hold, a small/fast-bundling fixture could
/// publish and broadcast the ONE `page` event before the test's SSE
/// subscription registers — the broadcast has no replay, so a missed event
/// means the test waits out the full deadline and fails despite correct
/// server behavior (codex-review finding, issue #1811). Sleep-then-publish,
/// inside the SAME boot hook, is what makes the subscribe race-free: any
/// subscription completed before the window elapses is guaranteed to be
/// registered when the bundle finally publishes.
const SSE_SUBSCRIBE_HOLD_MS: u64 = 5_000;

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

fn fixture_dir_named(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
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

/// Prepare a fresh fixture copy under `tmp`, WITHOUT spawning `zfb dev`.
/// Split out of `spawn_dev` (below) so a test can edit a file in the
/// prepared root — e.g. Test 8's deliberately-broken `pages/index.tsx` —
/// deterministically BEFORE the process ever starts, with no race against
/// the boot task (see `spawn_dev_in_root`'s doc comment).
fn prepare_dev_root(tmp: &tempfile::TempDir) -> PathBuf {
    prepare_dev_root_from_fixture(tmp, "dev-loop-basic")
}

fn prepare_dev_root_from_fixture(tmp: &tempfile::TempDir, fixture_name: &str) -> PathBuf {
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir_named(fixture_name), &root).expect("copy fixture into tempdir");
    root
}

/// Spawn `zfb dev --port 0` over an ALREADY-PREPARED root (see
/// `prepare_dev_root`), captured to log files, in its own process group.
fn spawn_dev_in_root(root: &Path, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(root)
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
        .env_remove("ZFB_DEV_TEST_SLOW_POST_ISLANDS_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_BUNDLE_MS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        root: root.to_path_buf(),
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Spawn `zfb dev --port 0` over a fresh copy of the fixture, captured
/// to log files, in its own process group.
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
    let root = prepare_dev_root(tmp);
    spawn_dev_in_root(&root, esbuild, extra_env)
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

fn log_line_count(session: &DevSession, needle: &str) -> usize {
    session
        .logs()
        .lines()
        .filter(|line| line.contains(needle))
        .count()
}

async fn wait_for_log_line(session: &DevSession, needle: &str, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if session.logs().lines().any(|line| line.contains(needle)) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "did not observe timing line {needle:?} within {}s.\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

async fn wait_for_log_line_count_above(
    session: &DevSession,
    needle: &str,
    minimum_count: usize,
    deadline: Duration,
) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if log_line_count(session, needle) > minimum_count {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "did not observe timing line {needle:?} above count {minimum_count} within {}s.\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

fn islands_chunk_names(entry: &str) -> Vec<String> {
    const PREFIX: &str = "./islands-chunk-";
    let mut names = Vec::new();
    let mut remaining = entry;
    while let Some(start) = remaining.find(PREFIX) {
        let rest = &remaining[start + 2..];
        let end = rest
            .find(".js")
            .unwrap_or_else(|| panic!("split chunk reference had no .js suffix:\n{entry}"));
        let name = rest[..end + ".js".len()].to_owned();
        if !names.contains(&name) {
            names.push(name);
        }
        remaining = &rest[end + ".js".len()..];
    }
    assert!(
        !names.is_empty(),
        "islands entry did not reference a split chunk:\n{entry}"
    );
    names
}

fn tick_kind_line_count(session: &DevSession, required_names: &[&str]) -> usize {
    session
        .logs()
        .lines()
        .filter(|line| {
            line.contains("[zfb-timing] tick():")
                && required_names.iter().all(|name| line.contains(name))
        })
        .count()
}

async fn wait_for_tick_kinds(
    session: &DevSession,
    required_names: &[&str],
    minimum_count: usize,
    deadline: Duration,
) -> String {
    let start = Instant::now();
    while start.elapsed() < deadline {
        let logs = session.logs();
        let matching = logs
            .lines()
            .filter(|line| {
                line.contains("[zfb-timing] tick():")
                    && required_names.iter().all(|name| line.contains(name))
            })
            .count();
        if matching > minimum_count {
            return logs;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "did not observe a coalesced tick containing {:?} after count {} within {}s.\n{}",
        required_names,
        minimum_count,
        deadline.as_secs(),
        session.logs(),
    );
}

async fn wait_for_tick_publication(
    session: &DevSession,
    client_before: usize,
    page_before: usize,
    deadline: Duration,
) -> String {
    const CLIENT_MARKER: &str = "[zfb-timing] tick: client scripts published";
    const PAGE_MARKER: &str = "[zfb-timing] tick: page write complete";
    let start = Instant::now();
    while start.elapsed() < deadline {
        let logs = session.logs();
        let client_count = logs
            .lines()
            .filter(|line| line.contains(CLIENT_MARKER))
            .count();
        let page_count = logs
            .lines()
            .filter(|line| line.contains(PAGE_MARKER))
            .count();
        if client_count > client_before && page_count > page_before {
            return logs;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "did not observe one client-publication + page-write marker pair within {}s \
         (before counts: client={client_before}, page={page_before}).\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

fn assert_tick_publication_order(logs: &str, client_before: usize, page_before: usize) {
    const CLIENT_MARKER: &str = "[zfb-timing] tick: client scripts published";
    const PAGE_MARKER: &str = "[zfb-timing] tick: page write complete";
    let lines: Vec<&str> = logs.lines().collect();
    let client_at = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(CLIENT_MARKER))
        .nth(client_before)
        .map(|(index, _)| index)
        .expect("new client-publication marker in captured logs");
    let page_at = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(PAGE_MARKER))
        .nth(page_before)
        .map(|(index, _)| index)
        .expect("new page-write marker in captured logs");
    assert!(
        client_at < page_at,
        "client-script publication must precede the page-write boundary; \
         client marker at line {client_at}, page marker at line {page_at}.\n{logs}"
    );
}

async fn ready_json(client: &reqwest::Client, url: &str) -> serde_json::Value {
    let response = client.get(url).send().await.expect("GET /__zfb/ready");
    assert_eq!(response.status().as_u16(), 200, "readiness endpoint status");
    serde_json::from_str(&response.text().await.expect("read readiness JSON body"))
        .expect("valid readiness JSON")
}

fn ready_generation(body: &serde_json::Value) -> u64 {
    body["generation"]
        .as_u64()
        .expect("readiness JSON generation")
}

async fn wait_for_publication_ready(
    client: &reqwest::Client,
    ready_url: &str,
    minimum_generation: u64,
    deadline: Duration,
    session: &DevSession,
) -> serde_json::Value {
    let start = Instant::now();
    let mut last = serde_json::Value::Null;
    while start.elapsed() < deadline {
        last = ready_json(client, ready_url).await;
        if last["ready"] == true && ready_generation(&last) > minimum_generation {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "readiness endpoint did not advance above generation {minimum_generation} within \
         {}s; last snapshot: {last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

async fn wait_for_client_script_urls(
    client: &reqwest::Client,
    ready_url: &str,
    minimum_generation: u64,
    expected_status: &str,
    expected_urls: &[&str],
    deadline: Duration,
    session: &DevSession,
) -> serde_json::Value {
    let expected_urls = serde_json::json!(expected_urls);
    let start = Instant::now();
    while start.elapsed() < deadline {
        let body = ready_json(client, ready_url).await;
        let urls_match = body["client_scripts"]["urls"] == expected_urls;
        if ready_generation(&body) > minimum_generation
            && body["client_scripts"]["status"] == expected_status
            && urls_match
        {
            return body;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "readiness endpoint did not reach client_scripts status {expected_status:?} and \
         URLs {expected_urls} above generation {minimum_generation} within {}s.\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client")
}

async fn assert_asset_status(
    client: &reqwest::Client,
    base: &str,
    asset_name: &str,
    expected_status: u16,
    session: &DevSession,
) -> String {
    let response = client
        .get(format!("{base}/assets/{asset_name}"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("request asset {asset_name}: {error}\n{}", session.logs()));
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status,
        expected_status,
        "GET /assets/{asset_name} returned {status}, expected {expected_status}; body:\n{body}\n{}",
        session.logs(),
    );
    body
}

async fn islands_chunk_with_marker(
    client: &reqwest::Client,
    base: &str,
    entry: &str,
    marker: &str,
    session: &DevSession,
) -> String {
    let names = islands_chunk_names(entry);
    for name in &names {
        let body = assert_asset_status(client, base, name, 200, session).await;
        if body.contains(marker) {
            return name.clone();
        }
    }
    panic!(
        "none of the referenced islands chunks contained marker {marker:?}; chunks={names:?}\n{}",
        session.logs(),
    );
}

/// Subscribe to the dev server's SSE live-reload endpoint. Must be called
/// BEFORE the event it is meant to observe — the channel is a broadcast, not
/// a queue, so an event that fires before subscription is gone forever.
/// Mirrors `dev_dep_invalidation_1284_e2e.rs`'s helper of the same
/// name/contract.
async fn subscribe_sse(base: &str) -> reqwest::Response {
    let resp = open_sse(base).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "SSE live-reload endpoint must answer 200"
    );
    resp
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

    // The SSE live-reload endpoint answers 200 the
    // instant the router is mounted, independent of any render/digest.
    // We send the request with a short per-request timeout so a never-
    // bound port can't hang the test; success within
    // FIRST_RESPONSE_DEADLINE (< SLOW_DIGEST_MS) is the proof.
    let url = format!("{base}/{}", ["__zfb", "reload"].join("/"));
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
    let url = format!("{base}/{}", ["__zfb", "reload"].join("/"));
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
///    server answers the SSE live-reload request (200 the instant the router is
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
    // The SSE live-reload endpoint is 200 the instant the router is mounted,
    // independent of any render / islands build. Answering within
    // FIRST_RESPONSE_DEADLINE of the banner while the islands slow-step is
    // still in flight proves the serve path does not wait on the build.
    let url = format!("{base}/{}", ["__zfb", "reload"].join("/"));
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
// Test 9 — dev hydration readiness during the held islands window (#2552).
// ---------------------------------------------------------------------------

/// Regression guard for the complete boot publication window: while the
/// injected islands build is held, readiness must remain pending and the page
/// must not be a real document. Once the staged entry is written, the
/// post-islands hold creates an entry-written/document-not-yet-published
/// phase; readiness must still remain false there. Only after the timing
/// marker for the boot render has landed may readiness become true, and the
/// returned document must then pass the browser-shaped module-entry probe.
#[tokio::test(flavor = "multi_thread")]
async fn dev_200_document_declares_and_serves_islands_module() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = prepare_dev_root_from_fixture(&tmp, "dev-islands-entry-probe");
    // These finite holds are test-local phase delimiters, not wall-clock
    // readiness proxies. The islands hold gives us the entry-pending phase;
    // the post-islands hold is deliberately after the staged entry write and
    // before route/document publication, so a readiness signal keyed only to
    // an entry returning 200 cannot pass this test.
    const HELD_ISLANDS_MS: u64 = 5_000;
    const HELD_POST_ISLANDS_MS: u64 = 15_000;
    let slow_islands = HELD_ISLANDS_MS.to_string();
    let slow_post_islands = HELD_POST_ISLANDS_MS.to_string();
    let mut session = spawn_dev_in_root(
        &root,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_ISLANDS_MS", slow_islands.as_str()),
            (
                "ZFB_DEV_TEST_SLOW_POST_ISLANDS_MS",
                slow_post_islands.as_str(),
            ),
            ("ZFB_DEV_TIMING", "1"),
            ("ZFB_DEV_EAGER", "1"),
        ],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    let base = format!("http://localhost:{port}");
    let page_url = format!("{base}/");
    let ready_url = format!("{base}/__zfb/ready");
    let client = client();

    // Phase 1 — the injected islands step is still held. The endpoint is the
    // authoritative phase signal: an entry-pending state cannot be confused
    // with a merely slow HTTP response. The first page response must not be a
    // real rendered document during this phase.
    let pending = ready_json(&client, &ready_url).await;
    assert_eq!(
        pending["ready"], false,
        "readiness must stay false while islands are held"
    );
    assert_eq!(pending["islands"]["status"], "pending");
    let pending_page = client
        .get(&page_url)
        .send()
        .await
        .expect("request page during held islands phase");
    assert_ne!(
        pending_page.status().as_u16(),
        200,
        "the document must not publish before the held islands entry; status={}\n{}",
        pending_page.status(),
        session.logs(),
    );

    // Phase 2 — the staged islands entry has been written, but the injected
    // post-islands hold keeps route/document work before the publication
    // boundary. The file itself must already be browser-reachable. This is
    // the critical BOTH-side assertion: an entry that is directly reachable
    // must not make readiness true on its own.
    wait_for_log_line(
        &session,
        "[zfb-timing] boot: islands published",
        RENDER_DEADLINE,
    )
    .await;
    let islands_response = client
        .get(format!("{base}/assets/islands.js"))
        .send()
        .await
        .expect("request staged islands entry during post-islands hold");
    assert_eq!(
        islands_response.status().as_u16(),
        200,
        "the staged islands entry must be reachable before document commit; {}",
        session.logs(),
    );
    let entry_published = ready_json(&client, &ready_url).await;
    assert_eq!(
        entry_published["ready"], false,
        "staged entry reachability alone is not document readiness"
    );
    assert_eq!(
        entry_published["documents"], "pending",
        "document generation must remain pending during the post-islands hold"
    );

    // Phase 3 — only after the boot-render publication marker may the
    // readiness signal turn green. This is anchored to the production timing
    // boundary rather than to a guessed sleep duration.
    wait_for_log_line(
        &session,
        "[zfb-timing] boot: render complete",
        RENDER_DEADLINE,
    )
    .await;
    let published = ready_json(&client, &ready_url).await;
    assert_eq!(published["ready"], true);
    assert_eq!(published["islands"]["status"], "published");
    let response = client
        .get(&page_url)
        .send()
        .await
        .expect("request published islands document");
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status,
        200,
        "the fixture document must be served with 200; body:\n{body}\n{}",
        session.logs(),
    );
    assert!(
        body.contains("<h1>dev-islands-entry-probe</h1>") && body.contains("data-zfb-island"),
        concat!(
            "the 200 response must be the rendered islands fixture, not a different or partial ",
            "document; body:\n{}\n{}"
        ),
        body,
        session.logs(),
    );

    let probes = probe_module_entries(&client, &body, &page_url)
        .await
        .expect("probe same-origin module entries declared by the 200 document");
    assert!(
        !probes.is_empty(),
        concat!(
            "200 HTML must declare an islands module entry after the boot ",
            "publication boundary; probes={:?}\n{}"
        ),
        probes,
        session.logs(),
    );
    assert!(
        probes
            .iter()
            .any(|probe| probe.url.path().ends_with("/assets/islands.js")),
        concat!(
            "200 HTML must declare the islands module entry, not only unrelated ",
            "module scripts; probes={:?}\n{}"
        ),
        probes,
        session.logs(),
    );
    for probe in probes {
        assert_eq!(
            probe.status.as_u16(),
            200,
            concat!(
                "every module entry declared by the 200 document must answer 200; ",
                "{} returned {}\n{}"
            ),
            probe.url,
            probe.status,
            session.logs(),
        );
    }
}

// ---------------------------------------------------------------------------
// Test 10 — islands companion retention across document-only ticks (#2587).
// ---------------------------------------------------------------------------

/// Content-hashed lazy chunks referenced by an already-open document must
/// survive a document-only boundary and the next two islands generations.
/// The third later islands generation prunes the oldest chunk, proving the
/// retention window is bounded rather than append-only.
#[tokio::test(flavor = "multi_thread")]
async fn dev_retains_previous_islands_chunk_generations_for_open_documents() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    const ISLANDS_MARKER: &str = "[zfb-timing] tick: islands published";
    const PAGE_MARKER: &str = "[zfb-timing] tick: page write complete";

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = prepare_dev_root_from_fixture(&tmp, "dev-islands-chunk-retention");
    let lazy_part_path = root.join("components/lazy-part.tsx");
    let page_path = root.join("pages/index.tsx");
    let mut session = spawn_dev_in_root(
        &root,
        &esbuild,
        &[("ZFB_DEV_EAGER", "1"), ("ZFB_DEV_TIMING", "1")],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    let base = format!("http://localhost:{port}");
    let ready_url = format!("{base}/__zfb/ready");
    let client = client();

    // Boot: inspect the emitted assets and establish the document's original
    // lazy-chunk obligation before any watcher tick.
    wait_for_log_line(
        &session,
        "[zfb-timing] boot: islands published",
        RENDER_DEADLINE,
    )
    .await;
    wait_for_log_line(
        &session,
        "[zfb-timing] boot: render complete",
        RENDER_DEADLINE,
    )
    .await;
    let boot_ready = ready_json(&client, &ready_url).await;
    assert_eq!(boot_ready["ready"], true, "boot publication readiness");
    let mut generation = ready_generation(&boot_ready);
    let entry0 = assert_asset_status(&client, &base, "islands.js", 200, &session).await;
    let chunk0 =
        islands_chunk_with_marker(&client, &base, &entry0, "lazy part boot", &session).await;
    let emitted_assets: Vec<String> = fs::read_dir(root.join(".zfb-build/dev-assets/assets"))
        .expect("inspect emitted dev assets after boot")
        .map(|entry| {
            entry
                .expect("read emitted dev asset")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        emitted_assets.iter().any(|name| name == &chunk0),
        "boot assets must contain referenced split chunk {chunk0:?}; assets={emitted_assets:?}"
    );
    assert_asset_status(&client, &base, &chunk0, 200, &session).await;

    // First islands generation: the stable entry points at a new chunk, but
    // the original document's chunk remains reachable for one lap.
    let islands_before = log_line_count(&session, ISLANDS_MARKER);
    fs::write(
        &lazy_part_path,
        "export const lazyPart = \"lazy part generation one\";\n",
    )
    .expect("write first lazy-part generation");
    wait_for_log_line_count_above(&session, ISLANDS_MARKER, islands_before, RENDER_DEADLINE).await;
    let ready =
        wait_for_publication_ready(&client, &ready_url, generation, RENDER_DEADLINE, &session)
            .await;
    generation = ready_generation(&ready);
    let entry1 = assert_asset_status(&client, &base, "islands.js", 200, &session).await;
    let chunk1 = islands_chunk_with_marker(
        &client,
        &base,
        &entry1,
        "lazy part generation one",
        &session,
    )
    .await;
    assert_ne!(
        chunk1, chunk0,
        "lazy-part edit must change the chunk hash; entry before:\n{entry0}\nentry after:\n{entry1}"
    );
    assert_asset_status(&client, &base, &chunk0, 200, &session).await;

    // Document-only boundary: publication advances, but islands must not
    // re-bundle and the retained original chunk must not be cleared.
    let page_before = log_line_count(&session, PAGE_MARKER);
    let islands_before_page = log_line_count(&session, ISLANDS_MARKER);
    let original_page = fs::read_to_string(&page_path).expect("read retention fixture page");
    let edited_page = original_page.replacen(
        "<h1>dev-islands-chunk-retention</h1>",
        "<h1>dev-islands-chunk-retention page edit</h1>",
        1,
    );
    assert_ne!(
        edited_page, original_page,
        "page edit fixture insertion point"
    );
    fs::write(&page_path, edited_page).expect("write document-only generation");
    wait_for_log_line_count_above(&session, PAGE_MARKER, page_before, RENDER_DEADLINE).await;
    let ready =
        wait_for_publication_ready(&client, &ready_url, generation, RENDER_DEADLINE, &session)
            .await;
    generation = ready_generation(&ready);
    assert_eq!(
        log_line_count(&session, ISLANDS_MARKER),
        islands_before_page,
        "a page-only edit must not publish an islands generation\n{}",
        session.logs(),
    );
    assert_asset_status(&client, &base, &chunk0, 200, &session).await;

    // Second later islands generation: chunk0 is the K=2 retained lap and
    // must still be available to an open document.
    let islands_before = log_line_count(&session, ISLANDS_MARKER);
    fs::write(
        &lazy_part_path,
        "export const lazyPart = \"lazy part generation two is longer\";\n",
    )
    .expect("write second lazy-part generation");
    wait_for_log_line_count_above(&session, ISLANDS_MARKER, islands_before, RENDER_DEADLINE).await;
    let ready =
        wait_for_publication_ready(&client, &ready_url, generation, RENDER_DEADLINE, &session)
            .await;
    generation = ready_generation(&ready);
    assert_asset_status(&client, &base, &chunk0, 200, &session).await;

    // Third later islands generation: chunk0 falls outside the bounded
    // window, while chunk1 and the newest chunk stay served.
    let islands_before = log_line_count(&session, ISLANDS_MARKER);
    fs::write(
        &lazy_part_path,
        "export const lazyPart = \"lazy part generation three is longer still\";\n",
    )
    .expect("write third lazy-part generation");
    wait_for_log_line_count_above(&session, ISLANDS_MARKER, islands_before, RENDER_DEADLINE).await;
    let ready =
        wait_for_publication_ready(&client, &ready_url, generation, RENDER_DEADLINE, &session)
            .await;
    let final_generation = ready_generation(&ready);
    assert!(
        final_generation > generation,
        "third islands generation bump"
    );
    let newest_entry = assert_asset_status(&client, &base, "islands.js", 200, &session).await;
    let newest_chunk = islands_chunk_with_marker(
        &client,
        &base,
        &newest_entry,
        "lazy part generation three is longer still",
        &session,
    )
    .await;
    assert_ne!(newest_chunk, chunk1, "third edit must emit a newer chunk");
    assert_asset_status(&client, &base, &chunk0, 404, &session).await;
    assert_asset_status(&client, &base, &newest_chunk, 200, &session).await;
    assert_asset_status(&client, &base, &chunk1, 200, &session).await;
}

// ---------------------------------------------------------------------------
// Test 11 — client-script add/remove publication ordering (#2555).
// ---------------------------------------------------------------------------

/// A real watcher tick must publish a newly discovered client entry before
/// the page write that references it. On removal, the old entry remains
/// reachable through the transition page write and is pruned only by the
/// later committed generation.
#[tokio::test(flavor = "multi_thread")]
async fn dev_tick_client_script_publication_add_remove_ordering() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = prepare_dev_root(&tmp);
    let index_path = root.join("pages/index.tsx");
    let original_index = fs::read_to_string(&index_path).expect("read dev-loop-basic index");
    let tagged_index = original_index
        .replacen(
            "import { SharedNote } from \"../components/shared-note\";",
            "import { clientScript } from \"@takazudo/zfb\";\nimport { SharedNote } from \"../components/shared-note\";",
            1,
        )
        .replacen(
            "<title>dev-loop-basic fixture</title>",
            "<title>dev-loop-basic fixture</title>\n        <script type=\"module\" src={clientScript(\"order\")} />",
            1,
        );
    assert_ne!(
        tagged_index, original_index,
        "the dev-loop-basic fixture must expose the clientScript insertion points"
    );
    let order_source = "export const ORDER_CLIENT_MARKER = \"ORDER_CLIENT_V1\";\n";

    let mut session = spawn_dev_in_root(&root, &esbuild, &[("ZFB_DEV_TIMING", "1")]);
    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    let base = format!("http://localhost:{port}");
    let page_url = format!("{base}/");
    let ready_url = format!("{base}/__zfb/ready");
    let order_url = format!("{base}/assets/client/order.js");
    let client = client();

    // Boot is deliberately clean: there is no client entry and no page tag.
    // The add transition below writes both synchronously, before polling, so
    // zfb-watcher's 50ms debounce receives one coalesced batch containing the
    // entry creation and the page reference.
    wait_for_log_line(
        &session,
        "[zfb-timing] boot: render complete",
        RENDER_DEADLINE,
    )
    .await;
    let baseline = ready_json(&client, &ready_url).await;
    assert_eq!(baseline["ready"], true);
    assert_eq!(baseline["client_scripts"]["status"], "not_expected");
    let baseline_generation = ready_generation(&baseline);
    let page = client
        .get(&page_url)
        .send()
        .await
        .expect("request clean boot document");
    assert_eq!(page.status().as_u16(), 200, "clean boot document status");
    let page_body = page.text().await.expect("read clean boot document");
    assert!(
        !page_body.contains("/assets/client/order.js"),
        "clean boot document must not name the not-yet-added client entry"
    );

    // Addition: create the entry, then add the page reference in one
    // synchronous write pair. The timing kind line pins that both paths were
    // actually coalesced into the same watcher tick.
    let client_marker = "[zfb-timing] tick: client scripts published";
    let page_marker = "[zfb-timing] tick: page write complete";
    let client_before_add = log_line_count(&session, client_marker);
    let page_before_add = log_line_count(&session, page_marker);
    let add_batch_before = tick_kind_line_count(&session, &["order.client.ts", "index.tsx"]);
    let order_path = root.join("pages/order.client.ts");
    fs::write(&order_path, order_source).expect("add order.client.ts");
    fs::write(&index_path, &tagged_index).expect("add clientScript tag");
    wait_for_tick_kinds(
        &session,
        &["order.client.ts", "index.tsx"],
        add_batch_before,
        RENDER_DEADLINE,
    )
    .await;
    let add_logs = wait_for_tick_publication(
        &session,
        client_before_add,
        page_before_add,
        RENDER_DEADLINE,
    )
    .await;
    assert_tick_publication_order(&add_logs, client_before_add, page_before_add);
    let added = wait_for_client_script_urls(
        &client,
        &ready_url,
        baseline_generation,
        "published",
        &["/assets/client/order.js"],
        RENDER_DEADLINE,
        &session,
    )
    .await;
    let added_generation = ready_generation(&added);
    assert!(
        added_generation > baseline_generation,
        "client-script addition must advance the publication generation"
    );
    let added_entry = client
        .get(&order_url)
        .send()
        .await
        .expect("request added client entry");
    assert_eq!(added_entry.status().as_u16(), 200);
    let added_body = added_entry.text().await.expect("read added client entry");
    assert!(
        added_body.contains("ORDER_CLIENT_MARKER") || added_body.contains("ORDER_CLIENT_V1"),
        "the added client entry must contain its fixture marker"
    );
    let added_page = client
        .get(&page_url)
        .send()
        .await
        .expect("request added document");
    assert_eq!(added_page.status().as_u16(), 200);
    let added_page_body = added_page.text().await.expect("read added document");
    assert!(
        added_page_body.contains("/assets/client/order.js"),
        "the committed added document must name the published client entry"
    );

    // Removal: remove the page tag and delete the entry in one synchronous
    // write pair. The old URL must remain served after this successful
    // removal generation; a later successful client generation is the point
    // at which the previous lazy fallback may be pruned.
    let untagged_index = tagged_index
        .replace("import { clientScript } from \"@takazudo/zfb\";\n", "")
        .replace(
            "        <script type=\"module\" src={clientScript(\"order\")} />\n",
            "",
        );
    assert_ne!(
        untagged_index, tagged_index,
        "the clientScript tag removal must change the fixture"
    );
    let client_before_remove = log_line_count(&session, client_marker);
    let page_before_remove = log_line_count(&session, page_marker);
    let remove_batch_before = tick_kind_line_count(&session, &["order.client.ts", "index.tsx"]);
    fs::write(&index_path, &untagged_index).expect("remove clientScript tag");
    fs::remove_file(&order_path).expect("remove order.client.ts");
    wait_for_tick_kinds(
        &session,
        &["order.client.ts", "index.tsx"],
        remove_batch_before,
        RENDER_DEADLINE,
    )
    .await;
    let remove_logs = wait_for_tick_publication(
        &session,
        client_before_remove,
        page_before_remove,
        RENDER_DEADLINE,
    )
    .await;
    assert_tick_publication_order(&remove_logs, client_before_remove, page_before_remove);
    let removed = wait_for_client_script_urls(
        &client,
        &ready_url,
        added_generation,
        "not_expected",
        &[],
        RENDER_DEADLINE,
        &session,
    )
    .await;
    assert!(
        ready_generation(&removed) > added_generation,
        "client-script removal must advance the publication generation"
    );
    let removed_page = client
        .get(&page_url)
        .send()
        .await
        .expect("request removal document");
    assert_eq!(removed_page.status().as_u16(), 200);
    let removed_page_body = removed_page.text().await.expect("read removal document");
    assert!(
        !removed_page_body.contains("/assets/client/order.js"),
        "the committed removal document must not name the old entry"
    );
    let retained_entry = client
        .get(&order_url)
        .send()
        .await
        .expect("request retained removed client entry");
    assert_eq!(
        retained_entry.status().as_u16(),
        200,
        "the removed entry must remain servable after the removal page commit"
    );

    // A later successful client generation is the cleanup boundary. Its
    // marker/page-write order is checked as well, and the old order asset is
    // no longer retained afterward.
    let cleanup_path = root.join("pages/cleanup.client.ts");
    let client_before_cleanup = log_line_count(&session, client_marker);
    let page_before_cleanup = log_line_count(&session, page_marker);
    fs::write(
        &cleanup_path,
        "export const CLEANUP_CLIENT_MARKER = \"CLEANUP_CLIENT_V1\";\n",
    )
    .expect("add cleanup.client.ts");
    let cleanup_logs = wait_for_tick_publication(
        &session,
        client_before_cleanup,
        page_before_cleanup,
        RENDER_DEADLINE,
    )
    .await;
    assert_tick_publication_order(&cleanup_logs, client_before_cleanup, page_before_cleanup);
    let cleaned = wait_for_client_script_urls(
        &client,
        &ready_url,
        ready_generation(&removed),
        "published",
        &["/assets/client/cleanup.js"],
        RENDER_DEADLINE,
        &session,
    )
    .await;
    assert!(
        ready_generation(&cleaned) > ready_generation(&removed),
        "cleanup client generation must advance publication generation"
    );
    let pruned_entry = client
        .get(&order_url)
        .send()
        .await
        .expect("request pruned client entry");
    assert_ne!(
        pruned_entry.status().as_u16(),
        200,
        "the removed entry must be pruned after the later successful client generation"
    );

    let final_page = client
        .get(&page_url)
        .send()
        .await
        .expect("request final document");
    assert_eq!(final_page.status().as_u16(), 200);
    let final_body = final_page.text().await.expect("read final document");
    assert!(
        !final_body.contains("/assets/client/order.js"),
        "the final committed document must not name the removed entry"
    );
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
/// Belt-and-braces: the SSE live-reload endpoint answers 200 within
/// `FIRST_RESPONSE_DEADLINE` while the deferred bundle slow-step is still in
/// flight — proof the serve path does not block on the deferred bundle. And
/// (issue #1390) `GET /` returns 200 with the seeded `dist/index.html` body
/// during that same window — proof the boot-lazy prebuilt-`dist/` seed leg
/// (`serve_page`'s Dev-gated `dist_root` fallback) actually serves pages,
/// not just 404s, before the deferred renderer publishes.
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
    // The SSE live-reload endpoint is 200 the instant the router is mounted, independent
    // of any bundle / render. Answering within FIRST_RESPONSE_DEADLINE of the
    // banner while the deferred bundle slow-step is still in flight proves the
    // serve path does not wait on the bundle.
    let url = format!("{base}/{}", ["__zfb", "reload"].join("/"));
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

    // Guarantee 3 (issue #1390 — the boot-lazy prebuilt-`dist/` seed is
    // ACTUALLY served during the deferred window, not merely documented):
    // `GET /` must return 200 with the seed body written above. During this
    // window the renderer is not published (the deferred bundle slow-step is
    // still sleeping) and no route has re-rendered into the dev HTML root, so
    // the ONLY way `/` resolves to 200 is the Dev-gated `dist_root` seed leg
    // in `serve_page` (`PageCache → html_root → public_root → dist_root → 404`).
    // Before #1390 that leg did not exist and this GET returned the dev 404 —
    // so this assertion is the e2e falsification of the missing leg.
    let root_url = format!("{base}/");
    let root_start = Instant::now();
    let mut seed_body: Option<String> = None;
    while root_start.elapsed() < FIRST_RESPONSE_DEADLINE {
        if let Ok(resp) = client.get(&root_url).send().await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            assert_eq!(
                status,
                200,
                "GET / must serve the prebuilt dist/ seed with 200 during the deferred \
                 window (issue #1390); got status {status}, body:\n{body}\n{}",
                session.logs(),
            );
            seed_body = Some(body);
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let seed_body = seed_body.unwrap_or_else(|| {
        panic!(
            "the dev server did not answer GET / within {}s of the banner while the \
             deferred bundle slow-step was in flight (issue #1390).\n{}",
            FIRST_RESPONSE_DEADLINE.as_secs(),
            session.logs(),
        )
    });
    assert!(
        seed_body.contains("servable-dist-seed"),
        "GET / must serve the prebuilt dist/index.html seed body during the deferred \
         window (issue #1390 — the Dev-gated dist_root leg in serve_page); got body:\n{seed_body}\n{}",
        session.logs(),
    );

    // The deferred bundle slow step is still sleeping; Drop group-kills it.
}

// ---------------------------------------------------------------------------
// Test 6 — cold-lazy held-bundle-window guard (issue #1806/#1808).
// ---------------------------------------------------------------------------

/// Cold-lazy (`ZFB_DEV_BOOT_LAZY=cold`) counterpart to Test 5's held-window
/// guard, seedless: NO `dist/` is written before spawning. Cold defers the
/// dev bundle exactly like Auto+seeded-dist does (`defer_dev_bundle_decision`'s
/// Cold conjunct — issue #1808), so the SAME `ZFB_DEV_TEST_SLOW_BUNDLE_MS`
/// seam applies; this test holds that window open and asserts TWO things:
///
/// 1. **No accidental eager fallback.** The ready banner still lands at boot
///    time (well under `BANNER_DEADLINE`), proving Cold takes the deferred
///    branch even without a seed. Falsifiability: if
///    `defer_dev_bundle_decision`'s Cold conjunct were reverted (Cold
///    treated like Auto, requiring a servable seed it doesn't have), the
///    eager pre-bind bundle path in `boot_dev_renderer` would run instead —
///    firing the EAGER `ZFB_DEV_TEST_SLOW_BUNDLE_MS` seam before bind and
///    delaying the banner past `BANNER_DEADLINE`, timing out
///    `wait_for_banner_port`. Observed directly (see the PR/report).
/// 2. **The controlled 404, not a connection error or eager content.**
///    `GET /` during the held window must return the controlled
///    `DEV_404_BODY` — WITH the live-reload script, so an open tab
///    self-heals via the post-publish `pages_stale` broadcast (Tests 7/8) —
///    never hang, never a connection refusal, never eager-rendered content.
///    Cold has no `dist/` seed at all (unlike Test 5's Auto + seeded-dist
///    window, which serves the seed here instead).
///
/// Terminates without waiting for the window to close — Drop group-kills the
/// whole process tree while the deferred bundle is still sleeping.
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_binds_and_serves_dev_404_during_held_bundle_window() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    // No dist/ seed written: cold-lazy is seedless by design (issue #1806).
    let slow = SLOW_BUNDLE_MS.to_string();
    let spawn_t = Instant::now();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_BUNDLE_MS", slow.as_str()),
            ("ZFB_DEV_BOOT_LAZY", "cold"),
        ],
    );

    // Guarantee 1 (no accidental eager fallback): see doc comment above.
    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip (no V8 / no esbuild)
    };
    let banner_elapsed = spawn_t.elapsed();
    assert!(
        (banner_elapsed.as_millis() as u64) < SLOW_BUNDLE_MS,
        "ready banner appeared {}ms after spawn, but the injected dev-bundle slow-step is \
         {}ms — cold-lazy (ZFB_DEV_BOOT_LAZY=cold) ran the deferred bundle (and its sleep) \
         BEFORE the bind/banner, an accidental eager fallback (issue #1808).\n{}",
        banner_elapsed.as_millis(),
        SLOW_BUNDLE_MS,
        session.logs(),
    );

    let base = format!("http://localhost:{port}");
    let client = client();

    // Guarantee 2 (the controlled 404, with live-reload, not a connection
    // error or eager content):
    let root_url = format!("{base}/");
    let request_start = Instant::now();
    let mut observed: Option<(u16, String)> = None;
    while request_start.elapsed() < FIRST_RESPONSE_DEADLINE {
        if let Ok(resp) = client.get(&root_url).send().await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            observed = Some((status, body));
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = observed.unwrap_or_else(|| {
        panic!(
            "the dev server did not answer GET / within {}s of the banner while the cold \
             deferred-bundle slow-step was in flight — the serve path is blocking on the \
             deferred bundle.\n{}",
            FIRST_RESPONSE_DEADLINE.as_secs(),
            session.logs(),
        )
    });
    assert_eq!(
        status,
        404,
        "GET / during the held cold-lazy bundle window must serve the controlled dev 404 \
         (no eager fallback, no wrong body, no seed to fall back on); got status {status}, \
         body:\n{body}\n{}",
        session.logs(),
    );
    assert!(
        body.contains("404"),
        "GET / during the held window returned status 404 but a body that doesn't look \
         like the controlled dev 404 page; body:\n{body}\n{}",
        session.logs(),
    );
    assert!(
        body.contains("<script src=\"/__zfb/livereload.js\"></script>"),
        "the cold dev-404 body must carry the live-reload script — that's what lets an \
         open tab self-heal via the post-publish `pages_stale` broadcast once the bundle \
         lands; got body:\n{body}\n{}",
        session.logs(),
    );

    // The deferred bundle slow step is still sleeping; Drop group-kills it.
}

// ---------------------------------------------------------------------------
// Test 7 — cold-lazy deferred publish: SSE self-heal + fresh 200 (issue
// #1806/#1808).
// ---------------------------------------------------------------------------

/// A Cold session, no `dist/` seed: proves the deferred publish's
/// post-publish stale-mark actually reaches the browser as a REAL SSE `page`
/// event on the SSE live-reload stream — not just "GET eventually returns 200", which
/// alone would only prove render-on-request, not the broadcast that makes
/// an already-open tab (still showing the dev 404 body from Test 6's
/// window) self-heal.
///
/// ## Race-free SSE subscribe (codex-review finding, issue #1811)
///
/// `ZFB_DEV_TEST_SLOW_BUNDLE_MS=SSE_SUBSCRIBE_HOLD_MS` holds the deferred
/// bundle open just long enough for the test process to read the banner
/// and complete the SSE HTTP subscribe handshake. Without this,
/// this fixture is small enough that the bundle could publish and broadcast
/// the ONE `page` event before the subscription registers — the broadcast
/// channel has no replay, so a missed event means the test waits out the
/// full `RENDER_DEADLINE` and fails despite correct server behavior. The
/// hold is a harness-only device (proportionally tiny next to
/// `RENDER_DEADLINE`); it does not change what's being asserted — the
/// publish still runs to completion and broadcasts on its own, we just
/// guarantee we're listening first.
///
/// After the event, `GET /` must serve 200 with the fixture's real homepage
/// content — a 200 from nothing: Cold has no `dist/` seed, so the only
/// possible source for that 200 is the request-time render-on-request hook
/// rendering the route from SOURCE.
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_deferred_publish_broadcasts_sse_and_serves_fresh_200() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let hold = SSE_SUBSCRIBE_HOLD_MS.to_string();
    let mut session = spawn_dev(
        &tmp,
        &esbuild,
        &[
            ("ZFB_DEV_TEST_SLOW_BUNDLE_MS", hold.as_str()),
            ("ZFB_DEV_BOOT_LAZY", "cold"),
        ],
    );

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    let base = format!("http://localhost:{port}");

    // Subscribe to SSE immediately after the banner — inside the held
    // window, so the subscription is guaranteed to be registered before
    // the deferred bundle publishes (see doc comment above).
    let sse = subscribe_sse(&base).await;

    // The authoritative proof (issue #1811 requires this be HARD-asserted,
    // not best-effort like `dev_dep_invalidation_1284_e2e.rs`'s watcher-tick
    // pattern): an actual SSE `page` event.
    let event = next_sse_event_name(sse, RENDER_DEADLINE)
        .await
        .expect("read SSE stream after the cold-lazy deferred publish");
    assert_eq!(
        event.as_deref(),
        Some("page"),
        "expected a `page` SSE event once the cold-lazy deferred bundle publishes and \
         marks every route stale (issue #1808); got {event:?} within {}s.\n{}",
        RENDER_DEADLINE.as_secs(),
        session.logs(),
    );

    // GET / now serves 200 with freshly rendered content. By the time the
    // SSE event is observed, the publish + stale-mark are already
    // committed (the broadcast fires only after the tick's state mutation
    // lands), so the FIRST response is asserted directly rather than
    // polling past a non-200 status (codex-review finding, issue #1811) —
    // that would mask an ordering regression instead of catching it.
    let client = client();
    let root_url = format!("{base}/");
    let request_start = Instant::now();
    let mut observed: Option<(u16, String)> = None;
    // Bounded by RENDER_DEADLINE (not the tighter FIRST_RESPONSE_DEADLINE):
    // the render-on-request hook renders synchronously WITHIN this request,
    // so it needs the same generous V8+esbuild budget as any other
    // first-render assertion in this file. The loop only retries on a
    // request-level failure (connection refused / client timeout) — the
    // first ACTUAL response, whatever its status, is captured and
    // asserted below, never silently retried past.
    while request_start.elapsed() < RENDER_DEADLINE {
        if let Ok(resp) = client.get(&root_url).send().await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            observed = Some((status, body));
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = observed.unwrap_or_else(|| {
        panic!(
            "GET / did not answer within {}s of the cold-lazy deferred publish's `page` \
             SSE event.\n{}",
            RENDER_DEADLINE.as_secs(),
            session.logs(),
        )
    });
    assert_eq!(
        status,
        200,
        "GET / (the FIRST request after the `page` SSE event) must serve 200 — a 200 \
         from nothing, since Cold has no dist/ seed; got status {status}, body:\n{body}\n{}",
        session.logs(),
    );
    assert!(
        body.contains("<h1>dev-loop-basic</h1>"),
        "GET / after the cold-lazy deferred publish must serve the freshly-rendered \
         fixture homepage; got body:\n{body}\n{}",
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Test 8 — cold-bootstrap recovery after a broken deferred bundle (issue
// #1809).
// ---------------------------------------------------------------------------

/// Live e2e proof of the #1809 cold-bootstrap recovery mechanism: when
/// Cold's FIRST deferred bundle attempt fails, later publishes must still
/// reach the browser rather than 404ing forever (the failed attempt
/// consumes the boot-render-pending flag against the empty scaffold route
/// tables — see `arm_cold_bootstrap_recovery_decision` /
/// `recover_cold_bootstrap_after_publish` in `commands/dev.rs`).
///
/// ## Deterministic corruption — broken BEFORE the process even starts
///
/// `pages/index.tsx` is overwritten with deliberately invalid syntax in the
/// PREPARED ROOT, before `zfb dev` is ever spawned (`prepare_dev_root` +
/// `spawn_dev_in_root`, rather than the single `spawn_dev` helper other
/// tests use). This is deliberately NOT a held-window-then-corrupt design
/// (an earlier draft used `ZFB_DEV_TEST_SLOW_BUNDLE_MS` to buy a window
/// after the banner): that design is a wall-clock race — if the test
/// process were descheduled long enough on a loaded host, the deferred
/// bundle could read the ORIGINAL valid source before the corruption write
/// landed, and the expected failure would never occur (codex-review
/// finding, issue #1811). Corrupting the file before the process starts at
/// all removes the race entirely: whenever the deferred bundle eventually
/// runs, the broken source is already the only thing on disk.
///
/// ## Sequence
///
/// 1. Prepare the root, corrupt `pages/index.tsx`, THEN spawn Cold.
/// 2. Assert the FIRST deferred bundle fails: the error-level cold-lazy
///    failure message (`deferred_bundle_failure_message`'s Cold branch)
///    appears in the logs, `/__zfb/ready` reports documents pending, and
///    `GET /` serves the controlled 404 (no route table was ever published).
/// 3. Subscribe to SSE (before the fix, so the recovery event can't be
///    missed), then restore the ORIGINAL `pages/index.tsx` source — an
///    ordinary watcher-tick edit, exercising the SAME
///    `refresh_bundle_and_routes` success path any other content change
///    does.
/// 4. Assert recovery: a hard-asserted `page` SSE event, the "cold-lazy
///    bootstrap recovered" info line (the mechanism's own explicit signal —
///    proves THIS is what fired, not just any successful publish), readiness
///    advancing to a non-pending generation, and a first-request 200 with the
///    real homepage content.
///
/// ## Falsifiability
///
/// If `arm_cold_bootstrap_recovery_decision` were reverted (Cold's bundle
/// failure never arms the recovery latch), the fix's successful publish
/// would find `take_cold_bootstrap_pending()` false and skip the mark-stale
/// + broadcast entirely — no `page` SSE event ever fires, and the hard SSE
/// assertion below times out and fails. Observed directly (see the
/// PR/report).
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_broken_bundle_recovers_after_source_fix() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_bind_before_walk_e2e] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");

    // Step 1 — corrupt pages/index.tsx in the PREPARED ROOT, before `zfb
    // dev` is spawned at all (see the fn doc comment on why this beats a
    // held-window-then-corrupt design). Capture the original bytes first
    // so the fix step restores byte-identical valid source without
    // hardcoding the fixture's content.
    let root = prepare_dev_root(&tmp);
    let index_path = root.join("pages").join("index.tsx");
    let original_index =
        fs::read_to_string(&index_path).expect("read original pages/index.tsx before breaking it");
    fs::write(&index_path, "export default function HomePage( {\n")
        .expect("write deliberately-broken pages/index.tsx");

    let mut session = spawn_dev_in_root(&root, &esbuild, &[("ZFB_DEV_BOOT_LAZY", "cold")]);

    let Some(port) = wait_for_banner_port(&mut session).await else {
        return; // known-skip
    };
    let base = format!("http://localhost:{port}");
    let client = client();
    let root_url = format!("{base}/");
    let ready_url = format!("{base}/__zfb/ready");

    // Step 2 — the FIRST deferred bundle must fail LOUDLY. Wait for the
    // error-level cold-lazy failure message first: it is the authoritative
    // signal that the bundle attempt actually ran and failed, unlike "GET /
    // is 404" — which is ALSO (trivially) true before the bundle even
    // attempts, since routes start as the empty scaffold.
    let error_deadline = Instant::now();
    let mut saw_error_message = false;
    while error_deadline.elapsed() < RENDER_DEADLINE {
        if session
            .logs()
            .contains("cold-lazy (ZFB_DEV_BOOT_LAZY=cold) has no prebuilt dist/ seed")
        {
            saw_error_message = true;
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        saw_error_message,
        "the error-level cold-lazy deferred-bundle-failure message never appeared within \
         {}s.\n{}",
        RENDER_DEADLINE.as_secs(),
        session.logs(),
    );

    // Confirm the route table was never published: GET / still serves the
    // controlled 404 (never a wrong/empty/partial body).
    let confirm_start = Instant::now();
    let mut confirmed_404: Option<(u16, String)> = None;
    while confirm_start.elapsed() < FIRST_RESPONSE_DEADLINE {
        if let Ok(resp) = client.get(&root_url).send().await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            confirmed_404 = Some((status, body));
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = confirmed_404.unwrap_or_else(|| {
        panic!(
            "GET / after the broken deferred bundle failed to answer.\n{}",
            session.logs()
        )
    });
    assert_eq!(
        status,
        404,
        "GET / after the broken deferred bundle must still serve the controlled dev 404 \
         (cold-lazy has no dist/ seed to fall back on); got status {status}, body:\n{body}\n{}",
        session.logs(),
    );
    assert!(
        body.contains("404"),
        "GET / after the broken deferred bundle returned status 404 but a body that \
         doesn't look like the controlled dev 404 page; body:\n{body}\n{}",
        session.logs(),
    );
    let pending = ready_json(&client, &ready_url).await;
    assert_eq!(
        pending["ready"], false,
        "a failed deferred Cold bundle must keep framework publication pending"
    );
    assert_eq!(pending["documents"], "pending");
    let pending_generation = ready_generation(&pending);

    // Step 3 — subscribe to SSE BEFORE the fix (broadcast, not a queue: an
    // event fired before subscription is gone forever).
    let sse = subscribe_sse(&base).await;

    fs::write(&index_path, &original_index).expect("restore original pages/index.tsx");

    // Step 4 — assert recovery. A hard-asserted `page` SSE event (see Test
    // 7's doc comment on why this must be hard, not best-effort).
    let event = next_sse_event_name(sse, RENDER_DEADLINE)
        .await
        .expect("read SSE stream after fixing pages/index.tsx");
    assert_eq!(
        event.as_deref(),
        Some("page"),
        "expected a `page` SSE event once the fixed source republishes and the \
         cold-bootstrap recovery latch fires (issue #1809); got {event:?} within {}s.\n{}",
        RENDER_DEADLINE.as_secs(),
        session.logs(),
    );

    // The mechanism's own explicit signal — proves the RECOVERY path fired
    // (not merely "some publish succeeded").
    assert!(
        session.logs().contains("cold-lazy bootstrap recovered"),
        "expected the \"dev: cold-lazy bootstrap recovered\" info line after the fixed \
         source republished (issue #1809's recovery latch).\n{}",
        session.logs(),
    );
    let recovered_readiness = wait_for_publication_ready(
        &client,
        &ready_url,
        pending_generation,
        RENDER_DEADLINE,
        &session,
    )
    .await;
    assert_ne!(
        recovered_readiness["documents"], "pending",
        "the successful Cold route-table recovery must commit a complete document boundary"
    );

    // First request after recovery: 200 with the real homepage content.
    // Bounded by RENDER_DEADLINE (the render-on-request hook renders
    // synchronously WITHIN this request), but the loop only retries on a
    // request-level failure — the first ACTUAL response, whatever its
    // status, is captured and asserted below rather than silently retried
    // past a non-200 (codex-review finding, issue #1811): if recovery
    // broadcast before routes were actually request-renderable, polling
    // through an initial 404/500 would mask exactly the ordering
    // regression the "first-request 200" guarantee exists to catch.
    let start = Instant::now();
    let mut observed: Option<(u16, String)> = None;
    while start.elapsed() < RENDER_DEADLINE {
        if let Ok(resp) = client.get(&root_url).send().await {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            observed = Some((status, body));
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = observed.unwrap_or_else(|| {
        panic!(
            "GET / did not answer within {}s of the cold-bootstrap recovery's `page` SSE \
             event.\n{}",
            RENDER_DEADLINE.as_secs(),
            session.logs(),
        )
    });
    assert_eq!(
        status,
        200,
        "GET / (the FIRST request after cold-bootstrap recovery) must serve 200; got \
         status {status}, body:\n{body}\n{}",
        session.logs(),
    );
    assert!(
        body.contains("<h1>dev-loop-basic</h1>"),
        "GET / after cold-bootstrap recovery must serve the freshly-rendered fixture \
         homepage; got body:\n{body}\n{}",
        session.logs(),
    );
}
