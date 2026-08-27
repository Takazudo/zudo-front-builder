//! Poll-backend dev-server E2E leg (issue #2175, epic #2172 — Watch Poll
//! Fallback). Confirms the `watchPollFallback`/`watchPollIntervalMs` config
//! plumbed end to end by #2173 (the `zfb-watcher` poll backend itself) and
//! #2174 (threading `Config::watch_poll_fallback`/`watch_poll_interval_ms`
//! into every watch-construction call site) actually drives a real `zfb dev`
//! session the way a project operator flipping the flag would expect.
//!
//! ## What this proves
//!
//! Everything upstream of this test already has unit/integration coverage
//! at its own level: `zfb_watcher::WatchBackend::Poll` itself
//! (`zfb-watcher/tests/real_fs_behavior.rs`), `Config::watch_poll_fallback`/
//! `watch_poll_interval_ms` parsing and validation (`crates/zfb/src/config.rs`),
//! and `commands/dev.rs`'s `watch_backend_from_config` /
//! `liveness_opts_for_backend` unit tests. Nothing until now spins up a REAL
//! `zfb dev` process with `watchPollFallback: true` set in `zfb.config.json`
//! and proves the two behaviors a project operator actually cares about when
//! they opt into the poll fallback:
//!
//! - **(a) in-place content edit** — rewriting an EXISTING collection
//!   entry's body is observed by the poll backend and reaches the served
//!   HTML, with zero dev-server restart;
//! - **(b) new-entry discovery** — a collection entry CREATED mid-session is
//!   discovered by the orchestrator, not merely "a file that happens to
//!   exist on disk". Proven by BOTH the new entry's own dynamic route
//!   serving its body AND a separate aggregate page (`pages/index.tsx`,
//!   which lists every `getCollection("posts")` member independently of the
//!   dynamic route) enumerating its title. A generic static-file handler
//!   could in principle serve the new entry's own route without ever having
//!   registered it as a collection member; it could not make an UNRELATED
//!   page's collection listing grow a new item. That requires the watcher
//!   event to have reached the orchestrator's discovery hook and the
//!   planner's unknown-content-path fallback to have re-rendered the index.
//!
//! ## Poll-backend mtime granularity (load-bearing)
//!
//! notify's `PollWatcher` stores mtimes truncated to WHOLE SECONDS and, with
//! `compare_contents` at its default `false`, has no content/size fallback —
//! an overwrite landing in the same wall-clock second as the watcher's
//! baseline scan is invisible to it (see `zfb-watcher/src/lib.rs`'s module
//! doc, "Backend selection & poll-backend parity limitations", and the
//! identical caveat pinned at the crate level by
//! `zfb-watcher/tests/real_fs_behavior.rs::modify_of_preexisting_file_after_handshake_with`).
//! This fixture's pre-existing content files are backdated by several
//! seconds BEFORE `zfb dev` is spawned, so scenario (a)'s in-place edit
//! (written at real wall-clock "now") is deterministically newer than the
//! watcher's very first baseline scan — regardless of how quickly boot
//! happens to complete, never relying on boot simply taking "long enough".
//! Scenario (b)'s brand-new file has no such caveat: the poll backend
//! detects creation by directory-listing presence, not by mtime comparison.
//!
//! ## Harness
//!
//! Local copy of `dev_serve_e2e.rs`'s harness (`spawn_dev` /
//! `boot_and_handshake` / `poll_until_contains` / `subscribe_sse` /
//! `CrossBinaryE2eLock` / `SERIAL`), reusing the SAME `dev-loop-basic`
//! fixture `dev_serve_e2e.rs` and `dev_dep_invalidation_1284_e2e.rs` already
//! copy. Only `zfb.config.json` is rewritten in the COPY (adding
//! `watchPollFallback`/`watchPollIntervalMs`) — the checked-in fixture stays
//! untouched for every other test that copies it expecting the native
//! backend. Rust integration tests are separate binaries and cannot import
//! another test file's private items (the documented convention across every
//! `dev_*_e2e.rs` file in this crate), so this is a deliberate copy, not a
//! new pattern.
//!
//! Tagged `heavy` (see `crates/CLAUDE.md`'s `#[ignore]` taxonomy): a real
//! `zfb dev` E2E, too slow / too reliant on a free port for the T1 gate.
//! Runs weekly via `exam.yml`'s `quarantine-heavy` job and locally via
//! `cargo test -p zfb --test dev_poll_backend_e2e -- --ignored`.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use zfb_test_utils::{
    locate_esbuild, next_sse_event_name, open_sse, zfb_binary, CrossBinaryE2eLock,
};

