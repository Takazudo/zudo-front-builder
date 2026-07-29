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
//! path as a `watchFiles` entry. `pages/index.tsx` imports `virtual:note`
//! and renders its value, and is `prerender = false` (SSR-only) — see the
//! next section for why that is load-bearing, not incidental.
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
//! ## Why the fixture page is SSR-only (`prerender = false`) — a real finding
//!
//! The first draft of this test used an ordinary prerendered/SSG page and
//! assumed (from `orchestrator.rs`'s `PreTickRefreshHook` doc comment,
//! "typically `PageSelection::All` … since watch files usually sit outside
//! the source roots") that a plugin watch-file edit would mark that page
//! stale via the `External` classification arm. **That assumption was
//! empirically wrong for THIS fixture's exact path shape**, and matters
//! enough to record here rather than silently work around:
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
//! dependency graph could ever see — so **zero pages are even considered
//! by this tick's lazy-render callback**, confirmed directly: a run with
//! `ZFB_DEV_TIMING=1` shows `stale probe: drained pages_stale=0` for the
//! `note.txt:Modified` tick, meaning `lazy_render_tick`'s per-page loop ran
//! over an EMPTY `pages` slice. A prerendered page's already-written
//! `dev-pages/index.html` is therefore NEVER invalidated by this edit, and
//! a subsequent `GET /` just re-serves the stale on-disk file forever —
//! **reproduced reliably (3/3 runs) against the fully-merged, unmodified
//! base** before this was traced to its cause.
//!
//! What DOES fire, unconditionally whenever
//! `GranularityPolicy::is_plugin_watch_target` matches (`orchestrator.rs`,
//! issue #2169's own addition, independent of `PathClass`): `plan.mark_css()`
//! / `mark_islands()` / `mark_client_scripts()` / `mark_ssr_reload_needed()`.
//! The CSS flag is what produces the SSE event this test observes (`css`,
//! not `page` — see the assertion below); the SSR-reload flag reloads the
//! V8 host against a freshly-rebuilt bundle, but for a PRERENDERED page
//! that reload has no consumer to serve it to, since nothing re-renders
//! that page's committed HTML file.
//!
//! An **SSR-only** page (`prerender = false`) sidesteps this entirely: such
//! a route has no `dev-pages/*.html` file and no staleness bookkeeping at
//! all — the renderer executes the page function FRESH on every single
//! request, always against whatever bundle is CURRENTLY loaded. So as long
//! as (a) the bundle gets rebuilt with the refreshed virtual-module source
//! (guaranteed by `mark_ssr_reload_needed` triggering a rebuild) and (b)
//! the refresh actually landed in the shared `PluginVirtualModuleStore`
//! before that rebuild reads it (issue #2169's whole point), the very next
//! `GET /` is guaranteed fresh — independent of `pages_stale`/lazy-render
//! narrowing altogether. Confirmed reliably (5/5 runs) once the fixture
//! page was changed to SSR-only, and confirmed revert-proof (see below).
//!
//! **This is a real, separate finding from the epic's own scope** — a
//! plugin watch-file living inside the project root under an unrecognized
//! directory name does not refresh a PRERENDERED page's served content
//! (only CSS/islands/client-scripts consumers and SSR-only routes benefit)
//! — reported to the team lead alongside this test rather than silently
//! routed around. It does not block this confirm-e2e, which targets the
//! shape the feature demonstrably supports end-to-end.
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
    locate_esbuild, next_sse_event_name, watcher_live_handshake, zfb_binary, CrossBinaryE2eLock,
    HandshakeOpts,
};

/// Serializes the spawning test in this file against any other test in the
/// SAME binary (there is only one today) — matches every sibling dev e2e's
/// `SERIAL` pattern so a future second test here doesn't race this one for
/// CPU/memory.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const OVERALL_DEADLINE: Duration = Duration::from_secs(180);
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
async fn subscribe_sse(client: &reqwest::Client, base: &str) -> reqwest::Response {
    let resp = client
        .get(format!("{base}/__zfb/reload"))
        .send()
        .await
        .expect("subscribe to /__zfb/reload");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "SSE endpoint /__zfb/reload must answer 200"
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
    // Confirm the boot-rendered page serves the virtual module's INITIAL
    // content before touching it.
    // ------------------------------------------------------------------
    poll_until_response_contains(
        &client,
        &format!("{base}/"),
        "V1-NOTE-CONTENT",
        "boot: initial virtual-module content",
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
    let sse = subscribe_sse(&client, &base).await;
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
        "entry rerender after the note.txt edit",
        &session,
    )
    .await;

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
