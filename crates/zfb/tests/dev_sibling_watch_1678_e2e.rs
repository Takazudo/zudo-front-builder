//! Level-4 acceptance gate for issue #1678 (epic #1670) — the dev watcher's
//! DYNAMIC workspace-sibling watch registration.
//!
//! Waves 1-4 taught `zfb build`/`zfb dev` to STAGE and bundle a claimed
//! pnpm-workspace sub-package host's SIBLING source reached through a `?raw`
//! import (and a project-local module worker whose own graph reaches a sibling
//! `?raw`). This test proves the DEV loop closes the circle: editing that
//! sibling source refreshes the served bundle without a `zfb dev` restart.
//!
//! The sibling directories are not among the project-relative
//! `DEFAULT_WATCH_ROOTS` — they live OUTSIDE the project root — so the FS-event
//! source is the dev watcher's dynamic-dependency channel: the parents of the
//! discovered `?raw`/module-worker targets are registered restart-free, on the
//! tick that discovers them, and surfaced (under `ZFB_DEV_TIMING`) as a
//! `watch-extra registered: <dir>` line. That line is the deterministic signal
//! the "sibling directory is now watched" wait keys on — the deflaking recipe's
//! Step-5 escalation, so the test never sleeps on an arbitrary duration.
//!
//! Scenarios (one dev session, to amortise the V8 + esbuild boot):
//! - **A** — editing a BOOT-discovered sibling `?raw` file refreshes the
//!   client bundle.
//! - **B** — editing a BOOT-discovered sibling MODULE-WORKER `?raw` dependency
//!   refreshes the worker companion.
//! - **C** — an in-project edit that introduces a NEW import to a brand-new
//!   sibling package (never watched at boot) makes that sibling directory
//!   watched (asserted on the `watch-extra registered:` signal), after which
//!   editing the new sibling file refreshes the bundle.
//! - **D** — (issue #1711, Sibling Invalidation epic #1709 confirm pass)
//!   editing a BOOT-discovered sibling PLAIN module (`sub/shared/plain.ts`,
//!   reached by an ordinary import — neither `?raw` nor a worker dependency)
//!   refreshes the served client bundle restart-free, proving the
//!   `client_script_siblings` registry threaded end-to-end in #1710 closes
//!   the dev loop for this third sibling shape too.
//!
//! `#[ignore]`d (`heavy:`): it spawns a real `zfb dev --port 0` with embedded
//! V8 + a real esbuild subprocess, polls it over HTTP, and is serialized with
//! the other heavy zfb process-spawning binaries by the nextest `e2e-heavy`
//! test-group. Run locally with
//! `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb --test dev_sibling_watch_1678_e2e -- --ignored`.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const BOOT_DEADLINE: Duration = Duration::from_secs(120);
// Boot CONTENT polls happen after the ready banner, when the eager boot bundle
// is already on disk — seconds, not the full boot budget. Kept smaller than
// BOOT_DEADLINE so the worst-case cumulative of every deadline in this test
// (~450s) stays clear of the nextest `e2e-heavy` 600s terminate-after, leaving
// margin for the diagnostic panic to print before a SIGKILL.
const BOOT_CONTENT_DEADLINE: Duration = Duration::from_secs(60);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);
const SIGNAL_DEADLINE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// Serialize with sibling heavy e2e binaries at the process level, and with
// this binary's own (single) test at the in-binary level.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-sibling-watch")
}

/// Recursive copy skipping generated/installed trees.
fn copy_fixture(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("node_modules" | "dist" | ".zfb-build")) {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_fixture(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

struct DevServerGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl DevServerGuard {
    fn try_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("poll `zfb dev` child")
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
    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            fs::read_to_string(&self.stdout_path).unwrap_or_default(),
            fs::read_to_string(&self.stderr_path).unwrap_or_default(),
        )
    }

    fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }
}