/// Serialises this binary's single spawning test against itself (a no-op
/// today, kept for parity with every sibling e2e file) and, via
/// `CrossBinaryE2eLock`, against every OTHER e2e binary that boots a real
/// `zfb dev`/`zfb build` process (issue #1339) — see
/// `zfb-test-utils/src/cross_binary_lock.rs` for the lock-ordering rationale.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Overall wall-clock watchdog for the whole test (boot + both scenarios).
const OVERALL_DEADLINE: Duration = Duration::from_secs(240);

/// Deadline for the dev server to print its ready banner + first `GET /` 200.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-assertion deadline for a marker to appear after an edit. Comfortably
/// more than an order of magnitude above `WATCH_POLL_INTERVAL_MS`, mirroring
/// `commands/dev.rs`'s own `LIVENESS_POLL_DEADLINE_INTERVAL_MULTIPLIER`
/// margin for the poll backend.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);

/// Deadline for the watcher-live handshake + per-scenario SSE tick signal.
const SSE_DEADLINE: Duration = Duration::from_secs(30);

/// HTTP poll cadence for this test's own `poll_until_*` loops (unrelated to
/// the dev server's filesystem poll-backend cadence below).
const HTTP_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The dev server's poll watch-backend re-scan cadence for this test
/// (`watchPollIntervalMs` in the fixture's `zfb.config.json`) — short enough
/// to keep the test fast, comfortably inside `Config::watch_poll_interval_ms`'s
/// validated 50..=10_000ms range.
const WATCH_POLL_INTERVAL_MS: u64 = 250;

/// How far in the past to backdate pre-existing content files' mtimes before
/// `zfb dev` boots — see the module doc's "Poll-backend mtime granularity"
/// section. Comfortably larger than 1s to survive scheduling jitter.
const BACKDATE: Duration = Duration::from_secs(5);

fn base_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-loop-basic")
}

/// Recursive directory copy (same shape as every sibling `dev_*_e2e.rs`).
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

/// Overwrite the COPIED fixture's `zfb.config.json` to opt into the poll
/// watch backend. Deliberately not done in the checked-in fixture: every
/// other test copying `dev-loop-basic` (`dev_serve_e2e.rs`,
/// `dev_dep_invalidation_1284_e2e.rs`) exercises the native backend and must
/// stay unaffected.
fn enable_poll_backend(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        format!(
            r#"{{
  "framework": "preact",
  "collections": [
    {{
      "name": "posts",
      "path": "content/posts"
    }}
  ],
  "watchPollFallback": true,
  "watchPollIntervalMs": {WATCH_POLL_INTERVAL_MS}
}}
"#
        ),
    )
    .expect("write zfb.config.json with watchPollFallback enabled");
}

/// Backdate `path`'s mtime by [`BACKDATE`] — see the module doc's
/// "Poll-backend mtime granularity" section.
fn backdate(path: &Path) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap_or_else(|e| panic!("open {} for backdate: {e}", path.display()))
        .set_modified(SystemTime::now() - BACKDATE)
        .unwrap_or_else(|e| panic!("backdate {}: {e}", path.display()));
}

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
        // Best-effort: ESRCH (already gone) is harmless.
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

/// Extract the port from the dev ready banner (`→ ready on http://localhost:PORT/`).
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

