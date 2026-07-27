//! Repro harness + healthy-path baseline for issue #2063 / epic #2092
//! (sub-issue #2093, Wave 1): a `zfb dev` content edit is reported to
//! re-render server-side (fresh bytes on a subsequent request, `css`/
//! `islands` SSE events fire normally when applicable) but emit ZERO
//! `page` reload SSE events, so an already-open browser tab never
//! refreshes.
//!
//! The fixture (`tests/fixtures/dev-content-reload-2063/`) carries:
//! - an MDX entry (`content/posts/alpha.mdx`) behind a prerendered
//!   `paths()` route (`pages/posts/[slug].tsx`), and
//! - a zero-`paths()` sibling route (`pages/archive/[year].tsx`) — issue
//!   #2064 is already fixed on `main` (commit `6296428d`), so this must be
//!   a tolerated `page_sources` member that builds cleanly, not trigger
//!   the pre-fix "content provenance unavailable" boot warning.
//!
//! Modeled on `dev_content_aggregate_cold_boot_e2e.rs` (issue #1598): a
//! real `zfb dev --port 0` process, the `subscribe_sse` /
//! `zfb_test_utils::next_sse_event_name` / `CrossBinaryE2eLock` pattern
//! from that file, and SSE-confirmed edits — never a bare `sleep`.
//!
//! ## The exactly-one-`page`-event assertion (binding shape for this epic)
//!
//! An SSE subscriber connects BEFORE the content edit, reads events
//! through the completed tick, then holds a BOUNDED quiet window open
//! that:
//! - REJECTS a second `page` event arriving in that window, while
//! - STILL ALLOWS the expected `css`/`islands` events to arrive normally.
//!
//! `zfb_test_utils::next_sse_event_name` closes its connection after the
//! FIRST observed event, which is the right shape for a single-event
//! handshake but the wrong shape here: a single tick's `page`/`css`/
//! `islands` companions are pushed to the broadcast channel back-to-back,
//! and resubscribing between reads would silently drop whichever land in
//! the gap. `collect_tick_events` below keeps ONE subscription open
//! across the whole tick instead, reusing
//! `zfb_test_utils::decode_utf8_incremental` for the same incremental
//! UTF-8 decoding `next_sse_event_name` uses internally.
//!
//! This is exercised on BOTH the default boot mode and Cold boot
//! (`ZFB_DEV_BOOT_LAZY=cold`) — see the two `#[tokio::test]` functions at
//! the bottom of this file. On current `main` this baseline MAY PASS on
//! both boot modes; that is itself useful data for sub #2094's variant
//! hunt, so the observed SSE sequence is printed either way rather than
//! only asserted on.
//!
//! No permanent test registration (nextest `e2e-heavy` group, CLAUDE.md
//! manifest row, `#[ignore]` tag, exam.yml wiring) is added here — that
//! is Wave 5 / sub #2098's job (see epic #2092).

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use zfb_test_utils::{
    decode_utf8_incremental, locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock,
};

static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const OVERALL_DEADLINE: Duration = Duration::from_secs(180);
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Bounds the wait for the edit's tick to start producing SSE traffic at
/// all — generous, matching the sibling aggregate-cold-boot harness's own
/// `SSE_DEADLINE` this file is modeled on.
const SSE_FIRST_EVENT_DEADLINE: Duration = Duration::from_secs(30);

/// The bounded "quiet window" the epic's assertion shape calls for: once
/// the tick's first event lands, held open long enough that a genuine
/// same-tick `css`/`islands` companion (or an unwanted duplicate `page`)
/// would show up, short enough to keep both boot-mode runs fast.
const SSE_QUIET_WINDOW: Duration = Duration::from_secs(3);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-content-reload-2063")
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

