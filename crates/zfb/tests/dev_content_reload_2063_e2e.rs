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

// Sub #2094 (variant matrix) additions below use these directly rather
// than spawning a real `zfb dev` process, for cell (d) — see that cell's
// own header comment.
use zfb_build::{AssetPipeline, BuildContext, BuildOrchestrator, BuildOutcome, OrchestratorConfig};
use zfb_graph::{DependencyGraph, PageDeps, PageId};
use zfb_server::outcome_to_events;

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

/// Dedicated client for `/__zfb/reload` subscriptions, deliberately built
/// WITHOUT a total-response timeout.
///
/// `build_reqwest_client`'s `.timeout(...)` bounds the whole response, not
/// just its headers. For an SSE stream the response body never completes,
/// so that setting silently caps how long ANY subscription can observe —
/// no matter what `SSE_FIRST_EVENT_DEADLINE`, `SSE_QUIET_WINDOW`, or
/// `OVERALL_DEADLINE` say. A tick that takes longer than the cap surfaces
/// as a reqwest transport error ("operation timed out") from inside
/// `next_sse_event_name`/`collect_tick_events`, which reads as a broken
/// harness rather than as the zero-`page`-events outcome this file exists
/// to be able to observe. Measured (sub #2094): the injected-route
/// fixture's first tick restages injected-route bundles and crosses 10s,
/// while the baseline fixture's cheap MDX warmup lands well under it — so
/// the cap was invisible until a slower fixture arrived.
///
/// The connect timeout is kept: failing to CONNECT is a real error, and
/// bounding it does not truncate a healthy stream.
static SSE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("build SSE reqwest client")
});

