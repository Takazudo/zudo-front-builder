//! Issue #1826 (Dev Self Heal epic #1999, sub-issue #2002): an SSR-only
//! project (every route `prerender = false`) DOES tell an already-open
//! browser tab to reload after the deferred dev bundle publishes — on the
//! healthy Cold-lazy boot path (#1182 / Cold premark #1808) and on the
//! cold-bootstrap recovery path (#1809) alike.
//!
//! Written RED in Wave 1 (#2000, then `ssr_reload_self_heal_red_e2e.rs` /
//! `red_ssr_only_*_never_reloads_open_tab`) and **inverted here in Wave 2**
//! (#2002), which landed the shared `ssr_routes_published` signal.
//!
//! ## The mechanism under test
//!
//! `outcome_to_events` (`crates/zfb-server/src/livereload.rs`) gates
//! `ReloadEvent::Page` on `pages_written` / `pages_stale` / `pages_pruned`
//! being non-empty, `client_scripts_changed`, or — since #2002 —
//! `ssr_routes_published`. Both self-heal channels for the deferred window
//! feed `pages_stale` from `mark_all_routes_stale`
//! (`crates/zfb/src/commands/dev.rs`), which walks `routes_by_source` — the
//! SSG route table. `prerender = false` routes live in the separate
//! `ssr_routes` table and have no `dist/` output path to mark, so an
//! SSR-only project drains an empty stale set on EITHER path. The fix is
//! the ONE shared bit both channels now raise via
//! `DevRenderSession::note_ssr_routes_published`:
//!
//! - **healthy** — the Cold-lazy deferred bundle publishes cleanly on its
//!   first attempt; the boot hook's step-0 success arm raises the bit right
//!   after `refresh_live_ssr_routes` publishes the live handle, and the
//!   boot drain folds it into the single `run_with_boot` broadcast.
//! - **cold-bootstrap recovery** — the deferred bundle FAILS once (arming
//!   `arm_cold_bootstrap_recovery`), then a later successful publish (a
//!   watcher-triggered `reload_renderer` tick after the source is fixed)
//!   calls `recover_cold_bootstrap_after_publish`, which raises the SAME
//!   bit through the SAME method; the tick pipeline's SSR-publish probe
//!   drains it.
//!
//! Serving already recovered on both paths before this fix — the SSR route
//! renders per-request the instant `refresh_live_ssr_routes` publishes the
//! live handle. What was missing was telling the open tab, which is what
//! these tests now assert.
//!
//! The two paths are asserted to be SYMMETRIC at the `BuildOutcome` level
//! by the unit tests in `crates/zfb/src/commands/dev.rs`
//! (`healthy_publish_and_cold_bootstrap_recovery_heal_an_ssr_only_tab_identically`);
//! these two e2e tests are the Level-4 confirmation that the same symmetry
//! survives the real dev server, real esbuild, and a real SSE stream.
//!
//! ## Fixture discipline
//!
//! Both scenarios use a project with a SINGLE page, `prerender = false`,
//! and NO collections / SSG routes at all — `routes_by_source` is
//! unconditionally empty for the whole session, so `mark_all_routes_stale`
//! marks nothing on every call and the reload can ONLY come from the shared
//! `ssr_routes_published` bit. That matches the epic's "all routes
//! `prerender = false`" acceptance shape exactly. No `node_modules` is
//! provisioned; the project falls back to the binary-embedded vendor
//! snapshot for `preact` (the same fallback `wasm_ssr_dev_smoke_e2e.rs`
//! relies on).
//!
//! ## Falsifiability
//!
//! - **Healthy path**: `ZFB_DEV_TEST_SLOW_BUNDLE_MS` opens an observable
//!   window between the ready banner (bind) and the deferred bundle's
//!   publish. The SSE stream is subscribed inside that window (simulating
//!   an already-open tab watching the dev 404 body), and the boot log's
//!   "no prebuilt dist/ seed required" line (unconditionally printed by
//!   `run_boot_render`'s Cold branch right after the deferred publish
//!   attempt concludes, success or failure) is the deterministic signal
//!   that the publish has already happened.
//! - **Cold-bootstrap recovery**: a module imported by the fixture's valid
//!   page starts with a genuine syntax error, so the boot's deferred bundle fails for real (proven
//!   by waiting for the exact `deferred_bundle_failure_message` Cold wording
//!   in stderr — not merely assumed). The SSE stream is subscribed AFTER
//!   that failure (simulating a tab that has been sitting on the dev 404
//!   since boot), then the source is fixed; the "cold-lazy bootstrap
//!   recovered" info line (`recover_cold_bootstrap_after_publish`'s only
//!   caller) is the deterministic signal that the recovery-latch publish
//!   happened, not merely that SOME tick ran.
//!
//! Both tests assert the TAB-FACING signal: after the deterministic
//! completion/recovery signal, a fresh manual GET returns 200 with the
//! page's marker (server-side recovery) AND the SSE stream subscribed
//! before that point delivers a `page` event (the browser was told). Each
//! test pins BOTH halves, so a regression that breaks only the broadcast
//! (leaving serving intact) is still caught.
//!
//! The failed-bootstrap scenario additionally asserts `/__zfb/ready`: the
//! failed Cold generation must report `documents: pending`, and the later
//! successful SSR-only route-table refresh must advance it to
//! `ready_on_request`. Serving recovered before the publication-state fix,
//! so the readiness assertion is the regression proof for the liveness bug.
//!
//! No sleeps are used to detect completion or recovery — both are observed
//! via distinct, unambiguous log lines emitted by the exact code paths
//! under test.
//!
//! ## Status
//!
//! GREEN as of #2002. Kept `#[ignore]`d as `env-gate: esbuild` (they spawn
//! a real `zfb dev` and need the pinned native binary); wiring them into a
//! CI lane is Wave 5/6's call (#2005, #2007), not theirs to self-promote.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{
    locate_esbuild, next_sse_event_name, open_sse, zfb_binary, CrossBinaryE2eLock,
};