/// `boot_lazy` sets `ZFB_DEV_BOOT_LAZY` explicitly (e.g. `Some("cold")`);
/// `None` removes the var entirely, i.e. the ordinary default boot mode.
fn spawn_dev(root: PathBuf, esbuild: &Path, boot_lazy: Option<&str>) -> DevSession {
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
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    if let Some(value) = boot_lazy {
        command.env("ZFB_DEV_BOOT_LAZY", value);
    }
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

async fn drain_ticks_until_quiescent(client: &reqwest::Client, base: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        let sse = subscribe_sse(client, base).await;
        match next_sse_event_name(sse, Duration::from_millis(1500)).await {
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
}

/// Read the SSE stream `resp` through the edit's completed tick, then hold
/// a bounded quiet window open on the SAME subscription, returning every
/// event name observed.
///
/// Deliberately does NOT resubscribe between reads (unlike
/// `next_sse_event_name` / `drain_ticks_until_quiescent` above, which only
/// ever need the FIRST event per subscription): a single tick's `page`/
/// `css`/`islands` companions can be pushed to the broadcast channel
/// back-to-back, and a fresh subscription would miss whichever land in
/// the resubscribe gap.
///
/// `first_event_deadline` bounds the wait for the tick to start producing
/// SSE traffic at all. `quiet_window` is the bounded quiet window applied
/// AFTER the first event: it (re)starts fresh every time a REAL event
/// line is parsed, so a same-tick `css`/`islands` companion extends the
/// read instead of being cut off, while an unwanted extra `page` event
/// still lands inside it.
///
/// Both windows are tracked as absolute `Instant` deadlines, advanced
/// only when a genuine `event:` line is parsed — never merely on chunk
/// arrival. `/__zfb/reload` is an axum `Sse` stream with a 15s
/// `KeepAlive` (`crates/zfb-server/src/livereload.rs`), which periodically
/// sends a `: `-comment chunk carrying no `event:` line at all. A
/// per-chunk-relative timeout (reset on every chunk, keepalives
/// included) would let that comment repeatedly re-arm the deadline —
/// turning the one outcome this baseline exists to be able to observe
/// (zero `page` events, ever) into a near-infinite hang instead of a
/// bounded, clearly-failing assertion.
///
/// Uses `reqwest::Response::chunk` (not `bytes_stream`) so this file needs
/// no `futures`/`futures-util` dependency of its own; reuses
/// `zfb_test_utils::decode_utf8_incremental` for the same incremental
/// UTF-8 handling `next_sse_event_name` relies on (a multibyte character
/// split across a chunk boundary must not corrupt or drop an already-
/// decoded `event:` line).
async fn collect_tick_events(
    mut resp: reqwest::Response,
    first_event_deadline: Duration,
    quiet_window: Duration,
) -> Vec<String> {
    let mut buf = Vec::<u8>::new();
    let mut decoded_up_to = 0usize;
    let mut pending_line = String::new();
    let mut events: Vec<String> = Vec::new();

    let mut phase_deadline = Instant::now() + first_event_deadline;

    loop {
        let remaining = phase_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let chunk = match tokio::time::timeout(remaining, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => bytes,
            Ok(Ok(None)) => break, // SSE connection closed.
            Ok(Err(_)) => break,   // Transport error — treat as end of stream.
            Err(_) => break,       // Deadline elapsed with no new real event.
        };
        buf.extend_from_slice(&chunk);

        let (decoded_str, new_decoded_up_to) = decode_utf8_incremental(&buf, decoded_up_to);
        decoded_up_to = new_decoded_up_to;

        let to_scan = if pending_line.is_empty() {
            decoded_str
        } else {
            let mut s = std::mem::take(&mut pending_line);
            s.push_str(&decoded_str);
            s
        };

        let mut lines = to_scan.split('\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_none() {
                // Last segment — may be an incomplete line (no trailing
                // newline yet). Save it for the next chunk.
                if !line.is_empty() {
                    pending_line = line.to_string();
                }
                break;
            }
            let trimmed = line.trim_end_matches('\r');
            if let Some(rest) = trimmed.strip_prefix("event:") {
                let name = rest.trim().to_string();
                if !name.is_empty() {
                    events.push(name);
                    // A genuine event was parsed — (re)arm the quiet
                    // window from NOW. Only a real `event:` line moves
                    // this deadline; a keepalive comment chunk never
                    // does (see the function doc comment above).
                    phase_deadline = Instant::now() + quiet_window;
                }
            }
        }
    }
    events
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
                    "[dev_content_reload_2063_e2e] known unavailable dependency; skipping.\n{}",
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

/// Repeated edits of the dedicated `__warmup.mdx` entry (never touched by
/// the test's own assertions below) prove the watch stream is live
/// without racing the fixture's `alpha` entry the scenario edits later.
async fn confirm_watcher_live(session: &DevSession, base: &str, client: &reqwest::Client) {
    let sse = subscribe_sse(client, base).await;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let warmup = session.root.join("content/posts/__warmup.mdx");
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut revision = 0u32;
            while !stop.load(Ordering::SeqCst) {
                fs::write(
                    &warmup,
                    format!(
                        "---\ntitle: Warmup\ndate: 2025-01-01\n---\n\nWarmup revision {revision}.\n"
                    ),
                )
                .expect("edit existing warmup content entry");
                revision += 1;
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        })
    };
    let first = next_sse_event_name(sse, SSE_FIRST_EVENT_DEADLINE)
        .await
        .expect("read SSE stream during watcher-live handshake");
    stop.store(true, Ordering::SeqCst);
    let _ = writer.await;
    assert!(
        first.is_some(),
        "watcher never became live: no edit-induced SSE event within {}s.\n{}",
        SSE_FIRST_EVENT_DEADLINE.as_secs(),
        session.logs(),
    );
}

fn build_reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client")
}

async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
    let port = wait_for_ready_port(session).await?;
    let base = format!("http://localhost:{port}");
    let client = build_reqwest_client();

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

    confirm_watcher_live(session, &base, &client).await;

    Some((base, client))
}