/// Spawn `zfb dev --port 0` over the already-prepared fixture root, in its
/// own process group, with stdout/stderr captured to files (never pipes — a
/// piped child outgrowing the ~64KB OS pipe buffer blocks on write and
/// masquerades as a hang, `build_terminates.rs`'s finding). Callers copy and
/// extend the fixture (and call `enable_poll_backend`/`backdate`) BEFORE
/// calling this.
fn spawn_dev(root: PathBuf, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
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
    // Strip any inherited lazy-render switches so a shell/CI environment
    // can't silently flip this session's render mode (codex review finding
    // in dev_serve_e2e.rs, #1027).
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
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

/// Poll `url` until the body contains `needle` with status 200.
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
        tokio::time::sleep(HTTP_POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve a body containing {needle:?} within {}s.\n\
         Last observation: {last_observation}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

/// Subscribe to the dev server's SSE live-reload endpoint. Must be called
/// BEFORE the edit whose tick it is meant to observe.
async fn subscribe_sse(base: &str) -> reqwest::Response {
    let resp = open_sse(base).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "SSE live-reload endpoint must answer 200"
    );
    resp
}

/// Drain any in-flight watcher ticks until the SSE stream stays quiet for
/// `quiet_gap` (or `cap` elapses) — same rationale as
/// `dev_dep_invalidation_1284_e2e.rs`'s helper of the same name: the
/// watcher-live handshake's trailing warmup write can leave a tick in flight
/// after `boot_and_handshake` returns, which would otherwise race the next
/// real edit's own tick.
async fn drain_ticks_until_quiescent(base: &str, quiet_gap: Duration, cap: Duration) {
    let start = Instant::now();
    while start.elapsed() < cap {
        let sse = subscribe_sse(base).await;
        match next_sse_event_name(sse, quiet_gap).await {
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
}

enum ScenarioOutcome {
    Completed,
    /// The dev binary exited with a known environmental skip indicator (no
    /// embedded V8 / no esbuild) — skip without failing.
    Skipped,
}

/// Phases A-C: ready-banner port discovery, HTTP readiness, and the
/// watcher-live handshake. Returns `None` when the binary exited with a
/// recognized environmental skip indicator, otherwise `(base_url, client)`.
///
/// The handshake's warmup writes are fresh-named CREATEs each iteration —
/// the poll backend detects those by directory-listing presence, so this
/// phase needs no mtime backdating to work under the poll backend.
async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
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
                    "[poll_backend_e2e] `zfb dev` exited with a known-skip indicator \
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
            break port;
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(HTTP_POLL_INTERVAL).await;
    };
    let base = format!("http://localhost:{port}");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client");

    // Phase B: HTTP readiness.
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
            tokio::time::sleep(HTTP_POLL_INTERVAL).await;
        }
    }

    // Phase C: watcher-live handshake — subscribe to SSE FIRST, then write
    // fresh-named warmup content files until the first SSE event arrives.
    {
        let sse = subscribe_sse(&base).await;
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
                    // Longer than WATCH_POLL_INTERVAL_MS so each warmup write
                    // lands in its own re-scan under the poll backend rather
                    // than several piling into one.
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
             (the poll backend never observed a warmup CREATE).\n{}",
            SSE_DEADLINE.as_secs(),
            session.logs(),
        );
    }

    Some((base, client))
}