const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const SIGNAL_DEADLINE: Duration = Duration::from_secs(60);
const CONTENT_DEADLINE: Duration = Duration::from_secs(60);
// How long to wait for the tab-facing reload event after the deterministic
// publish/recovery log line. Bounded, and NOT a sleep: `next_sse_event_name`
// returns the instant an event arrives and only uses this as its deadline.
const RELOAD_DEADLINE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
// `flock` serializes separate test binaries but is process-scoped on macOS,
// so both tests in this binary also need a local async guard. Always acquire
// the cross-binary lock first, matching the shared harness contract.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const HEALTHY_MARKER: &str = "SSR_HEALTHY_SELF_HEAL_MARKER";
const RECOVERY_MARKER: &str = "SSR_RECOVERY_SELF_HEAL_MARKER";

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

struct DevSession {
    guard: DevServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn stdout(&self) -> String {
        fs::read_to_string(&self.stdout_path).unwrap_or_default()
    }

    fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn combined(&self) -> String {
        format!("{}{}", self.stdout(), self.stderr())
    }

    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            self.stdout(),
            self.stderr(),
        )
    }
}

/// Extract the ephemeral port from the dev ready banner.
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = token.find(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

/// Minimal SSR-only fixture: one page, `prerender = false`, no collections
/// — `routes_by_source` (the SSG table `mark_all_routes_stale` walks) stays
/// empty for the whole session. `page_body` is the page module source.
fn write_ssr_only_fixture(root: &Path, page_body: &str) {
    fs::create_dir_all(root.join("pages")).expect("create pages/");
    fs::write(
        root.join("zfb.config.json"),
        "{\n  \"framework\": \"preact\"\n}\n",
    )
    .expect("write zfb.config.json");
    fs::write(root.join("pages/index.tsx"), page_body).expect("write pages/index.tsx");
}

fn healthy_page_source() -> String {
    format!(
        "export const prerender = false;\n\n\
         export default function HomePage() {{\n  \
         return (\n    <html lang=\"en\">\n      <body>{HEALTHY_MARKER}</body>\n    </html>\n  );\n\
         }}\n"
    )
}

fn recovery_fixed_page_source() -> String {
    format!(
        "import {{ trigger }} from \"../src/recovery-trigger\";\n\n\
         export const prerender = false;\n\n\
         export default function HomePage() {{\n  \
         return (\n    <html lang=\"en\">\n      <body>{RECOVERY_MARKER}{{trigger}}</body>\n    </html>\n  );\n\
         }}\n"
    )
}

fn spawn_dev(root: &Path, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_DIGEST_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_ISLANDS_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .env_remove("ZFB_DEV_TEST_SLOW_BUNDLE_MS");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.process_group(0);

    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Wait for the ready banner, returning the bound port. `None` on a
/// recognised environmental skip indicator.
async fn wait_for_ready(session: &mut DevSession) -> Option<u16> {
    let started = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = session.combined();
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[ssr_reload_self_heal] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild unavailable); skipping test.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before its ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&session.stdout()) {
            assert_ne!(
                port,
                0,
                "ready banner printed literal port 0 instead of the bound ephemeral port.\n{}",
                session.logs(),
            );
            return Some(port);
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until `needle` appears anywhere in the combined stdout+stderr log.
async fn wait_for_log_line(session: &DevSession, needle: &str, phase: &str) {
    let started = Instant::now();
    while started.elapsed() < SIGNAL_DEADLINE {
        if session.combined().contains(needle) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] no log line containing {needle:?} within {}s\n{}",
        SIGNAL_DEADLINE.as_secs(),
        session.logs(),
    );
}

async fn subscribe_sse(base: &str) -> reqwest::Response {
    let resp = open_sse(base).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "SSE live-reload endpoint must answer 200"
    );
    resp
}