async fn poll_until_response_contains(
    client: &reqwest::Client,
    url: &str,
    marker: &str,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last_observation = String::from("(no response yet)");
    while start.elapsed() < SCENARIO_DEADLINE {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 200 && body.contains(marker) {
                    return;
                }
                last_observation = format!("status {status}, body:\n{body}");
            }
            Err(error) => last_observation = format!("request error: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve {marker:?} within {}s. Last observation:\n\
         {last_observation}\n{}",
        SCENARIO_DEADLINE.as_secs(),
        session.logs(),
    );
}

enum ScenarioOutcome {
    Completed,
    /// The binary exited with a known environmental skip indicator (no
    /// V8 / no esbuild / no Tailwind) — skip without failing.
    Skipped,
}

/// Shared scenario body run under both boot modes. `boot_lazy` selects the
/// mode (`None` = default, `Some("cold")` = Cold boot); `label` tags every
/// assertion/log line so a failure names which boot mode it came from.
async fn run_scenario(boot_lazy: Option<&str>, label: &str) {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_content_reload_2063_e2e] [{label}] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let temp = tempfile::tempdir().expect("create tempdir for dev-content-reload fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy dev-content-reload fixture");

    let mut session = spawn_dev(root, &esbuild, boot_lazy);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };

        // Confirm the entry's initial content is served before editing it.
        poll_until_response_contains(
            &client,
            &format!("{base}/posts/alpha"),
            "V1-BODY-ALPHA",
            &format!("[{label}] boot entry route"),
            &session,
        )
        .await;

        // The boot render / watcher-live handshake can leave a trailing
        // in-flight tick. Settle before subscribing for the assertion
        // below — otherwise a late handshake `page` event could be
        // misattributed as the edit's own SSE traffic.
        drain_ticks_until_quiescent(&client, &base).await;

        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("content/posts/alpha.mdx"),
            "---\ntitle: Alpha V2 Frontmatter\ndate: 2026-01-02\n---\n\nV2-BODY-ALPHA updated markdown body.\n",
        )
        .expect("edit the alpha content entry");

        let events = collect_tick_events(sse, SSE_FIRST_EVENT_DEADLINE, SSE_QUIET_WINDOW).await;
        // The deliverable: the observed SSE sequence is printed regardless
        // of pass/fail so a healthy-baseline PASS is still recorded data
        // for sub #2094's variant hunt.
        eprintln!(
            "[dev_content_reload_2063_e2e] [{label}] observed SSE event sequence after the \
             content edit: {events:?}"
        );

        for name in &events {
            assert!(
                matches!(name.as_str(), "page" | "css" | "islands"),
                "[{label}] unexpected SSE event name {name:?}; observed sequence: {events:?}\n{}",
                session.logs(),
            );
        }
        let page_count = events.iter().filter(|name| name.as_str() == "page").count();
        assert!(
            page_count >= 1,
            "[{label}] expected at least one `page` SSE event after the content edit within \
             {}s; observed sequence: {events:?}\n{}",
            SSE_FIRST_EVENT_DEADLINE.as_secs(),
            session.logs(),
        );
        assert_eq!(
            page_count,
            1,
            "[{label}] expected EXACTLY ONE `page` SSE event for one content-edit tick (a \
             second `page` event means the open tab would be told to reload twice for one \
             edit) — a same-tick `css`/`islands` companion is fine, a duplicate `page` is not; \
             observed sequence: {events:?}\n{}",
            session.logs(),
        );

        // The edit must have actually landed — rules out a vacuous SSE
        // pass where the assertions above happened to hold with nothing
        // rendered.
        poll_until_response_contains(
            &client,
            &format!("{base}/posts/alpha"),
            "V2-BODY-ALPHA",
            &format!("[{label}] entry rerender after content edit"),
            &session,
        )
        .await;

        // #2064 (fixed on `main` as of 6296428d): the zero-`paths()`
        // sibling route (`pages/archive/[year].tsx`) must be a tolerated
        // `page_sources` member, not trigger the pre-fix
        // "content provenance unavailable" boot warning. Checked here,
        // at the very end (rather than right after boot), to give the
        // deferred boot-render task the most possible wall-clock time to
        // have logged it if it was going to. Under Cold boot this is
        // vacuously true (the eager boot render — and the only call site
        // of `complete_boot_content_provenance` — never runs), but it is
        // a meaningful check under the default boot mode.
        assert!(
            !session.logs().contains("content provenance unavailable"),
            "[{label}] the zero-`paths()` sibling route triggered the pre-#2064 boot warning; \
             `posts` collection provenance must complete cleanly on boot.\n{}",
            session.logs(),
        );

        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] [{label}] dev_content_reload_2063_e2e did not finish within {}s. \
             Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn content_edit_emits_exactly_one_page_event_default_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_scenario(None, "default boot").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn content_edit_emits_exactly_one_page_event_cold_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_scenario(Some("cold"), "Cold boot (ZFB_DEV_BOOT_LAZY=cold)").await;
}
