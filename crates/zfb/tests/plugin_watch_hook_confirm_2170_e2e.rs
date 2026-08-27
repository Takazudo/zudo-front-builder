//! Confirm e2e for the "Plugin Watch Hook" epic (#2166, sub-issue #2170):
//! a watched-file edit refreshes the served content of the virtual module
//! that reads it, end to end — `addVirtualModule(..., { watchFiles })`
//! registration (#2167), the shared live-refresh state (#2168), and the
//! orchestrator's pre-tick refresh hook (#2169), all exercised together
//! against a real `zfb dev` process. No prior test in this crate spawns a
//! real dev server and edits a file a plugin declared via `watchFiles` —
//! the crate-level unit tests for #2167/#2168/#2169 stub the plugin host
//! and the orchestrator directly (see `zfb-build`'s `policy.rs`,
//! `plugin_refresh.rs`, `orchestrator.rs` test modules), so a regression in
//! how those three pieces wire together end-to-end would pass every one of
//! them silently.
//!
//! ## Fixture (`tests/fixtures/plugin-watch-hook-confirm/`)
//!
//! `preset.mjs` registers `virtual:note`, whose loader reads
//! `plugin-watched/note.txt` directly via `node:fs` — a read the dev
//! bundler's static-import scan cannot see on its own — and declares that
//! path as a `watchFiles` entry. TWO pages import `virtual:note` and
//! render its value, deliberately covering both serving regimes:
//!
//! - `pages/index.tsx` — `prerender = false` (SSR-only). Rendered fresh on
//!   every request against whatever bundle is currently loaded.
//! - `pages/prerendered.tsx` — PRERENDERED (the default). Its HTML is
//!   committed to `.zfb-build/dev-pages/` and re-served from disk until a
//!   tick marks the page stale.
//!
//! The second page is not decoration: it is the only one of the two that
//! can fail when page INVALIDATION regresses, and it was added by issue
//! #2181 after the finding recorded in the next section. See that section
//! for why the first draft of this test had only the SSR-only page.
//!
//! `plugin-watched/` is deliberately a project-root child directory that is
//! NOT one of `crates/zfb/src/commands/dev.rs`'s `DEFAULT_WATCH_ROOTS`
//! (`pages`, `content`, `components`, `layouts`, `styles`, `data`, `src`,
//! plus the `zfb.config.*` files themselves), is not `public/`, and is not
//! claimed by any other dynamic-watch channel (CSS mirror roots, client
//! script siblings, islands/worker helpers). Every one of those boot watch
//! roots has NO extension filter, so a fixture using a path already inside
//! one of them (e.g. `data/note.txt`) would already produce a rebuild on
//! edit even with #2167/#2168/#2169 fully reverted, and would prove nothing
//! about the new plugin watch-file registration this epic added.
//!
//! ## The prerendered-page gap this test originally documented (issue #2181)
//!
//! The first draft of this test used an ordinary prerendered/SSG page and
//! assumed (from `orchestrator.rs`'s `PreTickRefreshHook` doc comment,
//! "typically `PageSelection::All` … since watch files usually sit outside
//! the source roots") that a plugin watch-file edit would mark that page
//! stale via the `External` classification arm. **That assumption was
//! empirically wrong for THIS fixture's exact path shape**, so the draft
//! was narrowed to an SSR-only page and the gap reported separately as
//! issue #2181. #2181 has since been FIXED, and this section records the
//! trail because the fix's correctness rests on it:
//!
//! `plugin-watched/note.txt` is a project-root-RELATIVE path (it does NOT
//! fail `strip_prefix(project_root)`) with no recognized top segment
//! (`pages`/`content`/`styles`/`data`/`public`/`components`/`layouts`/
//! `lib`/`src`), so `zfb-build`'s `classify_change_with_content_roots`
//! (`policy.rs`) falls through its root-segment walk to plain extension
//! sniffing, and `.txt` is not on that whitelist — the path classifies as
//! `PathClass::Unclassified`, NOT `PathClass::External` (`External` is
//! reserved for paths that FAIL `strip_prefix(project_root)` entirely —
//! the classic out-of-project `extraWatchPaths` shape the doc comment
//! above was describing). `Unclassified`'s own arm in `orchestrator.rs`'s
//! `plan_for_changes` only consults `graph.dirty_pages(&path)` — empty
//! here, since nothing imports `note.txt` via a real ESM specifier the
//! dependency graph could ever see — so **zero pages were even considered
//! by that tick's lazy-render callback**, confirmed directly at the time: a
//! run with `ZFB_DEV_TIMING=1` showed `stale probe: drained pages_stale=0`
//! for the `note.txt:Modified` tick, meaning `lazy_render_tick`'s per-page
//! loop ran over an EMPTY `pages` slice. A prerendered page's
//! already-written `dev-pages/*.html` was therefore never invalidated by
//! this edit, and subsequent requests re-served the stale on-disk file
//! forever — **reproduced reliably (3/3 runs)** before it was traced.
//!
//! The #2181 fix marks `PageSelection::All` inside the same
//! `is_plugin_watch_target` block that already set the consumer flags
//! (`orchestrator.rs`, both in `plan_for_changes` and in
//! `tick_with_kinds`'s removed-path fold), so page invalidation no longer
//! depends on how the watch file happens to classify. That is why
//! `pages/prerendered.tsx` exists in this fixture and is asserted below
//! ALONGSIDE the SSR-only page rather than replacing it — the two pages
//! fail for different reasons, and keeping both means a failure names
//! which contract broke:
//!
//! - **`/` (SSR-only)** has no `dev-pages/*.html` file and no staleness
//!   bookkeeping at all — the renderer executes the page function FRESH on
//!   every request, always against whatever bundle is currently loaded. It
//!   therefore proves the STORE-refresh half (#2168/#2169: the loader was
//!   re-invoked and its output published before the rebuild read it) and
//!   is completely insensitive to page selection.
//! - **`/prerendered/`** is served from committed HTML, so it proves the
//!   INVALIDATION half (#2181) specifically: it can only go fresh if the
//!   tick actually selected it for re-render. Reverting #2181's
//!   `plan.mark_pages(PageSelection::All)` leaves `/` green and fails
//!   exactly this route — the same 3/3-reproducible stale-V1 symptom
//!   recorded above.
//!
//! What fires regardless of `PathClass`, whenever
//! `GranularityPolicy::is_plugin_watch_target` matches (`orchestrator.rs`,
//! issue #2169's own addition): `plan.mark_css()` / `mark_islands()` /
//! `mark_client_scripts()` / `mark_ssr_reload_needed()`. The CSS flag is
//! what produces the SSE event this test observes (`css`, not `page` — see
//! the assertion below).
//!
//! ## The content-freshness assertion (binding shape, per issue #2170)
//!
//! The test edits `plugin-watched/note.txt` from `V1-NOTE-CONTENT` to a
//! fresh `V2-NOTE-CONTENT-FRESH` marker and polls `GET /` for the NEW
//! marker to appear in the served HTML — content freshness, not merely
//! "a re-render happened" or "an SSE event fired". This is deliberately
//! revert-proof in both directions the epic's design calls out:
//!
//! - **watch registration reverted** (#2167 / `addVirtualModule`'s
//!   `watchFiles` option never populating `GranularityPolicy`'s
//!   `plugin_watch_files` registry): `note.txt` sits outside every default
//!   watch root and every other dynamic-watch registry, so with an empty
//!   registry `register_dynamic_dependency_watches` never adds it to the
//!   live watch set at all — the edit produces NO filesystem event the
//!   orchestrator ever sees, no SSE event of ANY name ever fires, and the
//!   freshness poll times out with the ORIGINAL `V1-NOTE-CONTENT` still
//!   being served.
//! - **invalidation/refresh reverted** (#2168's `PluginVirtualModuleStore` /
//!   #2169's pre-tick hook): the watch-file path IS still registered, so
//!   the edit still reaches the orchestrator and `is_plugin_watch_target`
//!   still matches — `mark_css`/`mark_ssr_reload_needed` still fire (an
//!   SSE `css` event still broadcasts, and the renderer still reloads) —
//!   but without the pre-tick hook re-invoking the loader and publishing
//!   its fresh output BEFORE that reload, the shared
//!   `PluginVirtualModuleStore` keeps serving the loader's boot-time memo
//!   (`V1-NOTE-CONTENT`) forever, so the reloaded renderer bundles the SAME
//!   stale source and the freshness poll still times out even though
//!   delivery visibly happened. **Verified empirically** (not just
//!   reasoned about): temporarily short-circuiting
//!   `PluginVirtualModuleStore::publish` to a no-op (`crates/zfb-build/src/plugin_refresh.rs`)
//!   reproduced exactly this — the `css` SSE event and the
//!   `pre-tick refresh completed ok=true` line both still appeared, yet the
//!   freshness poll timed out serving the original `V1-NOTE-CONTENT` —
//!   then restoring `publish` (confirmed via a clean `git diff`) returned
//!   the test to green.
//!
//! Both halves are pinned as SEPARATE assertions below (SOME SSE event
//! observed — proving delivery — THEN the freshness poll — proving
//! invalidation), so a failure names which half of the contract broke
//! rather than reporting an ambiguous single timeout.
//!
//! ## Diagnostics & Hygiene additions (issue #2374, epic #2368)
//!
//! The same scenario, in the same dev session, also confirms three more
//! contracts the Plugin Diagnostics & Hygiene epic added around this
//! plugin-host wiring — extending `preset.mjs` (see its own header
//! comment) rather than adding a second dev-server boot to this file:
//!
//! - **Log rendering (#2369).** `preset.mjs`'s `setup` hook calls
//!   `logger.info(...)` once at boot; the loader itself calls
//!   `console.log(...)` on every invocation (routed through #2373's global
//!   `console` redirection). Both must reach the captured terminal output
//!   formatted exactly like every other plugin log line —
//!   `` zfb info: [plugin:<name>] <message> `` (`plugin_runner.rs`'s
//!   `format_plugin_log_line`) — asserted right after the existing V1→V2
//!   freshness pass above, by which point both lines have necessarily been
//!   written.
//! - **Failed re-invoke surfaces exactly once (#2370).** Writing the
//!   `THROW-ON-REINVOKE` sentinel to `note.txt` makes the SAME loader
//!   throw on its next forced re-invoke. `dev.rs`'s
//!   `fmt_plugin_refresh_failures` is the single user-facing rendering
//!   site for this (`zfb-build`'s own `tracing::warn!` inside
//!   `plugin_refresh.rs::refresh` is invisible without a subscriber
//!   attached) — the test asserts its diagnostic line appears, and
//!   appears EXACTLY ONCE, naming both the plugin and the specifier.
//! - **Last-good serving + recovery.** While the diagnostic is showing,
//!   both routes must keep serving the last successfully published
//!   content (`V2-NOTE-CONTENT-FRESH`) — proving `PluginRefreshState`'s
//!   all-or-nothing atomicity (documented in `plugin_refresh.rs`) held
//!   under a real throw, not just the crate's own unit tests. Writing a
//!   valid value back to `note.txt` afterwards must then recover fresh
//!   content on both routes, and must NOT re-print the failure
//!   diagnostic — the orchestrator's pre-tick hook only ever fires for a
//!   tick whose changed paths touch a registered plugin watch file, and
//!   this fixture registers exactly one (`note.txt` itself), so nothing
//!   between the failing edit and the recovery edit can re-trigger it.
//!
//! ## Modeled on
//!
//! `crates/zfb/tests/dev_supervision_e2e.rs`'s spawn/boot helpers and its
//! use of the shared, condition-keyed
//! `zfb_test_utils::watcher_live_handshake` (promoted in #1338 out of four
//! inline duplicates — this file uses it directly rather than
//! reintroducing a fifth ad hoc copy the way `dev_serve_e2e.rs` and
//! `dev_content_reload_2063_e2e.rs`, both older than the promotion, still
//! do). The plugin-preset fixture shape (a `preset.mjs` registering
//! `addVirtualModule`, with a page importing the registered specifier and
//! rendering its exported value) mirrors
//! `dev_serve_injected_routes_e2e.rs`'s `virtual:preset-banner` fixture and
//! `dev_content_reload_2063_e2e.rs`'s injected-route preset family.
//!
//! ## Self-skip, not `#[ignore]` (per `crates/CLAUDE.md`'s taxonomy)
//!
//! Self-skips via `locate_esbuild()` like every other real `zfb dev` e2e in
//! this crate (`dev_serve_e2e`, `dev_content_reload_2063_e2e`,
//! `dev_supervision_e2e`, ...) — health.yml always stages a pinned esbuild,
//! so the self-skip is a local-dev convenience only, not a CI blocker; none
//! of the 5 `#[ignore]` taxonomy prefixes apply (rule 1, `env-gate:`,
//! doesn't fire because the runner CAN provide esbuild; rule 3, `heavy:`,
//! doesn't fire either — this file's single scenario measured well under
//! this crate's other un-ignored real `zfb dev` e2e budgets, see the
//! session log). This test therefore runs, and is required to pass, on
//! every T1 gate. It IS registered in `.config/nextest.toml`'s
//! `[test-groups.e2e-heavy]` (flock-adopting bucket) purely for CPU/memory
//! serialization against the other real `zfb dev`/`zfb build` processes —
//! orthogonal to `#[ignore]` status, per that file's own registration rule.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{
    locate_esbuild, next_sse_event_name, open_sse, watcher_live_handshake, zfb_binary,
    CrossBinaryE2eLock, HandshakeOpts,
};

