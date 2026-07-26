//! RED test for issue #1825 (Dev Self Heal epic #1999, Wave 1 sub-issue
//! #2001).
//!
//! ## The bug
//!
//! `crates/zfb-server/src/routes.rs:1126` (`serve_from_waterfall`) resolves
//! a `_redirects` `200` rewrite's target through the on-disk waterfall
//! (`PageCache -> html_root -> public_root -> dist_root -> 404`)
//! **without** re-running the render-on-request hook. That split is
//! deliberate and correct (issue #1546): it matches the `_redirects`
//! "resolve once, no chaining" contract and mirrors Cloudflare Workers'
//! real Static Assets layer, which never hands a rewritten request back to
//! the Worker either.
//!
//! It was harmless before Cold existed, because `ZFB_DEV_BOOT_LAZY=1` only
//! took the boot-lazy path when a servable prebuilt `dist/` seed existed —
//! so a rewrite target's HTML was already on disk (stale but real).
//! Seedless `ZFB_DEV_BOOT_LAZY=cold` (issue #1808) removes that
//! requirement: at a fresh Cold boot, no waterfall leg has real content
//! for ANY route yet — pages render lazily, on their own first request,
//! via the render-on-request hook. But the hook is bypassed *by design*
//! for a rewrite target (see the doc comment above), so nothing ever
//! makes `/target`'s HTML exist unless `/target` is requested directly or
//! an unrelated full re-render happens to write it. A request to
//! `/alias` therefore 404s **indefinitely** — not "until the hook claims
//! it" (the hook never runs for this path at all).
//!
//! ## Fixture (`tests/fixtures/cold-rewrite-red/`)
//!
//! - `pages/index.tsx` — an ordinary home route, repeatedly edited by this
//!   test to produce genuine, SSE-confirmed watcher ticks. Never the
//!   rewrite target.
//! - `pages/target.tsx` — the `_redirects` rewrite target. This test's
//!   harness never requests it directly, by construction.
//! - `public/_redirects` — one rule: `/alias /target 200`.
//!
//! ## What "indefinitely" means here, and how this test proves it
//!
//! A single 404 response is consistent with "not yet rendered" (a false
//! positive for this bug — cold-lazy is *expected* to 404 a route that
//! genuinely has never been requested, until its own first request). The
//! defect is that `/alias` NEVER resolves no matter how many *unrelated*
//! things happen in the meantime, because the one mechanism that could
//! fix it (the render-on-request hook) is architecturally never invoked
//! for this path. So this test asserts the DESIRED (post-fix) outcome —
//! `GET /alias` eventually resolves the rewrite target — via a loop that
//! polls repeatedly and interleaves each poll with a genuine,
//! SSE-confirmed watcher tick (an edit to `pages/index.tsx`, unrelated to
//! `/target`), proving ticks are actually happening in the system, not
//! just that time has passed. Today every poll in that loop still 404s,
//! so the final assertion fails with a panic message enumerating every
//! poll's observation — the "quote the failure text" proof that this is
//! the persistent-404 bug, not a flake. `/target` is never requested
//! directly at any point. Event/condition-keyed waits only (each tick is
//! confirmed via its own SSE `page` event); no fixed sleeps gate any
//! assertion.
//!
//! ## `#[ignore]`
//!
//! Tagged `heavy` per this repo's `#[ignore]` manifest convention
//! (`CLAUDE.md`): a real `zfb dev --port 0` E2E, and RED (fails today by
//! design). Fixing #1825 is Waves 3-4 of epic #1999 (rewrite-target
//! enumeration + marking stale); this test only proves the gap exists
//! today. It stays ignored until that fix lands, at which point it
//! should flip green with its `#[ignore]` dropped, the way sibling red
//! tests in this repo have been (see e.g.
//! `bundler_root_workspace_stage_escape_audit_armed_regression.rs`).

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

const OVERALL_DEADLINE: Duration = Duration::from_secs(120);
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const SSE_DEADLINE: Duration = Duration::from_secs(30);
const GRACEFUL_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Number of confirmed, unrelated watcher ticks interleaved with the
/// `/alias` polls after the initial check. Each tick is its own SSE
/// `page` event, not a fixed time delay — see the module doc comment.
const UNRELATED_TICKS: u32 = 3;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cold-rewrite-red")
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
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
        unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