/// Falsifiability — (a): revert #2173's `WatchBackend::Poll` wiring (or
/// #2174's `watch_backend_from_config` threading) and this in-place edit is
/// never observed at all: the served `/posts/a/` body stays at its baseline
/// marker until `SCENARIO_DEADLINE` times out.
/// Falsifiability — (b): the discovery poll on `/posts/gamma/` and the
/// separate aggregate-index poll both time out the same way if the poll
/// backend never delivers the CREATE to the orchestrator's discovery hook.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: poll-backend e2e leg — runs in exam.yml quarantine-heavy"]
async fn e2e_poll_backend_content_edit_and_new_entry_discovery() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[poll_backend_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create tempdir for poll-backend fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&base_fixture_dir(), &root).expect("copy dev-loop-basic fixture");
    enable_poll_backend(&root);

    // Backdate the pre-existing content entries BEFORE `zfb dev` starts its
    // first poll-backend scan — see the module doc's "Poll-backend mtime
    // granularity" section. `b.md` is backdated too for consistency even
    // though this test never edits it.
    backdate(&root.join("content/posts/a.md"));
    backdate(&root.join("content/posts/b.md"));

    // ZFB_DEV_TIMING=1 (codex review finding, issue #2175): without this,
    // the native FSEvents/inotify backend observes the exact same
    // edit/create events the scenarios below assert on, so a revert of
    // `watch_backend_from_config`'s config-to-backend threading would leave
    // this test silently green even though the poll backend was never
    // actually selected. The timing line is the only way to prove WHICH
    // backend a session picked, not merely that something observed the FS.
    let mut session = spawn_dev(root, &esbuild, &[("ZFB_DEV_TIMING", "1")]);
    let pgid = session.guard.pgid;

    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };

        // ------------------------------------------------------------------
        // THE DISCRIMINATOR (codex review finding, issue #2175): assert the
        // dev server actually selected the POLL backend, not merely that it
        // observed the handshake's warmup writes — the native backend would
        // observe those identically, so without this check a revert of
        // `watch_backend_from_config`'s config-to-backend threading would
        // leave every scenario below silently green for the wrong reason.
        // ------------------------------------------------------------------
        {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path)
            );
            assert!(
                combined.contains(&format!(
                    "[zfb-timing] watch backend: poll interval={WATCH_POLL_INTERVAL_MS}ms"
                )),
                "expected the dev server to report selecting the POLL watch \
                 backend (watchPollFallback: true / watchPollIntervalMs: {WATCH_POLL_INTERVAL_MS} \
                 in zfb.config.json) via the ZFB_DEV_TIMING signal, but no such \
                 line was found — either the config-to-backend threading \
                 regressed, or the session fell back to the native backend.\n{}",
                session.logs(),
            );
        }

        // Baseline: the boot render served the fixture's original markers.
        poll_until_contains(
            &client,
            &format!("{base}/posts/a/"),
            "V1-MARKER-A",
            SCENARIO_DEADLINE,
            "baseline: GET /posts/a/",
            &session,
        )
        .await;

        // ------------------------------------------------------------------
        // Scenario (a) — in-place content edit reaches the served HTML.
        // ------------------------------------------------------------------
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;
        {
            let sse = subscribe_sse(&base).await;
            fs::write(
                session.root.join("content/posts/a.md"),
                "---\ntitle: Alpha\ndate: 2026-01-01\n---\n\n\
                 POLL-V2-MARKER-A body for the alpha post.\n",
            )
            .expect("edit a.md body");

            // Secondary signal (best-effort, matches the established idiom
            // across every dev_*_e2e.rs harness): an SSE `page` event should
            // fire. A timeout falls through to the authoritative HTTP poll
            // below rather than failing here.
            match next_sse_event_name(sse, SSE_DEADLINE).await {
                Ok(Some(name)) => assert_eq!(
                    name.as_str(),
                    "page",
                    "editing content/posts/a.md broadcast an unexpected SSE event \
                     (expected `page`).\n{}",
                    session.logs(),
                ),
                Ok(None) | Err(_) => eprintln!(
                    "[poll_backend_e2e scenario-a] no SSE `page` event observed \
                     within the window; relying on the authoritative HTTP poll."
                ),
            }

            poll_until_contains(
                &client,
                &format!("{base}/posts/a/"),
                "POLL-V2-MARKER-A",
                SCENARIO_DEADLINE,
                "scenario a: in-place content edit reaches the served HTML \
                 under the poll watch backend",
                &session,
            )
            .await;
        }

        // ------------------------------------------------------------------
        // Scenario (b) — a brand-new collection entry is DISCOVERED, not
        // merely servable: proven via its OWN dynamic route AND a SEPARATE
        // aggregate page's `getCollection` listing (module doc).
        // ------------------------------------------------------------------
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;
        {
            let sse = subscribe_sse(&base).await;
            fs::write(
                session.root.join("content/posts/gamma.md"),
                "---\ntitle: POLL-GAMMA-DISCOVERY-TITLE\ndate: 2026-01-03\n---\n\n\
                 POLL-GAMMA-BODY-MARKER for the new gamma post.\n",
            )
            .expect("create content/posts/gamma.md");

            match next_sse_event_name(sse, SSE_DEADLINE).await {
                Ok(Some(name)) => assert_eq!(
                    name.as_str(),
                    "page",
                    "creating content/posts/gamma.md broadcast an unexpected SSE \
                     event (expected `page`).\n{}",
                    session.logs(),
                ),
                Ok(None) | Err(_) => eprintln!(
                    "[poll_backend_e2e scenario-b] no SSE `page` event observed \
                     within the window; relying on the authoritative HTTP polls."
                ),
            }

            // The new entry's own dynamic route is servable.
            poll_until_contains(
                &client,
                &format!("{base}/posts/gamma/"),
                "POLL-GAMMA-BODY-MARKER",
                SCENARIO_DEADLINE,
                "scenario b: newly created entry's own route is discovered and \
                 served without a dev-server restart",
                &session,
            )
            .await;

            // The DISCOVERY proof: an UNRELATED aggregate page — `pages/index.tsx`,
            // whose `getStaticProps` calls `getCollection("posts")` independently
            // of the dynamic `[slug].tsx` route — must also enumerate the new
            // entry's title. Serving `/posts/gamma/` alone cannot distinguish
            // "the orchestrator discovered a new collection member" from "a
            // generic handler happened to find a file on disk"; a SEPARATE
            // page's collection listing growing a new item can only follow
            // from the watcher's CREATE event reaching the orchestrator's
            // discovery hook and the planner re-rendering the index.
            poll_until_contains(
                &client,
                &format!("{base}/"),
                "POLL-GAMMA-DISCOVERY-TITLE",
                SCENARIO_DEADLINE,
                "scenario b: the new entry is discovered by the aggregate index \
                 listing (orchestrator discovery, not merely static serving)",
                &session,
            )
            .await;
        }

        ScenarioOutcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] poll-backend e2e did not finish within {}s — hang, or \
             the poll watch backend never observed the edit/create. Process \
             group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}