/// Serializes the spawning test in this file against any other test in the
/// SAME binary (there is only one today) — matches every sibling dev e2e's
/// `SERIAL` pattern so a future second test here doesn't race this one for
/// CPU/memory.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

// Bumped from 180s (issue #2374): the scenario now runs two more
// watch-triggered edits (a failing re-invoke, then a recovery) each with
// their own bounded SSE/freshness/log polls on top of the original
// #2170 V1→V2 pass, inside the SAME dev session.
const OVERALL_DEADLINE: Duration = Duration::from_secs(240);
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
const SSE_DEADLINE: Duration = Duration::from_secs(30);
const FRESHNESS_DEADLINE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("plugin-watch-hook-confirm")
}

/// Recursive directory copy (same shape as every sibling dev e2e's
/// `copy_dir`).
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

/// Owns the spawned `zfb dev` process. Drop group-kills the entire process
/// group, so the dev server (and anything it spawned) is reaped on
/// success, panic, and watchdog-timeout paths alike.
struct DevServerGuard {
    child: std::process::Child,
    /// PGID == child PID (the child was spawned with `process_group(0)`).
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

/// Extract the port from the dev ready banner (same parser every sibling
/// dev e2e uses).
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

/// One spawned dev session: fixture root + process guard + log paths.
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

/// Spawn `zfb dev --port 0` over a fresh copy of the fixture, in its own
/// process group, with stdout/stderr captured to FILES (never pipes — a
/// piped child that outgrows the OS pipe buffer blocks on write and
/// masquerades as a hang, `build_terminates.rs` pattern).
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path) -> DevSession {
    // macOS /tmp is a symlink to /private/tmp; notify reports canonical
    // paths, and the plugin preset joins `projectRoot` (this canonical
    // path) with `plugin-watched/note.txt` — every path the dev process,
    // the plugin host, and this test compare must agree on the canonical
    // form (watch_add_confirm.rs pattern, reused by every sibling dev e2e).
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
        // Surfaces `[zfb-timing] tick(): kinds=[...]`,
        // `[zfb-timing] watch-extra registered: ...`, and
        // `[zfb-timing] plugin-refresh: pre-tick refresh completed ok=...`
        // on stderr (`crates/zfb-build/src/orchestrator.rs`) — the last of
        // these is this epic's own pre-tick-refresh signal (#2169),
        // printed only when a batch actually touches the plugin watch-file
        // set, so its presence directly pins the hook firing for THIS
        // edit rather than merely "some tick happened".
        .env("ZFB_DEV_TIMING", "1")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    // Strip any inherited lazy-render / boot switches (codex review lesson
    // from `dev_serve_e2e.rs`/`dev_supervision_e2e.rs`): a shell/CI
    // environment exporting one of these would silently change this
    // test's boot contract out from under it.
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    // New process group (PGID == child PID) so kill(-pgid, SIGKILL) reaps
    // the dev server plus any helper process it spawned.
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

/// Phases A-B: discover the ephemeral port from the ready banner, then
/// HTTP-poll `GET /` until 200. Returns `None` when the binary exited with
/// a recognized environmental skip indicator.
async fn boot_and_wait_ready(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
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
                    "[plugin_watch_hook_confirm_2170_e2e] `zfb dev` exited with a known-skip \
                     indicator (V8/esbuild unavailable); skipping test.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the ready \
                 banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "ready banner printed port 0 — the `--port 0` actual-bound-port contract \
                 regressed.\n{}",
                session.logs(),
            );
            break port;
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client");

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
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    Some((base, client))
}