async fn subscribe_sse(base: &str) -> reqwest::Response {
    let response = SSE_CLIENT
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

async fn drain_ticks_until_quiescent(base: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        let sse = subscribe_sse(base).await;
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
///
/// `warmup_path` is an ABSOLUTE path to the file this handshake edits
/// repeatedly. Generalized (sub #2094) beyond a hardcoded
/// `content/posts/__warmup.mdx` MDX rewrite in two ways:
///
/// - the injected/out-of-root matrix fixtures split an in-project
///   `project/` dir from a sibling out-of-root `shared-content/` dir —
///   the warmup entry there does not live under `session.root` at all;
/// - `render_revision` generates each revision's file bytes, because the
///   injected-route matrix fixture (cell (a)+(c2)) CANNOT use a content
///   entry here at all: its `posts` collection has no in-project
///   consumer other than the dynamic injected route under test, which
///   is SSE-dark by design (the very behavior this file exists to
///   characterize) — so a content-entry warmup edit there would never
///   produce an SSE event either, making this liveness handshake hang
///   for the wrong reason instead of proving the watcher is live. That
///   fixture instead warms up its STATIC injected `/` route's own
///   component source (`pkg/home.tsx`), an ordinary Module dependency
///   edit wholly unrelated to the Content-provenance path under test.
///
/// `warmup_interval` is the delay between successive warmup rewrites — see
/// `MatrixFixture::warmup_interval`'s doc comment for why this must also
/// vary by fixture (manager finding, 2026-07, alongside the SSE-client
/// total-timeout bug fixed above).
async fn confirm_watcher_live(
    session: &DevSession,
    base: &str,
    warmup_path: &Path,
    render_revision: fn(u32) -> String,
    warmup_interval: Duration,
) {
    let sse = subscribe_sse(base).await;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let warmup = warmup_path.to_path_buf();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut revision = 0u32;
            while !stop.load(Ordering::SeqCst) {
                fs::write(&warmup, render_revision(revision)).expect("edit existing warmup entry");
                revision += 1;
                tokio::time::sleep(warmup_interval).await;
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

/// Default warmup-rewrite cadence — the baseline/out-of-root fixtures'
/// cheap MDX warmup always lands well under this.
const WARMUP_INTERVAL_DEFAULT: Duration = Duration::from_millis(400);

async fn boot_and_handshake(
    session: &mut DevSession,
    warmup_path: &Path,
    render_revision: fn(u32) -> String,
    warmup_interval: Duration,
) -> Option<(String, reqwest::Client)> {
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

    confirm_watcher_live(
        session,
        &base,
        warmup_path,
        render_revision,
        warmup_interval,
    )
    .await;

    Some((base, client))
}

/// The baseline/out-of-root fixtures' warmup content-entry revision
/// generator (mirrors the original hardcoded body #2093 wrote).
fn render_mdx_warmup_revision(revision: u32) -> String {
    format!("---\ntitle: Warmup\ndate: 2025-01-01\n---\n\nWarmup revision {revision}.\n")
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
        let warmup_path = session.root.join("content/posts/__warmup.mdx");
        let Some((base, client)) = boot_and_handshake(
            &mut session,
            &warmup_path,
            render_mdx_warmup_revision,
            WARMUP_INTERVAL_DEFAULT,
        )
        .await
        else {
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
        drain_ticks_until_quiescent(&base).await;

        let sse = subscribe_sse(&base).await;
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

// ============================================================================
// Sub #2094 (epic #2092, Wave 1): variant repro matrix.
//
// Sub #2093 (above) proved the healthy baseline. This section extends the
// SAME harness across the suspicious shapes the epic names, in the epic's
// mandated run order:
//
//   (a)+(c2) — a dynamic INJECTED route (zudo-doc-style route injection)
//              over an out-of-root collection, combined with the
//              "no in-project `pages/` consumer at all" empty-known-page-
//              universe shape. The epic's own text names this the single
//              most #2063-relevant cell.
//   (c1)     — an ORDINARY out-of-root collection, no narrowing hook (the
//              #1038 baseline) — a healthy-characterization control.
//   (b)      — plan/tables key drift (`lazy_render_tick`'s `None => continue`
//              at `crates/zfb/src/commands/dev.rs:8199`). DOCUMENTED AND
//              SKIPPED below — see that section for why.
//   (d)      — a simulated provenance-wipe world, driven as a focused
//              in-process test (no real `zfb dev` process, no new
//              production fault-injection seam — see that cell's own
//              header comment for why).
//
// No early exit: every cell below runs regardless of what an earlier cell
// found, per the epic's explicit instruction.
// ============================================================================

/// Describes one process-based matrix cell fixture, generalizing
/// `run_scenario` above beyond the flat single-root baseline layout: the
/// injected/out-of-root fixture families split an in-project `project/`
/// dir from a sibling out-of-root `shared-content/` dir (mirroring a real
/// `allowOutsideRoot: true` collection config), so `zfb dev` must be
/// spawned in the `project/` subdirectory of the copied fixture tree while
/// the edited content entry and the warmup entry live outside it.
struct MatrixFixture {
    /// Directory name under `tests/fixtures/`.
    family_dir: &'static str,
    /// Subdirectory (relative to the copied family root) `zfb dev` is
    /// spawned in.
    project_subdir: &'static str,
    /// Path (relative to the copied family root) of the warmup entry
    /// `confirm_watcher_live` edits repeatedly.
    warmup_rel: &'static str,
    /// Generates each revision's bytes for `warmup_rel` — see
    /// `confirm_watcher_live`'s doc comment for why this must vary by
    /// fixture rather than always being an MDX content rewrite.
    warmup_render_revision: fn(u32) -> String,
    /// Delay between successive warmup rewrites. Manager finding
    /// (2026-07): `confirm_watcher_live`'s original unconditional 400ms
    /// cadence works for a cheap MDX rewrite, but the injected fixture's
    /// warmup edit (a TSX component source) triggers a real rebundle —
    /// churning it every 400ms can invalidate an in-flight rebundle
    /// before it ever reaches its publish/SSE step, so that fixture
    /// needs a much longer interval (comfortably above its observed
    /// ~8s first-tick latency) to let one edit's tick actually complete.
    warmup_interval: Duration,
    /// Path (relative to the copied family root) of the entry this
    /// scenario edits for its own assertion.
    entry_rel: &'static str,
    /// `GET /` readiness-probe route and its expected marker.
    home_route: &'static str,
    home_marker: &'static str,
    /// The route this scenario polls before and after the edit.
    entry_route: &'static str,
    v1_marker: &'static str,
    v2_marker: &'static str,
    /// New bytes written to `entry_rel` for the edit.
    edit_contents: &'static str,
}

fn matrix_family_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Shared scenario body for the process-based matrix cells below — the
/// same shape as `run_scenario`, generalized over `MatrixFixture` instead
/// of the single hardcoded baseline layout. See `run_scenario`'s own doc
/// comment for the rationale behind every wait/assert primitive reused
/// here (bounded quiet window, exactly-one-`page`-event discipline,
/// freshness check regardless of SSE outcome).
async fn run_matrix_scenario(fixture: &MatrixFixture, boot_lazy: Option<&str>, label: &str) {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_content_reload_2063_e2e] [{label}] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let temp = tempfile::tempdir().expect("create tempdir for matrix fixture");
    let family_root = temp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&matrix_family_dir(fixture.family_dir), &family_root)
        .expect("copy matrix fixture family");

    let project_root = family_root.join(fixture.project_subdir);
    let warmup_path = family_root.join(fixture.warmup_rel);
    let entry_path = family_root.join(fixture.entry_rel);

    let mut session = spawn_dev(project_root, &esbuild, boot_lazy);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) = boot_and_handshake(
            &mut session,
            &warmup_path,
            fixture.warmup_render_revision,
            fixture.warmup_interval,
        )
        .await
        else {
            return ScenarioOutcome::Skipped;
        };

        // Readiness probe: confirm the family's `GET /` route (whatever it
        // is for this fixture) answers before touching the entry under
        // test.
        poll_until_response_contains(
            &client,
            &format!("{base}{}", fixture.home_route),
            fixture.home_marker,
            &format!("[{label}] boot readiness probe ({})", fixture.home_route),
            &session,
        )
        .await;

        // Confirm the entry's initial content is served before editing it.
        poll_until_response_contains(
            &client,
            &format!("{base}{}", fixture.entry_route),
            fixture.v1_marker,
            &format!("[{label}] boot entry route ({})", fixture.entry_route),
            &session,
        )
        .await;

        // Settle any trailing boot/handshake tick before subscribing —
        // see `run_scenario`'s identical step for why.
        drain_ticks_until_quiescent(&base).await;

        let sse = subscribe_sse(&base).await;
        fs::write(&entry_path, fixture.edit_contents).expect("edit the matrix fixture entry");

        let events = collect_tick_events(sse, SSE_FIRST_EVENT_DEADLINE, SSE_QUIET_WINDOW).await;
        // The deliverable for every matrix cell: the observed SSE sequence
        // is printed regardless of pass/fail, so a red cell's exact
        // symptom (and a healthy cell's confirmation) both become
        // recorded evidence for the epic's decision sub (#2096).
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

        // The freshness check runs BEFORE the page-event-count
        // assertions below (codex review finding on this cell,
        // 2026-07): the defining #2063 symptom is fresh served bytes
        // PAIRED WITH zero `page` events. If the event-count assertion
        // ran first and this cell reproduces (zero events), the test
        // would panic before ever confirming the edit reached disk at
        // all — leaving no evidence to distinguish "the reported
        // regression" from "the edit was never even processed". Running
        // the freshness poll first means a red cell's failure output
        // still records that server-side freshness half of the story.
        poll_until_response_contains(
            &client,
            &format!("{base}{}", fixture.entry_route),
            fixture.v2_marker,
            &format!("[{label}] entry rerender after content edit"),
            &session,
        )
        .await;

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

        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] [{label}] dev_content_reload_2063_e2e matrix cell did not finish within \
             {}s. Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

// ----------------------------------------------------------------------------
// Matrix cell (a)+(c2): dynamic INJECTED route over an out-of-root
// collection, project has NO `pages/` directory at all.
//
// RED -> INVERT CONVENTION: this test's assertions are written in DESIRED
// POST-FIX FORM (assert exactly one `page` event), matching every other
// cell/baseline test in this file. Per the epic's own world-fact #2 and
// this fixture's header comment (`preset.mjs` /
// `pkg/injected-post.tsx`), the PREDICTED result today is ZERO `page`
// events:
//   - the project's whole known-page universe (`routes_by_source`) is
//     empty (no `pages/` dir at all), so `lazy_render_tick`'s per-page
//     loop over the `PageSelection::All` fallback (out-of-root edit, no
//     `external_invalidation` hook configured) finds nothing and
//     `pages_stale` stays empty;
//   - `restale_dynamic_injected` (`crates/zfb/src/commands/dev.rs:3899`)
//     re-stales the previously-rendered injected route at the table swap
//     WITHOUT pushing to `tick_stale` (by its own doc comment), so it
//     never reaches `BuildOutcome::pages_stale` either;
//   - `outcome_to_events`'s `Page` gate (`crates/zfb-server/src/livereload.rs`)
//     therefore never fires, even though the served bytes ARE fresh on
//     the next request (confirmed by this test's own freshness poll).
//
// If this test fails today with an OBSERVED SEQUENCE OF `[]` (zero
// events), that is NOT a test bug — it is the epic's target regression.
// A future wave (#2097, "the fix") should be able to flip this test green
// WITHOUT touching any assertion below, only implementing the fix epic
// #2092 Wave 3 (#2096) locks.
// ----------------------------------------------------------------------------

/// Warmup revision generator for the injected fixture's `pkg/home.tsx` —
/// see `confirm_watcher_live`'s doc comment for why this fixture cannot
/// use a content-entry warmup like the other two matrix fixtures. Keeps
/// the `INJECTED_ROOT_OK` marker present at every revision (the `home_marker`
/// polled after boot) and stays valid TSX; only a revision-numbered
/// leading comment changes between writes.
fn render_injected_home_warmup_revision(revision: u32) -> String {
    format!(
        "// warmup revision {revision}\n\
         export default function InjectedHome() {{\n\
         \x20 return (\n\
         \x20   <html lang=\"en\">\n\
         \x20     <head>\n\
         \x20       <meta charSet=\"utf-8\" />\n\
         \x20       <title>dev-content-reload-2063-injected fixture</title>\n\
         \x20     </head>\n\
         \x20     <body>\n\
         \x20       <h1>INJECTED_ROOT_OK</h1>\n\
         \x20     </body>\n\
         \x20   </html>\n\
         \x20 );\n\
         }}\n"
    )
}

fn injected_matrix_fixture() -> MatrixFixture {
    MatrixFixture {
        family_dir: "dev-content-reload-2063-injected",
        project_subdir: "project",
        warmup_rel: "project/pkg/home.tsx",
        warmup_render_revision: render_injected_home_warmup_revision,
        // Well above the ~8s first-tick latency the manager observed by
        // hand for this fixture (one `pkg/home.tsx` append -> `page`
        // event), so a subsequent warmup rewrite cannot interrupt the
        // in-flight rebundle before it publishes.
        warmup_interval: Duration::from_secs(15),
        entry_rel: "shared-content/posts/alpha.mdx",
        home_route: "/",
        home_marker: "INJECTED_ROOT_OK",
        entry_route: "/injected-posts/alpha",
        v1_marker: "V1-BODY-ALPHA-INJECTED",
        v2_marker: "V2-BODY-ALPHA-INJECTED",
        edit_contents: "---\ntitle: Alpha V2 Frontmatter Injected\ndate: 2026-01-02\n---\n\nV2-BODY-ALPHA-INJECTED updated markdown body.\n",
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_dynamic_route_content_edit_emits_exactly_one_page_event_default_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_matrix_scenario(
        &injected_matrix_fixture(),
        None,
        "matrix (a)+(c2) default boot",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_dynamic_route_content_edit_emits_exactly_one_page_event_cold_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_matrix_scenario(
        &injected_matrix_fixture(),
        Some("cold"),
        "matrix (a)+(c2) Cold boot (ZFB_DEV_BOOT_LAZY=cold)",
    )
    .await;
}

// ----------------------------------------------------------------------------
// Matrix cell (c1): an ORDINARY out-of-root collection, no narrowing hook
// (the #1038 external-override baseline) — a healthy-characterization
// control paired with cell (a)+(c2) above. Unlike the injected fixture,
// `/posts/[slug]` is an ORDINARY in-project dynamic page, so it IS a
// member of `routes_by_source`; the out-of-root edit's `PageSelection::All`
// conservative fallback (no `external_invalidation` hook configured here
// either) marks its expanded routes stale through the normal tick-side
// channel, which DOES reach `BuildOutcome::pages_stale`. This cell is
// therefore predicted to PASS like the healthy baseline — evidence that
// the out-of-root shape ALONE (without an injected route in the mix) is
// not the #2063 trigger.
// ----------------------------------------------------------------------------

fn out_of_root_matrix_fixture() -> MatrixFixture {
    MatrixFixture {
        family_dir: "dev-content-reload-2063-outofroot",
        project_subdir: "project",
        warmup_rel: "shared-content/posts/__warmup.mdx",
        warmup_render_revision: render_mdx_warmup_revision,
        warmup_interval: WARMUP_INTERVAL_DEFAULT,
        entry_rel: "shared-content/posts/alpha.mdx",
        home_route: "/",
        home_marker: "dev-content-reload-2063-outofroot",
        entry_route: "/posts/alpha",
        v1_marker: "V1-BODY-ALPHA-OOR",
        v2_marker: "V2-BODY-ALPHA-OOR",
        edit_contents: "---\ntitle: Alpha V2 Frontmatter OOR\ndate: 2026-01-02\n---\n\nV2-BODY-ALPHA-OOR updated markdown body.\n",
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_out_of_root_content_edit_emits_exactly_one_page_event_default_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_matrix_scenario(
        &out_of_root_matrix_fixture(),
        None,
        "matrix (c1) default boot",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_out_of_root_content_edit_emits_exactly_one_page_event_cold_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_matrix_scenario(
        &out_of_root_matrix_fixture(),
        Some("cold"),
        "matrix (c1) Cold boot (ZFB_DEV_BOOT_LAZY=cold)",
    )
    .await;
}

// ----------------------------------------------------------------------------
// Matrix cell (b): plan/tables key drift — DOCUMENTED AND SKIPPED.
//
// The epic names `lazy_render_tick`'s silent `None => continue`
// (`crates/zfb/src/commands/dev.rs:8199`, reached when a page in the
// tick's `pages: &[PageId]` plan has no matching entry in the LIVE
// `routes_by_source` table) as a candidate for this cell. No construction
// was found that exercises it without contriving unrealistic internal
// state, for two independent reasons:
//
// 1. `lazy_render_tick` is a private, non-`pub` free function inside
//    `crates/zfb/src/commands/dev.rs` — this file is an EXTERNAL
//    integration test (`crates/zfb/tests/`), which only sees the `zfb`
//    library's public API. There is no black-box lever to call it
//    directly, unlike cell (d) below (which composes only genuinely
//    `pub` orchestrator/pipeline machinery).
//
// 2. Even granting internal access (e.g. from a `#[cfg(test)] mod tests`
//    inside `dev.rs` itself), the `pages` slice `lazy_render_tick`
//    iterates and the `routes_by_source` table it reads are DERIVED FROM
//    THE SAME SOURCE OF TRUTH within one tick — the plan comes from the
//    dependency graph the same P4 route-table swap that populates
//    `routes_by_source` also updates, both under the dev session's own
//    internal locking discipline. Manufacturing a real desync between
//    them would require either (a) a genuine mid-tick race window this
//    black-box e2e harness has no lever to hit deterministically, or
//    (b) a new test-only seam that mutates `routes_by_source` out from
//    under a plan already computed against the old table — exactly the
//    kind of contrived, unrealistic state the epic instructs this sub to
//    avoid rather than force.
//
// No test is added for this cell. If a future wave discovers a genuine
// realistic trigger for this drift (e.g. a specific watcher-coalescing
// shape), it should get its own dedicated regression test at that point,
// not a synthetic non-repro added here.
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// Matrix cell (d): simulated provenance-wipe world.
//
// The epic's own overview already DISPROVED the reporter's provenance-
// starvation hypothesis as the root cause: a provenance wipe degrades to
// `PageSelection::All`, which POPULATES `pages_stale` (pinned by
// `content_edit_after_a_provenance_wipe_falls_back_to_a_full_rebuild`,
// `crates/zfb-build/src/orchestrator.rs`). This cell's job is to compose
// that already-proven half of the story with the SSE-emission half
// (`outcome_to_events`, `crates/zfb-server/src/livereload.rs`) so the
// epic has one piece of end-to-end evidence, at unit-test speed, that
// this world does NOT reproduce the reported symptom.
//
// SEAM CONSTRAINT (binding, epic #2092 / sub #2094): no production
// fault-injection path may be left behind, and any temporary env knob
// used for diagnosis must be removed before the epic closes. This test
// adds NEITHER: it is a plain, non-`#[tokio::test]`, non-e2e unit-style
// test that spawns no real `zfb dev` process at all (so "both boot
// modes" does not apply to this cell — there is no boot). It only calls
// two genuinely `pub` production entry points exactly as production code
// does:
//
//   1. `zfb_build::BuildOrchestrator::plan_for_changes` against a
//      `DependencyGraph` constructed to look like the POST-WIPE world —
//      a page that survives with its `Content` edge already gone (the
//      exact shape `content_edit_after_a_provenance_wipe_falls_back_to_a_full_rebuild`
//      uses) — confirming `PageSelection::All` fires for a content-path
//      change the graph no longer recognizes.
//   2. `zfb_server::outcome_to_events` against a `BuildOutcome` whose
//      `pages_stale` is populated the way a real lazy dev tick's
//      `PageSelection::All` fallback populates it — every currently
//      known page's output path (not the drifted/removed one) — proving
//      the SSE `Page` gate fires exactly once.
//
// A `NoopPipeline` is needed only to satisfy `BuildOrchestrator<P>`'s
// generic bound; its `apply` is never invoked (this test only calls
// `plan_for_changes`, which needs no pipeline execution).
#[derive(Debug, Default, Clone)]
struct NoopPipeline;

impl AssetPipeline for NoopPipeline {
    fn apply(
        &self,
        _plan: &zfb_build::RebuildPlan,
        _ctx: &BuildContext,
    ) -> anyhow::Result<BuildOutcome> {
        unreachable!("NoopPipeline::apply is never invoked by this test — only plan_for_changes is exercised");
    }
}

#[test]
fn simulated_provenance_wipe_world_still_populates_pages_stale_and_emits_one_page_event() {
    let surviving_page = PageId::new(PathBuf::from("/proj/pages/posts/[slug].tsx"));
    let content_path = PathBuf::from("/proj/content/posts/alpha.mdx");

    // The post-wipe shape: the page survives in the graph, but its
    // `Content` edge to `content_path` does NOT — mirroring
    // `content_edit_after_a_provenance_wipe_falls_back_to_a_full_rebuild`'s
    // own graph construction exactly (page present, deps empty).
    let mut graph = DependencyGraph::new();
    graph.upsert(PageDeps::new(surviving_page.clone(), vec![]));

    let orchestrator = BuildOrchestrator::new(
        OrchestratorConfig::new(
            "/proj",
            vec![PathBuf::from("pages"), PathBuf::from("content")],
        ),
        Arc::new(std::sync::Mutex::new(graph)),
        NoopPipeline,
    );

    let plan = orchestrator.plan_for_changes(vec![content_path]);
    assert!(
        plan.pages.is_all(),
        "a content path the graph no longer knows (the simulated provenance-wipe world) must \
         take the conservative whole-site fallback; got {:?}",
        plan.pages,
    );

    // Mirror what a real lazy dev tick does with a `PageSelection::All`
    // plan: every currently-known page's output gets marked stale (see
    // `lazy_render_tick`'s doc comment — "everything else ... ALL
    // selected routes are marked stale"). Only ONE page is known post-
    // wipe here (the surviving page above), so `pages_stale` carries
    // exactly its output.
    let outcome = BuildOutcome {
        pages_stale: vec![PathBuf::from("dist/posts/alpha/index.html")],
        ..BuildOutcome::default()
    };

    let events = outcome_to_events(&outcome);
    let page_count = events
        .iter()
        .filter(|event| matches!(event, zfb_server::ReloadEvent::Page))
        .count();
    assert_eq!(
        page_count, 1,
        "the simulated provenance-wipe world's `PageSelection::All` fallback must still reach \
         a non-empty `pages_stale`, which `outcome_to_events` turns into exactly one `page` SSE \
         event — this is the epic's DISPROOF of the reporter's provenance-starvation hypothesis, \
         not a repro; observed events: {events:?}",
    );
}
