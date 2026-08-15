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
//! Permanent registration (sub #2098, epic #2092 Wave 5): per
//! zudo-test-wisdom's mechanical, first-match `#[ignore]` classification
//! rules, none of the 6 fire here. Rule 1 (`env-gate:` — needs an env var
//! / external binary the PR runner cannot provide) does NOT apply: every
//! one of these functions already self-skips via `locate_esbuild()` at
//! its own top (`let Some(esbuild) = locate_esbuild() else { ...; return; }`),
//! exactly the convention `wasm_ssr_dev_smoke_e2e` and
//! `embedded_host_request_time_e2e` use — both of which are documented in
//! crates/CLAUDE.md as "NOT `#[ignore]`d — it self-skips at
//! runtime via `locate_esbuild()`". health.yml's T1 gate always stages a
//! pinned esbuild, so the runner CAN provide it; the self-skip is a
//! local-dev convenience only, not a CI blocker. Rule 3 (`heavy:` —
//! runtime over budget) doesn't fire either: the Level-4 confirm run
//! (below) measured **92.53s for the full 7-cell matrix** (~13-15s/test
//! average), well under the per-test budgets other un-ignored real
//! `zfb dev` e2e tests in this crate already carry (e.g. `dev_serve_e2e`'s
//! own watchdogs run up to 280s). So **rule 7 applies: no `#[ignore]`,
//! these run on every T1 gate** like their siblings. The binary IS still
//! registered in `.config/nextest.toml`'s `[test-groups.e2e-heavy]` — that
//! registration is about CPU/memory serialization against other real
//! `zfb dev`/`zfb build` processes, completely orthogonal to `#[ignore]`
//! status (several other group members, including the two named above,
//! are un-ignored too). No exam.yml wiring or crates/CLAUDE.md `#[ignore]`
//! manifest row is needed, since there is nothing to schedule into a
//! weekly allowed-to-fail lane — these tests already run, and are already
//! required to pass, on every PR.
//!
//! `simulated_provenance_wipe_world_still_populates_pages_stale_and_emits_one_page_event`
//! below is a plain unit test with no real `zfb dev` boot at all (see its
//! own header comment) and was never a candidate for any tag.
//!
//! ## Revert-proof (sub #2098, per the #2002/#2004 idiom)
//!
//! Performed by the manager at Level 4 on this file's FINAL, sharpened
//! fixture (`c073cd04`), not re-run by this sub — see issue #2097's
//! closing comment for the full transcript.
//!
//! **Seam disabled**: `crates/zfb/src/commands/dev.rs`, the raise inside
//! `DevRenderInner::restale_dynamic_injected`, changed to
//! `if false && restaled_any { self.set_dynamic_injected_restaled(); }`.
//!
//! **Observed** (both boot modes, identically):
//!
//! ```text
//! observed delivery: [zfb-timing] tick(): kinds=[alpha.mdx:Modified] eager_hint=true fan_out_safe=true
//! observed SSE event sequence after the content edit: []
//! panicked: expected at least one `page` SSE event ... observed sequence: []
//! ```
//!
//! i.e. delivery proven, served bytes fresh, **zero SSE events** — the
//! exact pre-fix #2063 symptom, reproduced in-harness on this epic's own
//! shipping fixture (not a stubbed/simulated one).
//!
//! **Restored**: `git status` clean, `git diff` empty at `c073cd04`, then
//! the full matrix run: **7 passed, 0 failed** (92.53s) — the two
//! injected-fixture cells flipped `[]` -> `["page"]`; the other five
//! (baseline + out-of-root, both boot modes, plus the plain unit test)
//! were unchanged throughout.

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

/// Per-iteration SSE quiet window inside `drain_ticks_until_quiescent`.
const DRAIN_QUIET_WINDOW: Duration = Duration::from_millis(1500);

/// Consecutive quiet+stable iterations `drain_ticks_until_quiescent`
/// requires before declaring the pipeline idle — see its doc comment for
/// why one window is not enough (silent gaps inside a running tick).
const DRAIN_STABLE_ROUNDS: u32 = 2;