/// Writes a fresh-named warmup content file into the fixture's watched
/// `content/posts/` directory — the same operation class
/// `watcher_live_handshake`'s docs call out as required (a brand-new
/// create, never a re-write of one path), and deliberately never
/// `plugin-watched/note.txt` — the file this test's own scenario edits.
fn write_warmup_marker(root: &Path, idx: u32) {
    let warmup = root.join(format!("content/posts/__warmup-{idx}.md"));
    let _ = fs::write(
        &warmup,
        format!("---\ntitle: warmup {idx}\n---\n\nwarmup body {idx}\n"),
    );
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
    while start.elapsed() < FRESHNESS_DEADLINE {
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
        "[{phase}] GET {url} did not serve {marker:?} within {}s. Last observation:\n{}\n{}",
        FRESHNESS_DEADLINE.as_secs(),
        last_observation,
        session.logs(),
    );
}

enum ScenarioOutcome {
    Completed,
    /// The binary exited with a known environmental skip indicator (no
    /// V8 / no esbuild) — skip without failing.
    Skipped,
}

async fn run_scenario() -> ScenarioOutcome {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[plugin_watch_hook_confirm_2170_e2e] no esbuild binary available; skipping. Set \
             ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return ScenarioOutcome::Skipped;
    };
    let tmp = tempfile::tempdir().expect("create tempdir for plugin-watch-hook-confirm fixture");
    let mut session = spawn_dev(&tmp, &esbuild);

    let Some((base, client)) = boot_and_wait_ready(&mut session).await else {
        return ScenarioOutcome::Skipped;
    };

    // ------------------------------------------------------------------
    // Watcher-live handshake: prove the watch stream is past its
    // FSEvents/inotify startup dead window using an UNRELATED warmup file
    // before touching `plugin-watched/note.txt`. Keyed on the
    // `ZFB_DEV_TIMING` `[zfb-timing] tick():` line (a synchronous, non-
    // blocking read of captured stderr — the shape
    // `watcher_live_handshake`'s `signal_seen` needs), not on an SSE read:
    // this handshake makes no assumption about how a plugin watch-file
    // edit classifies, since that classification is exactly what the
    // scenario below exists to prove.
    // ------------------------------------------------------------------
    let stdout_path = session.stdout_path.clone();
    let stderr_path = session.stderr_path.clone();
    let root = session.root.clone();
    let result = watcher_live_handshake(
        HandshakeOpts::new(HANDSHAKE_DEADLINE).with_marker_interval(Duration::from_millis(400)),
        {
            let root = root.clone();
            move |idx| write_warmup_marker(&root, idx)
        },
        move || dump_logs(&stdout_path, &stderr_path).contains("[zfb-timing] tick():"),
    )
    .await;
    assert!(
        result.live,
        "watcher never became live: no `[zfb-timing] tick():` line observed within {}s — {} \
         warmup markers written.\n{}",
        HANDSHAKE_DEADLINE.as_secs(),
        result.markers_written,
        session.logs(),
    );

    // ------------------------------------------------------------------
    // Confirm BOTH pages serve the virtual module's INITIAL content before
    // touching it — the SSR-only route and the prerendered one. Without
    // this baseline a post-edit freshness pass could not distinguish "the
    // page refreshed" from "the page never rendered the old value at all".
    // ------------------------------------------------------------------
    poll_until_response_contains(
        &client,
        &format!("{base}/"),
        "V1-NOTE-CONTENT",
        "boot: initial virtual-module content (SSR-only route)",
        &session,
    )
    .await;
    poll_until_response_contains(
        &client,
        &format!("{base}/prerendered/"),
        "V1-NOTE-CONTENT",
        "boot: initial virtual-module content (prerendered route)",
        &session,
    )
    .await;

    // ------------------------------------------------------------------
    // The scenario edit: subscribe to SSE FIRST, then rewrite
    // `plugin-watched/note.txt` — a path outside every default watch root,
    // watchable only because the plugin's `addVirtualModule(..., {
    // watchFiles })` registration (#2167) put it in
    // `GranularityPolicy::plugin_watch_files`.
    // ------------------------------------------------------------------
    let sse = subscribe_sse(&base).await;
    fs::write(
        session.root.join("plugin-watched/note.txt"),
        "V2-NOTE-CONTENT-FRESH\n",
    )
    .expect("edit plugin-watched/note.txt");

    let ev = next_sse_event_name(sse, SSE_DEADLINE)
        .await
        .expect("read SSE stream after the note.txt edit");
    // The observed event name here is `css`, not `page` — see this file's
    // header comment ("Why `css`, not `page`") for the classification
    // trail that explains it. Assert only that DELIVERY happened at all
    // (some event, not a timeout): if `watchFiles` registration (#2167)
    // is reverted, `note.txt` is watched by nothing, no tick ever
    // dispatches for this edit, and `next_sse_event_name` times out to
    // `None` — a clean, distinguishable "delivery never happened" signal
    // separate from the freshness poll below.
    assert!(
        ev.is_some(),
        "editing the plugin's `watchFiles`-registered note.txt must broadcast SOME SSE event \
         (observed to be `css` on this codebase revision) — if `watchFiles` registration \
         (#2167) is reverted this file is watched by nothing and no tick, and therefore no SSE \
         event, would ever fire for this edit; observed None (timed out after {}s).\n{}",
        SSE_DEADLINE.as_secs(),
        session.logs(),
    );

    // Auxiliary delivery evidence: the pre-tick refresh hook's OWN timing
    // line (issue #2169) only ever prints when `pre_tick_refresh_applies`
    // matched (i.e. `is_plugin_watch_target` — #2167/#2168's registration
    // — was true) AND a hook was installed (#2169) — its presence at all
    // is the meaningful signal, narrower than "some tick happened". The
    // trailing `ok=true` is NOT itself a per-loader success proof — the
    // hook closure `commands/dev.rs` installs always returns `Ok(())`
    // regardless of individual loader outcomes (those are logged
    // separately); the CONTENT freshness poll below is what actually
    // proves the refresh took effect. Bounded, condition-keyed poll of
    // the same captured stderr the handshake above already reads.
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if session
                .logs()
                .contains("[zfb-timing] plugin-refresh: pre-tick refresh completed ok=true")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "expected a `[zfb-timing] plugin-refresh: pre-tick refresh completed ok=true` \
                 line (issue #2169's pre-tick hook) after editing note.txt; it never appeared \
                 — the hook was never installed, or `is_plugin_watch_target` never matched this \
                 batch (the #2167/#2168 registration).\n{}",
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // ------------------------------------------------------------------
    // THE central assertion (issue #2170's binding shape): CONTENT
    // FRESHNESS, not just a re-render or an SSE event. Fails against a
    // reverted watch registration (no tick ever fires for this path, see
    // the SSE assertion above) AND against reverted invalidation (a tick
    // still fires and still reloads the renderer, but the
    // `PluginVirtualModuleStore` never published fresh content, so the
    // reloaded renderer bundles the SAME stale source and this request
    // keeps serving the boot-time `V1-NOTE-CONTENT` memo forever) — both
    // verified empirically, see the header comment above.
    // ------------------------------------------------------------------
    poll_until_response_contains(
        &client,
        &format!("{base}/"),
        "V2-NOTE-CONTENT-FRESH",
        "entry rerender after the note.txt edit (SSR-only route)",
        &session,
    )
    .await;

    // The #2181 half, asserted SECOND and separately so a failure names
    // which contract broke. The SSR-only route above cannot fail on page
    // selection (it has no committed HTML and no staleness bookkeeping);
    // this PRERENDERED route is served from `.zfb-build/dev-pages/` and
    // can only go fresh if the tick actually selected it for re-render —
    // which is exactly what `plan.mark_pages(PageSelection::All)` in
    // `orchestrator.rs`'s `is_plugin_watch_target` block provides. With
    // that line reverted, the route above stays green and THIS one times
    // out still serving `V1-NOTE-CONTENT`.
    poll_until_response_contains(
        &client,
        &format!("{base}/prerendered/"),
        "V2-NOTE-CONTENT-FRESH",
        "prerendered page invalidation after the note.txt edit (issue #2181)",
        &session,
    )
    .await;

    // ------------------------------------------------------------------
    // Issue #2374, assertion 1: the setup-time `logger.info` call and the
    // loader's own `console.log` call must both have reached the captured
    // terminal output by now — setup ran once at host boot (before the
    // ready banner), and the loader has been invoked at least once (the
    // initial load above, plus the forced re-invoke that produced
    // V2-NOTE-CONTENT-FRESH). Both are formatted exactly like every other
    // plugin log line: `plugin_runner.rs`'s `format_plugin_log_line`
    // ("zfb {level}: [plugin:{plugin}] {message}").
    // ------------------------------------------------------------------
    let logs_so_far = session.logs();
    assert!(
        logs_so_far.contains(
            "zfb info: [plugin:plugin-watch-hook-confirm-preset] plugin-watch-hook-confirm-preset: setup ran"
        ),
        "expected the plugin's setup-time `logger.info` line in the captured dev terminal \
         output (issue #2374) — the setup hook's log rendering may have regressed.\n{}",
        session.logs(),
    );
    assert!(
        logs_so_far.contains(
            "zfb info: [plugin:plugin-watch-hook-confirm-preset] plugin-watch-hook-confirm-preset: virtual:note loader read"
        ),
        "expected the virtual:note loader's `console.log` line — routed through issue #2373's \
         global console redirection — in the captured dev terminal output (issue #2374).\n{}",
        session.logs(),
    );

    // ------------------------------------------------------------------
    // Issue #2374, assertion 2: a loader that throws on a watch-triggered
    // re-invoke must surface a visible, single-owner error line naming
    // the plugin, while both routes keep serving the last successfully
    // published content. `THROW-ON-REINVOKE` is a sentinel this same
    // `virtual:note` loader recognizes (see preset.mjs) — never written
    // by the V1/V2 pass above, so this cannot be a false positive from
    // an unrelated failure.
    // ------------------------------------------------------------------
    const FAILURE_MARKER: &str = "plugin \"plugin-watch-hook-confirm-preset\" failed to reload \
                                   virtual module \"virtual:note\"";

    let sse_before_failure = subscribe_sse(&base).await;
    fs::write(
        session.root.join("plugin-watched/note.txt"),
        "THROW-ON-REINVOKE\n",
    )
    .expect("edit plugin-watched/note.txt to the throw sentinel");

    let ev = next_sse_event_name(sse_before_failure, SSE_DEADLINE)
        .await
        .expect("read SSE stream after the throw-sentinel edit");
    assert!(
        ev.is_some(),
        "editing note.txt to the throw sentinel must still broadcast SOME SSE event — \
         `is_plugin_watch_target` marks css/ssr-reload regardless of whether the pre-tick \
         refresh itself succeeds (timed out after {}s).\n{}",
        SSE_DEADLINE.as_secs(),
        session.logs(),
    );

    // Bounded, condition-keyed poll of captured stderr for the
    // single-owner failure line (dev.rs's `fmt_plugin_refresh_failures`,
    // rendered via `output::warn`).
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if session.logs().contains(FAILURE_MARKER) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "expected the single-owner plugin-refresh failure line ({FAILURE_MARKER:?}) \
                 after writing the throw sentinel to note.txt; it never appeared.\n{}",
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    let occurrences_after_failure = session.logs().matches(FAILURE_MARKER).count();
    assert_eq!(
        occurrences_after_failure,
        1,
        "the failed-reinvoke diagnostic must name the plugin EXACTLY ONCE (single-owner \
         rendering, issue #2374) — observed {occurrences_after_failure} occurrences.\n{}",
        session.logs(),
    );

    // Last-good serving: both routes must keep the last successfully
    // published content, never blank, never the throw sentinel.
    poll_until_response_contains(
        &client,
        &format!("{base}/"),
        "V2-NOTE-CONTENT-FRESH",
        "failed re-invoke: SSR-only route keeps serving the last-good content",
        &session,
    )
    .await;
    poll_until_response_contains(
        &client,
        &format!("{base}/prerendered/"),
        "V2-NOTE-CONTENT-FRESH",
        "failed re-invoke: prerendered route keeps serving the last-good content",
        &session,
    )
    .await;

    // Recovery: a subsequent successful re-invoke must restore fresh
    // content on both routes, and must NOT reprint the failure
    // diagnostic — the pre-tick hook only fires for a tick whose changed
    // paths touch a registered plugin watch file, and this fixture
    // registers exactly one (note.txt itself), which this edit is the
    // first to touch again since the failure above.
    let sse_recovery = subscribe_sse(&base).await;
    fs::write(
        session.root.join("plugin-watched/note.txt"),
        "V3-NOTE-CONTENT-RECOVERED\n",
    )
    .expect("edit plugin-watched/note.txt back to a valid value");

    let ev = next_sse_event_name(sse_recovery, SSE_DEADLINE)
        .await
        .expect("read SSE stream after the recovery edit");
    assert!(
        ev.is_some(),
        "the recovery edit must broadcast SOME SSE event (timed out after {}s).\n{}",
        SSE_DEADLINE.as_secs(),
        session.logs(),
    );

    poll_until_response_contains(
        &client,
        &format!("{base}/"),
        "V3-NOTE-CONTENT-RECOVERED",
        "recovery: SSR-only route serves fresh content after a successful re-invoke",
        &session,
    )
    .await;
    poll_until_response_contains(
        &client,
        &format!("{base}/prerendered/"),
        "V3-NOTE-CONTENT-RECOVERED",
        "recovery: prerendered route serves fresh content after a successful re-invoke",
        &session,
    )
    .await;

    let occurrences_after_recovery = session.logs().matches(FAILURE_MARKER).count();
    assert_eq!(
        occurrences_after_recovery,
        1,
        "the failed-reinvoke diagnostic must not reappear after a successful recovery edit — \
         observed {occurrences_after_recovery} occurrences.\n{}",
        session.logs(),
    );

    ScenarioOutcome::Completed
}

#[tokio::test(flavor = "multi_thread")]
async fn watched_file_edit_refreshes_served_virtual_module_content() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    match tokio::time::timeout(OVERALL_DEADLINE, run_scenario()).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] plugin_watch_hook_confirm_2170_e2e did not finish within {}s.",
            OVERALL_DEADLINE.as_secs(),
        ),
    }
}