async fn poll_get(client: &reqwest::Client, url: &str) -> (u16, String) {
    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            (status, body)
        }
        Err(e) => (0, format!("request error: {e}")),
    }
}

async fn readiness_json(client: &reqwest::Client, base: &str) -> serde_json::Value {
    let response = client
        .get(format!("{base}/__zfb/ready"))
        .send()
        .await
        .expect("GET /__zfb/ready");
    assert_eq!(response.status().as_u16(), 200, "readiness endpoint status");
    serde_json::from_str(&response.text().await.expect("read readiness response"))
        .expect("valid readiness JSON")
}

async fn wait_for_publication_ready(
    client: &reqwest::Client,
    base: &str,
    session: &DevSession,
) -> serde_json::Value {
    let started = Instant::now();
    let mut last = serde_json::Value::Null;
    while started.elapsed() < CONTENT_DEADLINE {
        last = readiness_json(client, base).await;
        if last["ready"] == true {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "publication readiness stayed false for {}s after Cold recovery; last snapshot: \
         {last}\n{}",
        CONTENT_DEADLINE.as_secs(),
        session.logs(),
    );
}

/// Poll `url` until the response body contains `needle` with status 200.
async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let started = Instant::now();
    let mut last = String::from("(no response yet)");
    while started.elapsed() < deadline {
        let (status, body) = poll_get(client, url).await;
        if status == 200 && body.contains(needle) {
            return;
        }
        last = format!("status {status}, body:\n{body}");
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve a body containing {needle:?} within {}s.\n\
         Last observation: {last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// SCENARIO 1 — healthy Cold-lazy deferred publish (#1182 / Cold premark #1808)
// ---------------------------------------------------------------------------

/// An SSR-only project's healthy (never-failed) Cold-lazy deferred boot
/// publish tells an already-open tab to reload, at the same moment the SSR
/// route becomes servable.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: esbuild — cargo test -p zfb --test ssr_reload_self_heal_e2e \
            -- --ignored --exact ssr_only_healthy_deferred_publish_reloads_open_tab \
            (ZFB_ESBUILD_BIN or the staged crates/zfb/binaries/esbuild slot). GREEN as \
            of issue #2002 — see epic #1999 / issue #1826."]
async fn ssr_only_healthy_deferred_publish_reloads_open_tab() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[ssr_reload_self_heal] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("ssr-reload-self-heal healthy fixture tempdir");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    write_ssr_only_fixture(&root, &healthy_page_source());

    // A generous slow-bundle window: long enough to reliably subscribe SSE
    // and observe the pre-publish 404 before the deferred bundle concludes,
    // short enough to keep the test's wall clock reasonable.
    let mut session = spawn_dev(
        &root,
        &esbuild,
        &[
            ("ZFB_DEV_BOOT_LAZY", "cold"),
            ("ZFB_DEV_TEST_SLOW_BUNDLE_MS", "4000"),
        ],
    );
    let Some(port) = wait_for_ready(&mut session).await else {
        return; // environmental skip
    };
    let base = format!("http://localhost:{port}");
    let index_url = format!("{base}/");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    // Subscribe to the livereload stream INSIDE the slow-bundle window —
    // this simulates a tab that is already open (and would be sitting on
    // the dev 404 body) the instant the deferred bundle publishes. Uses the
    // shared no-total-timeout SSE opener.
    let sse = subscribe_sse(&base).await;

    // Sanity: while the deferred bundle is still in flight, the SSR route
    // must not yet serve the marker (proves we really are inside the
    // pre-publish window, not racing past it).
    let (pre_status, pre_body) = poll_get(&client, &index_url).await;
    assert!(
        pre_status != 200 || !pre_body.contains(HEALTHY_MARKER),
        "GET {index_url} unexpectedly already served the marker before the deferred bundle \
         published — the ZFB_DEV_TEST_SLOW_BUNDLE_MS window closed before the SSE \
         subscription; widen it.\nstatus {pre_status}, body:\n{pre_body}\n{}",
        session.logs(),
    );

    // Deterministic completion signal: `run_boot_render`'s Cold branch
    // prints this line unconditionally right after the deferred bundle's
    // publish attempt concludes (success or failure) — see
    // `crates/zfb/src/commands/dev.rs` around the `BootLazyMode::Cold`
    // match arm in `run_boot_render`.
    wait_for_log_line(
        &session,
        "no prebuilt dist/ seed required",
        "healthy: boot completion",
    )
    .await;
    // This scenario is specifically the HEALTHY (never-failed) path — a
    // failure here would mean the fixture accidentally reproduces the
    // OTHER scenario instead.
    assert!(
        !session.stderr().contains("deferred dev bundle failed"),
        "the healthy-path fixture must not fail its deferred bundle — that would \
         accidentally exercise the cold-bootstrap-recovery scenario instead\n{}",
        session.logs(),
    );

    // THE POSITIVE HALF — the server has recovered: a fresh manual GET now
    // returns 200 with the marker.
    poll_until_contains(
        &client,
        &index_url,
        HEALTHY_MARKER,
        CONTENT_DEADLINE,
        "healthy: server recovered",
        &session,
    )
    .await;

    // THE TAB-FACING ASSERTION (issue #1826) — the tab subscribed BEFORE
    // the publish receives a `page` reload event. Without the shared
    // `ssr_routes_published` bit this stream stays silent forever: the
    // SSR-only project has zero SSG routes for `mark_all_routes_stale` to
    // put in `pages_stale`, so `outcome_to_events` had nothing to gate on.
    match next_sse_event_name(sse, RELOAD_DEADLINE).await {
        Ok(Some(name)) => assert_eq!(
            name,
            "page",
            "the healthy deferred publish must broadcast a full-page reload, not \
             {name:?}\n{}",
            session.logs(),
        ),
        Ok(None) => panic!(
            "no SSE event reached the tab subscribed before the healthy deferred publish \
             within {}s, even though the server already serves 200 — issue #1826 has \
             regressed on the healthy path (the shared ssr_routes_published signal is not \
             reaching outcome_to_events).\n{}",
            RELOAD_DEADLINE.as_secs(),
            session.logs(),
        ),
        Err(e) => panic!("reading the SSE stream failed: {e:#}\n{}", session.logs()),
    }
}

// ---------------------------------------------------------------------------
// SCENARIO 2 — cold-bootstrap recovery (#1809)
// ---------------------------------------------------------------------------

/// An SSR-only project whose deferred boot bundle genuinely FAILS once,
/// then recovers on a later successful publish, tells an already-open tab
/// to reload — the gap #1809's recovery mechanism was supposed to close but
/// couldn't, because it reuses the same SSG-table-only
/// `mark_all_routes_stale`. It now raises the shared
/// `ssr_routes_published` bit through the same seam the healthy path uses,
/// so the two heal identically rather than one healing better.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: esbuild — cargo test -p zfb --test ssr_reload_self_heal_e2e \
            -- --ignored --exact ssr_only_cold_bootstrap_recovery_reloads_open_tab \
            (ZFB_ESBUILD_BIN or the staged crates/zfb/binaries/esbuild slot). GREEN as \
            of issue #2002 — see epic #1999 / issue #1826 / issue #1809."]
async fn ssr_only_cold_bootstrap_recovery_reloads_open_tab() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[ssr_reload_self_heal] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("ssr-reload-self-heal recovery fixture tempdir");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    // The page is valid from the start; its imported module is genuinely
    // syntax-broken so the deferred boot bundle still fails for real.
    write_ssr_only_fixture(&root, &recovery_fixed_page_source());
    let trigger_path = root.join("src/recovery-trigger.ts");
    fs::create_dir_all(trigger_path.parent().expect("trigger parent")).expect("create src/");
    fs::write(&trigger_path, "export const trigger = ;\n").expect("write recovery trigger module");

    let mut session = spawn_dev(&root, &esbuild, &[("ZFB_DEV_BOOT_LAZY", "cold")]);
    let Some(port) = wait_for_ready(&mut session).await else {
        return; // environmental skip
    };
    let base = format!("http://localhost:{port}");
    let index_url = format!("{base}/");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    // Proof of a REAL failure: `deferred_bundle_failure_message`'s exact
    // Cold wording, not an assumption that the broken module did anything.
    wait_for_log_line(
        &session,
        "deferred dev bundle failed — cold-lazy (ZFB_DEV_BOOT_LAZY=cold) has no",
        "recovery: boot bundle failure",
    )
    .await;
    let pending = readiness_json(&client, &base).await;
    assert_eq!(
        pending["ready"], false,
        "a failed deferred Cold bundle must not claim publication readiness"
    );
    assert_eq!(pending["documents"], "pending");

    // Subscribe to the livereload stream AFTER the failure — this
    // simulates a tab that has been sitting on the dev 404 body since
    // boot, the whole time the project was broken. Uses the dedicated
    // no-total-timeout SSE opener.
    let sse = subscribe_sse(&base).await;

    // Sanity: the route must not yet serve the marker (still broken).
    let (pre_status, pre_body) = poll_get(&client, &index_url).await;
    assert!(
        pre_status != 200 || !pre_body.contains(RECOVERY_MARKER),
        "GET {index_url} unexpectedly already served the marker before the source was \
         fixed.\nstatus {pre_status}, body:\n{pre_body}\n{}",
        session.logs(),
    );

    // THE FIX — a renderer-relevant edit of the imported module retries the
    // bundle via the watcher's `reload_renderer` closure. macOS may report
    // this edit as Created or Modified; either classification reloads the
    // renderer, but neither can enter page discovery and eagerly publish
    // this SSR-only route before the readiness boundary is observed.
    fs::write(&trigger_path, "export const trigger = 'fixed';\n")
        .expect("edit recovery trigger module");

    // Deterministic recovery signal: `recover_cold_bootstrap_after_publish`
    // is the ONLY caller of this exact info line
    // (`crates/zfb/src/commands/dev.rs`, the shared publish success path).
    wait_for_log_line(
        &session,
        "cold-lazy bootstrap recovered",
        "recovery: latch fired",
    )
    .await;

    let recovered_readiness = wait_for_publication_ready(&client, &base, &session).await;
    assert_eq!(
        recovered_readiness["documents"],
        "ready_on_request",
        "a complete SSR-only route-table refresh is the repaired document boundary\n{}",
        session.logs(),
    );
    assert!(
        recovered_readiness["generation"].as_u64().unwrap_or(0) > 0,
        "Cold recovery must commit a new publication generation: {recovered_readiness}"
    );

    // THE POSITIVE HALF — the server has recovered: a fresh manual GET now
    // returns 200 with the marker.
    poll_until_contains(
        &client,
        &index_url,
        RECOVERY_MARKER,
        CONTENT_DEADLINE,
        "recovery: server recovered",
        &session,
    )
    .await;

    // THE TAB-FACING ASSERTION (issue #1826) — the tab subscribed before
    // the fix, which has been sitting on the dev 404 body since boot,
    // receives a `page` reload event once the recovery latch fires. This is
    // the SAME assertion the healthy scenario above makes, against the SAME
    // shared signal: the two recovery paths are not allowed to diverge.
    match next_sse_event_name(sse, RELOAD_DEADLINE).await {
        Ok(Some(name)) => assert_eq!(
            name,
            "page",
            "the cold-bootstrap recovery must broadcast a full-page reload, not \
             {name:?}\n{}",
            session.logs(),
        ),
        Ok(None) => panic!(
            "no SSE event reached the tab subscribed before the cold-bootstrap-recovery \
             publish within {}s, even though the recovery latch fired and the server \
             already serves 200 — issue #1826 has regressed on the recovery path.\n{}",
            RELOAD_DEADLINE.as_secs(),
            session.logs(),
        ),
        Err(e) => panic!("reading the SSE stream failed: {e:#}\n{}", session.logs()),
    }
}