/// Overall bound on `drain_ticks_until_quiescent` (it falls through with
/// a loud diagnostic rather than hanging).
const DRAIN_DEADLINE: Duration = Duration::from_secs(20);

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
        // Surfaces `[zfb-timing] tick(): kinds=[<file>:<Kind>]` on stderr
        // (`crates/zfb-build/src/orchestrator.rs`) — proof a filesystem
        // event reached the orchestrator, independent of whatever the
        // tick decided to do about SSE. Harmless for every cell; load-
        // bearing for the injected-route cell (a)+(c2), whose whole
        // premise is that an SSE event is NOT a trustworthy liveness
        // signal (see `wait_for_tick_mentioning`'s doc comment). Same
        // env var / pattern as `mirror_css_scan_mdx_e2e.rs`'s `spawn_dev`.
        .env("ZFB_DEV_TIMING", "1")
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

/// Wait until the dev server's tick pipeline is genuinely idle before the
/// caller subscribes for its assertion window.
///
/// An SSE quiet window alone cannot see an IN-FLIGHT tick: on a loaded
/// machine a tick runs longer than any reasonable quiet threshold (2.1s
/// observed vs a 1.5s window), so the handshake's trailing warmup tick
/// could complete AFTER quiescence was declared and leak its `page` event
/// into the caller's assertion subscription — the exact `["page", "page"]`
/// duplicate this guard closes (observed on macOS, 2026-08; the product
/// emits one `page` per tick by construction, see
/// `zfb-server/src/livereload.rs`).
///
/// Quiescence therefore requires [`DRAIN_STABLE_ROUNDS`] CONSECUTIVE
/// iterations in which BOTH hold:
///
/// 1. **SSE quiet:** a subscription observes no event for
///    [`DRAIN_QUIET_WINDOW`] (an observed event drains it and resets the
///    streak — the pre-existing behavior).
/// 2. **stderr stable:** the dev server's stderr did not grow during that
///    window. An in-flight tick keeps writing `[zfb-timing]` lines
///    (`tick(): kinds=`, `bundle():`, `tick=<ms>`, `lazy-render …`), so
///    growth is read as pipeline activity and extends draining. This is a
///    LENGTH DELTA per iteration, deliberately not a start/completion
///    line-count balance: the timing lines are not a matched pair (a
///    no-op tick prints a start with no completion, the deferred boot
///    publish prints a completion with no start), so any cumulative
///    balance check wedges permanently on the first mismatch. A delta
///    heuristic's worst failure mode is only a longer drain, and the SSE
///    drain itself runs on every iteration regardless.
///
/// Requiring [`DRAIN_STABLE_ROUNDS`] consecutive quiet+stable windows puts
/// the effective quiet horizon (>= 3s) above the longest observed silent
/// gap inside a running tick (~1.1s, the bundle `asm` phase).
///
/// Bounded by [`DRAIN_DEADLINE`]; on timeout a loud diagnostic is printed
/// and the caller proceeds — the scenario's own assertions then fail with
/// full logs rather than hanging, and the diagnostic distinguishes a
/// degraded drain from a healthy one.
async fn drain_ticks_until_quiescent(session: &DevSession, base: &str) {
    let start = Instant::now();
    let mut stable_rounds = 0u32;
    let mut last_len = read_log(&session.stderr_path).len();
    while start.elapsed() < DRAIN_DEADLINE {
        let sse = subscribe_sse(base).await;
        let observed_event = matches!(
            next_sse_event_name(sse, DRAIN_QUIET_WINDOW).await,
            Ok(Some(_))
        );
        let cur_len = read_log(&session.stderr_path).len();
        let stderr_grew = cur_len != last_len;
        last_len = cur_len;
        if observed_event || stderr_grew {
            stable_rounds = 0;
            continue;
        }
        stable_rounds += 1;
        if stable_rounds >= DRAIN_STABLE_ROUNDS {
            return;
        }
    }
    eprintln!(
        "[dev_content_reload_2063_e2e] drain_ticks_until_quiescent: pipeline never went \
         quiet+stable for {DRAIN_STABLE_ROUNDS} consecutive {}ms windows within {}s \
         (stable_rounds={stable_rounds} at deadline) — proceeding anyway; a duplicate-`page` \
         assertion failure after this line may be a drain shortfall rather than a product bug.",
        DRAIN_QUIET_WINDOW.as_millis(),
        DRAIN_DEADLINE.as_secs(),
    );
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

/// Wait until a `[zfb-timing] tick(): kinds=[...]` line whose `kinds` list
/// mentions `filename` appears in the dev server's stderr — proof a
/// filesystem event for that file reached the orchestrator (delivery),
/// independent of whatever the tick decided to do about SSE. Mirrors
/// `mirror_css_scan_mdx_e2e.rs`'s `wait_for_tick_mentioning` helper.
///
/// Load-bearing for the injected-route matrix cell (a)+(c2)
/// (`run_injected_matrix_scenario` below): that fixture's whole premise is
/// that `restale_dynamic_injected` re-stales without ever pushing to
/// `tick_stale`, so an SSE event is NOT a trustworthy "the watcher noticed
/// my edit" signal there — the manager's own manual repro attempt (2026-
/// 07) mistook an unrelated boot/deferred-publish tick for the tick of an
/// edit to a route source under `pkg/`, for exactly this reason, and
/// confirmed separately that `pkg/` is not a watch root at all (only
/// `pages/`, `content/`, `components/`, `layouts/`, `styles/`, `data/`,
/// `src/` are). This helper instead proves delivery from the SAME tick the
/// content-edit assertion below observes, via a channel (`ZFB_DEV_TIMING`
/// stderr tracing) that is unaffected by the SSE-dark bug under test.
async fn wait_for_tick_mentioning(session: &DevSession, filename: &str) -> String {
    let started = Instant::now();
    while started.elapsed() < SSE_FIRST_EVENT_DEADLINE {
        let stderr = read_log(&session.stderr_path);
        if let Some(line) = stderr
            .lines()
            .find(|line| line.contains("tick(): kinds=[") && line.contains(filename))
        {
            return line.to_string();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "no `[zfb-timing] tick(): kinds=[...]` line mentioning {filename:?} within {}s — the \
         filesystem event for the content edit never reached the orchestrator at all, which \
         would be a DIFFERENT bug than #2063 (no delivery vs. SSE-dark delivery)\n{}",
        SSE_FIRST_EVENT_DEADLINE.as_secs(),
        session.logs(),
    );
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

/// Repeated edits of a dedicated warmup entry (never touched by the test's
/// own assertions later) prove the watch stream is live without racing
/// the fixture's real entry the scenario edits later.
///
/// Used by the baseline (`__warmup.mdx`) and out-of-root/cell-(c1)
/// (`shared-content/posts/__warmup.mdx`) scenarios ONLY. The injected-route
/// matrix cell (a)+(c2) does NOT use this handshake at all — see
/// `run_injected_matrix_scenario`'s own header comment for why an
/// SSE-based liveness probe is the wrong instrument there, and
/// `wait_for_tick_mentioning` for the delivery-proof mechanism it uses
/// instead (manager finding, 2026-07: `pkg/` is not a watch root, so no
/// warmup edit under it — at any interval — would ever be observed).
///
/// `warmup_path` is an ABSOLUTE path to the file this handshake edits
/// repeatedly; `render_revision` generates each revision's bytes;
/// `warmup_interval` is the delay between successive rewrites — see
/// `MatrixFixture`'s doc comments for why these are still fields on that
/// struct even though only one matrix cell (c1) uses it today.
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
        drain_ticks_until_quiescent(&session, &base).await;

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

/// Describes one process-based matrix cell fixture driven through
/// `run_matrix_scenario`, generalizing `run_scenario` above beyond the flat
/// single-root baseline layout: the out-of-root fixture family splits an
/// in-project `project/` dir from a sibling out-of-root `shared-content/`
/// dir (mirroring a real `allowOutsideRoot: true` collection config), so
/// `zfb dev` must be spawned in the `project/` subdirectory of the copied
/// fixture tree while the edited content entry and the warmup entry live
/// outside it.
///
/// Currently used by cell (c1) only — the injected-route cell (a)+(c2)
/// has its own dedicated `run_injected_matrix_scenario` (manager finding,
/// 2026-07: that fixture cannot use this struct's `confirm_watcher_live`-
/// based liveness handshake at all; see that function's header comment).
/// The `warmup_render_revision`/`warmup_interval` fields stay generalized
/// (fn pointer / configurable cadence, not hardcoded to the baseline's
/// plain MDX rewrite) in case a future matrix cell needs a different
/// warmup shape, rather than narrowing back to what only cell (c1) needs
/// today.
struct MatrixFixture {
    /// Directory name under `tests/fixtures/`.
    family_dir: &'static str,
    /// Subdirectory (relative to the copied family root) `zfb dev` is
    /// spawned in.
    project_subdir: &'static str,
    /// Path (relative to the copied family root) of the warmup entry
    /// `confirm_watcher_live` edits repeatedly.
    warmup_rel: &'static str,
    /// Generates each revision's bytes for `warmup_rel`.
    warmup_render_revision: fn(u32) -> String,
    /// Delay between successive warmup rewrites.
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
        drain_ticks_until_quiescent(&session, &base).await;

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
// STEP 0 CORRECTION (issue #2097): this cell was a VACUOUS PASS as
// originally written. Its fixture registered a STATIC injected `/` route
// purely as the readiness probe, which put `/` into
// `injected_static_seeds` — and `mark_injected_seeds_stale` pushes to
// `tick_stale` unconditionally on EVERY route-table swap, so
// `pages_stale` was non-empty on every full-refresh tick regardless of
// the content edit. The `["page"]` #2094's matrix observed came from that
// seed, not from the dynamic injected channel under test. (The tell was
// in #2094's own output: this cell's header PREDICTED `[]` and OBSERVED
// `["page"]`, and nobody explained the delta.)
//
// The probe route is now GONE entirely, not merely made dynamic. The
// fixture registers EXACTLY ONE injected route and the readiness probe is
// that same route (`GET /injected-posts/alpha`). A dynamic probe route
// would have fixed `injected_static_seeds` but still joined
// `stale.dynamic_injected` once requested, so a post-fix `page` event
// would not have been attributable to the route under test alone — a
// milder instance of the very contamination that produced the vacuous
// pass. Now both sets contain only the route whose content this cell
// edits. See `preset.mjs`.
//
// With that correction the cell was confirmed RED (both boot modes:
// `observed SSE event sequence: []`, with the freshness poll passing and
// the `tick()` delivery line present) BEFORE any production change, and
// the fix below flipped it green with no assertion touched.
//
// RED -> INVERT CONVENTION: this test's assertions are written in DESIRED
// POST-FIX FORM (assert exactly one `page` event), matching every other
// cell/baseline test in this file. Per the epic's own world-fact #2 and
// this fixture's header comment (`preset.mjs` /
// `pkg/injected-post.tsx`), the PRE-FIX result was ZERO `page` events:
//   - the project's whole known-page universe (`routes_by_source`) is
//     empty (no `pages/` dir at all), so `lazy_render_tick`'s per-page
//     loop over the `PageSelection::All` fallback (out-of-root edit, no
//     `external_invalidation` hook configured) finds nothing and
//     `pages_stale` stays empty;
//   - `restale_dynamic_injected` (`crates/zfb/src/commands/dev.rs`)
//     re-stales the previously-rendered injected route at the table swap
//     WITHOUT pushing to `tick_stale` (by its own doc comment), so it
//     never reaches `BuildOutcome::pages_stale` either;
//   - `outcome_to_events`'s `Page` gate (`crates/zfb-server/src/livereload.rs`)
//     therefore never fired, even though the served bytes ARE fresh on
//     the next request (confirmed by this test's own freshness poll).
//
// THE FIX (issue #2097): `restale_dynamic_injected` still performs no
// `tick_stale` push — that shape is forbidden here, because a non-tick
// drain can swallow an in-flight tick's marks (the documented
// `run_and_broadcast` race). Instead it raises a separate sticky
// `BuildOutcome::dynamic_injected_restaled` bit, drained per tick like
// `ssr_routes_published` (#1826/#2002) and folded into the SAME single
// `Page` gate — so this cell now sees exactly one `page` event, and the
// #958/#1025/#1583 lazy-render narrowing is untouched.
//
// DOES NOT use `run_matrix_scenario`/`MatrixFixture`/`confirm_watcher_live`
// (manager finding, 2026-07): an SSE-based watcher-liveness handshake is
// the wrong instrument for a fixture whose entire premise is that SSE is
// dark for this collection. `pkg/` (the only place this fixture's route
// sources live) is not a watch root at all (only `pages/`, `content/`,
// `components/`, `layouts/`, `styles/`, `data/`, `src/` are), so an edit
// to anything under `pkg/` is never observed by the watcher regardless of
// rewrite interval — the manager's manual repro that seemed to show a
// `page` event ~8s after such an edit was in fact the unrelated boot/
// deferred-publish tick landing shortly after they subscribed, not a
// response to that edit. This cell instead proves delivery the way
// `mirror_css_scan_mdx_e2e.rs` does — `wait_for_tick_mentioning` above —
// keyed on the SAME single edit the assertion below observes
// (`shared-content/posts/alpha.mdx`), so delivery and the SSE-event
// collection race the same tick: this pins all three legs of #2063's
// signature together — event delivered, bytes fresh, zero `page` events.
// ----------------------------------------------------------------------------

async fn run_injected_matrix_scenario(boot_lazy: Option<&str>, label: &str) {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_content_reload_2063_e2e] [{label}] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let temp = tempfile::tempdir().expect("create tempdir for injected matrix fixture");
    let family_root = temp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(
        &matrix_family_dir("dev-content-reload-2063-injected"),
        &family_root,
    )
    .expect("copy injected matrix fixture family");

    let project_root = family_root.join("project");
    let entry_path = family_root.join("shared-content/posts/alpha.mdx");

    let mut session = spawn_dev(project_root, &esbuild, boot_lazy);
    let pgid = session.guard.pgid;
    let body = async {
        let Some(port) = wait_for_ready_port(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };
        let base = format!("http://localhost:{port}");
        let client = build_reqwest_client();

        // Readiness probe — inlined rather than via `boot_and_handshake`,
        // since this cell deliberately skips `confirm_watcher_live`'s
        // SSE-based liveness handshake entirely (see the header comment
        // above), and since `boot_and_handshake`'s shared boot loop probes
        // `GET /`, which this fixture deliberately does not serve.
        //
        // Probed at `/injected-posts/alpha` — the route UNDER TEST, not a
        // dedicated probe route (issue #2097 Step 0). This fixture
        // registers exactly one injected route, so `injected_static_seeds`
        // is empty AND `stale.dynamic_injected` can only ever contain the
        // route whose content the assertion below edits. Any separate probe
        // route, static or dynamic, would contaminate one of those two sets
        // — see `preset.mjs`'s header comment.
        let ready_start = Instant::now();
        loop {
            if matches!(
                client.get(format!("{base}/injected-posts/alpha")).send().await,
                Ok(response) if response.status().as_u16() == 200
            ) {
                break;
            }
            assert!(
                ready_start.elapsed() < BOOT_DEADLINE,
                "[{label}] GET /injected-posts/alpha never answered 200 within {}s after the \
                 ready banner.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // Confirm the entry's initial content is served before editing it.
        // This doubles as the readiness marker check — one route, one probe.
        poll_until_response_contains(
            &client,
            &format!("{base}/injected-posts/alpha"),
            "V1-BODY-ALPHA-INJECTED",
            &format!("[{label}] boot entry route (/injected-posts/alpha)"),
            &session,
        )
        .await;

        let sse = subscribe_sse(&base).await;
        fs::write(
            &entry_path,
            "---\ntitle: Alpha V2 Frontmatter Injected\ndate: 2026-01-02\n---\n\nV2-BODY-ALPHA-INJECTED updated markdown body.\n",
        )
        .expect("edit the injected fixture's content entry");

        // Delivery proof (see this cell's header comment) — keyed on the
        // SAME edit the SSE-collection/freshness assertions below
        // observe, so a delivery regression can never be misread as the
        // #2063 SSE-dark regression under test.
        let tick_line = wait_for_tick_mentioning(&session, "alpha.mdx").await;
        eprintln!("[dev_content_reload_2063_e2e] [{label}] observed delivery: {tick_line}");
        assert!(
            tick_line.contains("Modified") || tick_line.contains("Created"),
            "[{label}] expected the tick() line for alpha.mdx to report a Modified/Created \
             kind, got: {tick_line}\n{}",
            session.logs(),
        );

        let events = collect_tick_events(sse, SSE_FIRST_EVENT_DEADLINE, SSE_QUIET_WINDOW).await;
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

        // Freshness check runs BEFORE the page-event-count assertions —
        // see `run_matrix_scenario`'s identical ordering and its own
        // comment for why.
        poll_until_response_contains(
            &client,
            &format!("{base}/injected-posts/alpha"),
            "V2-BODY-ALPHA-INJECTED",
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
            "[watchdog] [{label}] dev_content_reload_2063_e2e injected matrix cell did not \
             finish within {}s. Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_dynamic_route_content_edit_emits_exactly_one_page_event_default_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_injected_matrix_scenario(None, "matrix (a)+(c2) default boot").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_dynamic_route_content_edit_emits_exactly_one_page_event_cold_boot() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    run_injected_matrix_scenario(
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