/// Spawn `zfb dev --port 0` in its own process group with `ZFB_DEV_TIMING=1`
/// (so the `watch-extra registered:` signal is emitted) and logs captured.
fn spawn_dev(root: &Path, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout = fs::File::create(&stdout_path).expect("create dev stdout log");
    let stderr = fs::File::create(&stderr_path).expect("create dev stderr log");
    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env("ZFB_DEV_TIMING", "1")
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    command.process_group(0);
    let child = command.spawn().expect("spawn `zfb dev --port 0`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = token.find(':') {
            let digits = token[colon + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

/// Returns `None` when `zfb dev` exited with a recognized environmental skip
/// indicator (no embedded V8 / no esbuild); otherwise the bound port.
async fn wait_for_ready(session: &mut DevSession) -> Option<u16> {
    let started = Instant::now();
    loop {
        if let Some(status) = session.guard.try_status() {
            let logs = session.logs();
            if logs.contains("embed_v8") || logs.contains("no esbuild") {
                eprintln!(
                    "[dev_sibling_watch_1678] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild unavailable); skipping test.\n{logs}"
                );
                return None;
            }
            panic!("`zfb dev` exited before readiness with {status:?}\n{logs}");
        }
        if let Some(port) =
            parse_ready_port(&fs::read_to_string(&session.stdout_path).unwrap_or_default())
        {
            assert_ne!(port, 0, "ready banner reported literal port 0");
            return Some(port);
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "`zfb dev --port 0` did not become ready in {}s\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `url` until it returns 200 with `marker` in the body, keyed on the
/// served-output invariant (never a fixed sleep). Returns the matching body.
async fn poll_body(
    client: &reqwest::Client,
    url: &str,
    marker: &str,
    label: &str,
    deadline: Duration,
    session: &DevSession,
) -> String {
    let started = Instant::now();
    let mut last = String::from("no response");
    while started.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if status.as_u16() == 200 && body.contains(marker) {
                    return body;
                }
                last = format!("status={status}");
            }
            Err(error) => last = format!("request failed: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "{label}: {url} did not return 200 with {marker:?} in {}s; last={last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

/// Wait until a `watch-extra registered:` line whose directory ends with
/// `dir_suffix` appears in the dev server's stderr — the deterministic signal
/// that the sibling directory has entered the watch set.
async fn wait_for_watch_extra(session: &DevSession, dir_suffix: &str) {
    let started = Instant::now();
    while started.elapsed() < SIGNAL_DEADLINE {
        if session.stderr().lines().any(|line| {
            line.contains("watch-extra registered:") && line.trim_end().ends_with(dir_suffix)
        }) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "no `watch-extra registered:` line ending in {dir_suffix:?} within {}s\n{}",
        SIGNAL_DEADLINE.as_secs(),
        session.logs(),
    );
}

/// Rewrite `path` with `contents` on a bounded cadence while polling `url` for
/// `marker`. A newly-registered non-recursive watch has a brief FSEvents
/// activation window on macOS; re-issuing the (idempotent) edit until the
/// served invariant flips absorbs that window deterministically, without a
/// fixed sleep. Panics on `deadline`.
async fn edit_until_served(
    client: &reqwest::Client,
    path: &Path,
    contents: &str,
    url: &str,
    marker: &str,
    label: &str,
    session: &DevSession,
) {
    let started = Instant::now();
    let mut last_write = Instant::now() - Duration::from_secs(1);
    while started.elapsed() < SCENARIO_DEADLINE {
        if last_write.elapsed() >= Duration::from_millis(700) {
            fs::write(path, contents).unwrap_or_else(|e| {
                panic!("edit {}: {e}", path.display());
            });
            last_write = Instant::now();
        }
        if let Ok(response) = client.get(url).send().await {
            if response.status().as_u16() == 200
                && response.text().await.unwrap_or_default().contains(marker)
            {
                return;
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "{label}: {url} never served {marker:?} after re-editing {} within {}s\n{}",
        path.display(),
        SCENARIO_DEADLINE.as_secs(),
        session.logs(),
    );
}

fn worker_companion_filename(host_root: &Path) -> String {
    zfb_types::module_worker_filename(host_root, &host_root.join("src/entry.worker.ts"))
        .expect("derive module-worker companion filename for src/entry.worker.ts")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_dev_watches_workspace_sibling_raw_and_worker_sources() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_sibling_watch_1678] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let workspace = tempfile::tempdir().expect("dev-sibling-watch fixture tempdir");
    copy_fixture(&fixture_dir(), workspace.path()).expect("copy dev-sibling-watch fixture");
    let host_root = workspace.path().join("sub/host");
    let shared_dir = workspace.path().join("sub/shared");

    // Scenario D (issue #1711) needs COPY mode for the preprocess stage, not
    // the default symlink mode: a sibling PLAIN module is resolved directly
    // by esbuild's normal module graph (unlike `?raw`/worker deps, which are
    // rewritten into generated in-place modules and never hit esbuild's own
    // symlink resolution). Real esbuild's `--preserve-symlinks` does not
    // protect a plain relative-import symlink the way it protects a
    // `node_modules`-shaped one, so in symlink mode the stage-escape audit
    // (#1705) rejects the resolved real path as an escape. `copy_mode` in
    // `stage_client_script_preprocessing` (crates/zfb/src/commands/build.rs)
    // requires BOTH a discoverable `node_modules` dir and a tsconfig `paths`
    // map in scope — the `sub/host/tsconfig.json` fixture already carries an
    // (otherwise unused) `paths` entry for this; this empty dir is the other
    // half. `copy_fixture` above intentionally skips copying `node_modules`,
    // so it must be created here rather than checked into the fixture.
    std::fs::create_dir_all(workspace.path().join("node_modules"))
        .expect("create workspace node_modules dir to force copy-mode staging");

    let mut session = spawn_dev(&host_root, &esbuild);
    let Some(port) = wait_for_ready(&mut session).await else {
        return; // environmental skip (no V8/esbuild)
    };
    let origin = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client");

    let client_url = format!("{origin}/assets/client/entry.js");
    let worker_url = format!(
        "{origin}/assets/client/{}",
        worker_companion_filename(&host_root)
    );

    // Boot state: both sibling payloads are inlined into the served bundles,
    // with no `?raw` specifier leaking through.
    let boot_entry = poll_body(
        &client,
        &client_url,
        "ZFB_SIBLING_PANEL_RAW_PAYLOAD_V1",
        "boot client entry inlines the sibling ?raw",
        BOOT_CONTENT_DEADLINE,
        &session,
    )
    .await;
    assert!(
        !boot_entry.contains("?raw"),
        "served client entry must not leak a `?raw` specifier\n{}",
        session.logs(),
    );
    // Scenario D's boot-discovered sibling PLAIN module: its exported string
    // constant is inlined into the bundled client entry alongside the `?raw`
    // payload above (proving `graph.modules` — not just the raw/worker edges
    // — feeds the client_script_siblings closure staged for this entry).
    assert!(
        boot_entry.contains("ZFB_SIBLING_PLAIN_PAYLOAD_V1"),
        "boot client entry must inline the sibling plain module's export\n{}",
        session.logs(),
    );
    poll_body(
        &client,
        &worker_url,
        "ZFB_SIBLING_WORKER_RAW_PAYLOAD_V1",
        "boot worker companion inlines the sibling ?raw",
        BOOT_CONTENT_DEADLINE,
        &session,
    )
    .await;

    // Scenario A — edit the boot-discovered sibling `?raw` file. The sibling
    // directory is watched from boot, but a freshly-registered non-recursive
    // FSEvents watch has a brief activation window on macOS; re-issuing the
    // (idempotent) edit until the served invariant flips absorbs it
    // deterministically, keyed on the served output — never a fixed sleep.
    edit_until_served(
        &client,
        &shared_dir.join("panel.frag"),
        "ZFB_SIBLING_PANEL_RAW_PAYLOAD_V2\nedited sibling raw payload\n",
        &client_url,
        "ZFB_SIBLING_PANEL_RAW_PAYLOAD_V2",
        "scenario A: sibling ?raw edit refreshes the client bundle",
        &session,
    )
    .await;

    // Scenario B — edit the boot-discovered sibling module-worker `?raw` dep.
    edit_until_served(
        &client,
        &shared_dir.join("worker-panel.frag"),
        "ZFB_SIBLING_WORKER_RAW_PAYLOAD_V2\nedited sibling worker payload\n",
        &worker_url,
        "ZFB_SIBLING_WORKER_RAW_PAYLOAD_V2",
        "scenario B: sibling worker ?raw edit refreshes the worker companion",
        &session,
    )
    .await;

    // Scenario C — introduce a NEW sibling package via an in-project edit, then
    // prove the newly-introduced sibling becomes watched (no restart) and its
    // edits invalidate.
    let late_dir = workspace.path().join("sub/late");
    fs::create_dir_all(&late_dir).expect("create late sibling package dir");
    fs::write(late_dir.join("package.json"), "{\n  \"name\": \"@zfb-test/sibling-late\",\n  \"private\": true,\n  \"type\": \"module\"\n}\n")
        .expect("write late sibling package.json");
    fs::write(
        late_dir.join("late.frag"),
        "ZFB_LATE_PANEL_RAW_PAYLOAD_V1\n",
    )
    .expect("seed late sibling frag");

    // The in-project edit that introduces the import (client entry lives under
    // the watched `src/` root, so this edit alone fires a tick).
    let client_entry_src = host_root.join("src/entry.client.ts");
    let mut entry_src = fs::read_to_string(&client_entry_src).expect("read entry.client.ts");
    entry_src.push_str(
        "\nimport latePanel from \"../../late/late.frag?raw\";\n\
         console.info(\"ZFB_LATE_MARKER\", latePanel);\n",
    );
    fs::write(&client_entry_src, &entry_src).expect("append late import to entry.client.ts");

    // The rebuild that discovers the new import bundles the late payload...
    poll_body(
        &client,
        &client_url,
        "ZFB_LATE_PANEL_RAW_PAYLOAD_V1",
        "scenario C: the newly-introduced sibling import is bundled",
        SCENARIO_DEADLINE,
        &session,
    )
    .await;
    // ...and registers the late sibling directory as a dynamic watch, which the
    // orchestrator surfaces as the observable signal we key the next edit on.
    wait_for_watch_extra(&session, "sub/late").await;

    // Editing the newly-watched sibling now refreshes the bundle, restart-free.
    edit_until_served(
        &client,
        &late_dir.join("late.frag"),
        "ZFB_LATE_PANEL_RAW_PAYLOAD_V2_EDITED\n",
        &client_url,
        "ZFB_LATE_PANEL_RAW_PAYLOAD_V2_EDITED",
        "scenario C: editing the newly-watched sibling refreshes the client bundle",
        &session,
    )
    .await;

    // Scenario D (issue #1711) — edit the BOOT-discovered sibling PLAIN
    // module (`sub/shared/plain.ts`, a committed fixture file imported by
    // `entry.client.ts` since boot, same as the `?raw` sibling in Scenario A
    // — NOT introduced after boot the way Scenario C's `late` package is).
    // The sibling directory is already watched from boot for the same reason
    // Scenario A's is; re-issuing the edit absorbs the same FSEvents
    // activation window `edit_until_served` already accounts for.
    edit_until_served(
        &client,
        &shared_dir.join("plain.ts"),
        "export const plainMarker = \"ZFB_SIBLING_PLAIN_PAYLOAD_V2_EDITED\";\n",
        &client_url,
        "ZFB_SIBLING_PLAIN_PAYLOAD_V2_EDITED",
        "scenario D: sibling plain-module edit refreshes the client bundle",
        &session,
    )
    .await;

    session.guard.child.try_wait().ok();
}