struct DevSession {
    root: PathBuf,
    guard: DevServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            read_log(&self.stdout_path),
            read_log(&self.stderr_path),
        )
    }

    #[allow(dead_code)]
    async fn stop_gracefully(&mut self) {
        unsafe { libc::kill(-self.guard.pgid, libc::SIGINT) };

        let start = Instant::now();
        loop {
            if let Some(status) = self.guard.try_exit_status() {
                assert!(
                    status.success(),
                    "`zfb dev` exited unsuccessfully after SIGINT ({status:?}).\n{}",
                    self.logs(),
                );
                return;
            }
            if start.elapsed() >= GRACEFUL_SHUTDOWN_DEADLINE {
                unsafe { libc::kill(-self.guard.pgid, libc::SIGKILL) };
                let _ = self.guard.child.wait();
                panic!(
                    "`zfb dev` did not exit within {}s after SIGINT.\n{}",
                    GRACEFUL_SHUTDOWN_DEADLINE.as_secs(),
                    self.logs(),
                );
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(idx) = rest.find("http://") {
        let candidate = &rest[idx + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or("");
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

fn spawn_dev_cold(root: PathBuf, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log");

    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        // Request-time lazy render (the default when `ZFB_DEV_EAGER` is
        // unset) is required for boot-lazy to activate at all — see
        // `boot_lazy_decision` in `crates/zfb/src/commands/dev.rs`.
        .env_remove("ZFB_DEV_EAGER")
        .env("ZFB_DEV_BOOT_LAZY", "cold")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.process_group(0);

    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        root,
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

fn build_reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client")
}

async fn wait_for_ready_port(session: &mut DevSession) -> Option<u16> {
    let boot_start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path),
            );
            if combined.contains("embed_v8")
                || combined.contains("no esbuild")
                || combined.contains("no tailwind")
                || combined.contains("tailwindcss") && combined.contains("not found")
            {
                eprintln!(
                    "[cold_rewrite_red_e2e] known unavailable dependency; skipping.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing a ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            return Some(port);
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn subscribe_sse(client: &reqwest::Client, base: &str) -> reqwest::Response {
    let response = client
        .get(format!("{base}/__zfb/reload"))
        .send()
        .await
        .expect("subscribe to /__zfb/reload");
    assert_eq!(
        response.status().as_u16(),
        200,
        "SSE endpoint must answer 200"
    );
    response
}

fn home_page_source(revision: u32) -> String {
    format!(
        "export default function HomePage() {{\n  return (\n    <html lang=\"en\">\n      <head>\n        <meta charSet=\"utf-8\" />\n        <title>cold-rewrite-red fixture</title>\n      </head>\n      <body>\n        <h1>COLD_REWRITE_RED_HOME_MARKER_V{revision}</h1>\n      </body>\n    </html>\n  );\n}}\n"
    )
}

/// Confirms the watcher is live by editing the UNRELATED home page and
/// waiting for the SSE `page` event it must produce. Used both for the
/// initial boot handshake and for every subsequent "unrelated tick"
/// interleaved between `/alias` polls — this is the test's sole
/// timekeeping mechanism, never a fixed sleep.
///
/// Codex review finding: a single immediate write can land in the
/// watcher's own startup dead window (SSE already accepting connections,
/// filesystem watcher not fully registered yet) and produce no event,
/// which would then fail this handshake on an unrelated timeout rather
/// than ever reaching the `/alias` assertion. Mirrors
/// `confirm_watcher_live` in `dev_content_aggregate_cold_boot_e2e.rs`:
/// keep writing on a short interval until an SSE event arrives or the
/// deadline elapses, rather than a single write-then-wait.
async fn confirm_unrelated_tick(
    session: &DevSession,
    base: &str,
    client: &reqwest::Client,
    revision_base: u32,
) {
    let sse = subscribe_sse(client, base).await;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let root = session.root.clone();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut revision = revision_base;
            while !stop.load(Ordering::SeqCst) {
                fs::write(root.join("pages/index.tsx"), home_page_source(revision))
                    .expect("edit unrelated home page to produce a genuine watcher tick");
                revision += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    };

    let event = next_sse_event_name(sse, SSE_DEADLINE).await;
    stop.store(true, Ordering::SeqCst);
    let _ = writer.await;

    let event = event.unwrap_or_else(|_| {
        panic!(
            "no SSE event observed within {}s of repeatedly editing the unrelated home page — \
             the watcher tick this test relies on as its timekeeping mechanism never happened.\n{}",
            SSE_DEADLINE.as_secs(),
            session.logs(),
        )
    });
    assert_eq!(
        event.as_deref(),
        Some("page"),
        "unrelated home-page edit did not produce the expected confirmed watcher tick.\n{}",
        session.logs(),
    );
}

/// A single `/alias` observation: status code and (on non-connection
/// error) response body, for failure-message purposes only.
struct AliasObservation {
    status: Option<u16>,
    body_snippet: String,
}

impl std::fmt::Display for AliasObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "status {:?}, body: {}", self.status, self.body_snippet)
    }
}

const TARGET_MARKER: &str = "COLD_REWRITE_RED_TARGET_MARKER";

async fn observe_alias(client: &reqwest::Client, base: &str) -> AliasObservation {
    match client.get(format!("{base}/alias")).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            AliasObservation {
                status: Some(status),
                body_snippet: body.chars().take(200).collect(),
            }
        }
        Err(error) => AliasObservation {
            status: None,
            body_snippet: format!("request error: {error}"),
        },
    }
}

fn alias_resolved(observation: &AliasObservation) -> bool {
    observation.status == Some(200) && observation.body_snippet.contains(TARGET_MARKER)
}

/// Proves issue #1825: at a fresh Cold boot, a `_redirects` 200-rewrite
/// target with no on-disk content anywhere in the waterfall 404s
/// indefinitely — not merely "not yet", but genuinely forever, since the
/// one mechanism (the render-on-request hook) that could make it fresh is
/// architecturally bypassed for this dispatch path. `/target` is never
/// requested directly at any point in this test; every intervening
/// watcher tick is unrelated (an edit to the unrelated home page) and
/// independently SSE-confirmed.
#[ignore = "heavy: run with --ignored — real `zfb dev` E2E proving issue #1825 (RED — remove/invert once Waves 3-4 of epic #1999 land the fix)"]
#[tokio::test(flavor = "multi_thread")]
async fn cold_redirects_rewrite_target_404s_indefinitely() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[cold_rewrite_red_e2e] no esbuild binary available; skipping. Set ZFB_ESBUILD_BIN \
             or install esbuild on PATH."
        );
        return;
    };

    let temp = tempfile::tempdir().expect("create tempdir for cold-rewrite-red fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy cold-rewrite-red fixture");
    assert!(
        !root.join(".zfb-build").exists(),
        "fixture must have no persisted dev graph or prior render output before cold boot"
    );

    let mut session = spawn_dev_cold(root, &esbuild);
    let pgid = session.guard.pgid;

    let body = async {
        let Some(port) = wait_for_ready_port(&mut session).await else {
            return;
        };
        let base = format!("http://localhost:{port}");
        let client = build_reqwest_client();

        // Boot readiness via the home route — NOT `/target`, so the
        // rewrite target is never requested directly by the handshake.
        let ready_start = Instant::now();
        loop {
            if matches!(
                client.get(format!("{base}/")).send().await,
                Ok(response) if response.status().as_u16() == 200
            ) {
                break;
            }
            assert!(
                ready_start.elapsed() < BOOT_DEADLINE,
                "GET / never answered 200 within {}s after the ready banner.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // Establish the watcher is live before the first assertion, using
        // the same unrelated-tick mechanism the rest of the test relies on.
        confirm_unrelated_tick(&session, &base, &client, 1).await;

        // This test asserts the DESIRED (post-fix) behavior: `/alias`
        // must eventually resolve the rewrite target. It polls
        // repeatedly, interleaving each poll with a genuine,
        // SSE-confirmed unrelated watcher tick (an edit to `pages/index.tsx`,
        // never `/target`) — proof that ticks are demonstrably happening
        // in the system, not just that time has passed. Today, issue
        // #1825 means this loop exhausts every poll and the final assert
        // below fails, with the panic message enumerating every 404
        // observation. `/target` itself is NEVER requested directly here:
        // doing so would render it via the render-on-request hook (a
        // legitimate, different dispatch path — see `serve_page` in
        // routes.rs) and prove nothing about the indefiniteness this test
        // exists to demonstrate.
        let mut observations: Vec<(u32, AliasObservation)> = Vec::new();

        let initial = observe_alias(&client, &base).await;
        let mut resolved = alias_resolved(&initial);
        observations.push((1, initial));

        for tick in 0..UNRELATED_TICKS {
            if resolved {
                break;
            }
            confirm_unrelated_tick(&session, &base, &client, tick + 2).await;
            let observation = observe_alias(&client, &base).await;
            resolved = alias_resolved(&observation);
            observations.push((tick + 2, observation));
        }

        if !resolved {
            let history = observations
                .iter()
                .map(|(poll, obs)| format!("  poll {poll}: {obs}"))
                .collect::<Vec<_>>()
                .join("\n");
            panic!(
                "issue #1825: GET /alias never resolved the `_redirects` 200-rewrite target \
                 across {} polls, despite {} confirmed unrelated watcher tick(s) occurring in \
                 between (each an SSE-confirmed edit to the unrelated pages/index.tsx). The \
                 target 404s indefinitely at a fresh Cold boot because `serve_from_waterfall` \
                 deliberately bypasses the render-on-request hook for a rewrite target — see \
                 this file's module doc comment. `/target` was never requested directly.\n\
                 Poll history:\n{history}\n{}",
                observations.len(),
                UNRELATED_TICKS,
                session.logs(),
            );
        }
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(()) => {}
        Err(_) => panic!(
            "[watchdog] cold-rewrite-red E2E did not finish within {}s. Process group {pgid} \
             will be killed.",
            OVERALL_DEADLINE.as_secs(),
        ),
    }
}
