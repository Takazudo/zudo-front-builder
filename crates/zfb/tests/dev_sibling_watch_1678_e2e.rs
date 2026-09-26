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
//!
//! **Scenario E** (issue #1805, epic #1799 gap (a) confirm pass) lives in its
//! own test function below, `e2e_dev_sibling_tailwind_utility_class_refreshes_served_css`,
//! rather than as a fifth scenario in the function above: it needs the
//! Tailwind binary in addition to esbuild, and its sibling must be reached
//! ONLY through a tsconfig alias claim (`zfb_build::SiblingMirrorPlan`'s
//! claim source (b)) — never through a `?raw`/worker/plain-module import —
//! so the `css_mirror_roots` recursive-directory watch (#1801/#1802) is the
//! ONLY registry that can make its edit observable. Reusing this file's
//! `sub/shared` sibling would have defeated that: it is ALREADY a mirror root
//! (its tsconfig alias) as well as ALREADY registered on the file-parent
//! `watch_additional_files` channel (Scenarios A/B/D's imports), so an edit
//! there would keep refreshing even with the Wave-2 registration reverted. A
//! dedicated function also keeps its own independent nextest `e2e-heavy`
//! per-test 600s `slow-timeout` budget instead of sharing this function's
//! already-tight cumulative deadline total.
//!
//! **Issue #3163** (epic #3160, the confirm pass over #3161/#3162) adds five
//! `e2e_3163_*` functions at the end of this file, each over its own fixture:
//! an edit of a first-party workspace package that the SSR page imports by
//! name is served restart-free — for a root site, a nested site (including a
//! package first imported mid-session), a nested site whose compiled-JS-only
//! sibling exports into `dist/`, and a deferred Cold boot — and a restart
//! serves an edit made before the stop (#3155's report). Those packages are
//! reached by no client import and no tsconfig alias, so only the SSR
//! module-dependency registry (plus the boot-only #1284 D4 watch for an eager
//! nested host) can observe them.
//!
//! **Issue #3190** (epic #3187, #3181) adds two `e2e_3190_*` functions after
//! those: over the same root site, a SINGLE edit written the instant `ready`
//! is printed must be served, including when it lands before the watcher is
//! armed at all (the orchestrator's watch-arm reconcile recovers it).

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Locate a tailwindcss v4 binary for Scenario E, mirroring
/// `sibling_css_module_command_layer_build.rs`'s two env-gate tests'
/// resolution (`ZFB_TAILWIND_BIN` env var, else the workspace-staged
/// `crates/zfb/binaries/tailwindcss-v4` slot). Unlike
/// `zfb_test_utils::locate_esbuild` this has no pnpm-store/PATH fallback —
/// Scenario E is gated on the exact same slot the rest of this repo's
/// Tailwind env-gate tests rely on, so there is no reason to search further.
/// Returns `None` so the caller can skip cleanly on a machine that only
/// staged esbuild.
fn locate_tailwind() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_TAILWIND_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let slot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tailwindcss-v4");
    slot.is_file().then_some(slot)
}

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
// this binary's own tests (every `#[tokio::test]` fn below) at the
// in-binary level.
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
/// `tailwind` sets `ZFB_TAILWIND_BIN` for the child when `Some` (Scenario E);
/// scenarios A-D pass `None`, leaving the child's env byte-identical to
/// before issue #1805.
fn spawn_dev(root: &Path, esbuild: &Path, tailwind: Option<&Path>) -> DevSession {
    spawn_dev_with_env(root, esbuild, tailwind, &[], "")
}

/// [`spawn_dev`] plus extra child env vars (issue #3163's deferred-boot
/// scenario sets `ZFB_DEV_BOOT_LAZY=cold`) and a log-file tag, so a restarted
/// session over the same root keeps the first session's logs readable.
fn spawn_dev_with_env(
    root: &Path,
    esbuild: &Path,
    tailwind: Option<&Path>,
    extra_env: &[(&str, &str)],
    log_tag: &str,
) -> DevSession {
    let stdout_path = root.join(format!(".zfb-dev{log_tag}-stdout.log"));
    let stderr_path = root.join(format!(".zfb-dev{log_tag}-stderr.log"));
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
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(tailwind) = tailwind {
        command.env("ZFB_TAILWIND_BIN", tailwind);
    }
    command.envs(extra_env.iter().copied());
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

    let mut session = spawn_dev(&host_root, &esbuild, None);
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

// ---------------------------------------------------------------------------
// Scenario E (issue #1805, epic #1799 gap (a) confirm pass)
// ---------------------------------------------------------------------------

/// Build a fresh pnpm-workspace fixture for Scenario E: a HOST project
/// (`sub-packages/uhost`) reaching a SIBLING component
/// (`<ws_root>/lib/ushared/Badge.tsx`) through a wildcard tsconfig alias —
/// the exact same claim shape
/// `sibling_css_module_command_layer_build.rs`'s
/// `write_utility_only_sibling_fixture` proves for `zfb build`, adapted here
/// for a `zfb dev` session. Deliberately independent from the A-D
/// `dev-sibling-watch` fixture (see this file's header comment): the sibling
/// here is claimed ONLY through the tsconfig alias (`SiblingMirrorPlan`
/// claim source (b)) — no `?raw`/worker/plain-module import ever reaches it
/// — so the ONLY registry that can make its edits observable is the
/// `css_mirror_roots` recursive-directory watch (#1801/#1802).
fn write_tailwind_sibling_dev_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .expect("write pnpm-workspace.yaml");
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/uhost");
    fs::create_dir_all(project.join("pages")).expect("create pages/");

    // Issue #1803's confound (documented at
    // sibling_css_module_command_layer_build.rs:649-662, restated here for a
    // DEV session): `zfb dev` writes its own SSR bundle under
    // `<project_root>/.zfb-build/`, which inlines every JSX class-attribute
    // string literal it touches. Tailwind v4's automatic content detection
    // (active here since no `source(none)` is ever added) respects
    // `.gitignore` — WITHOUT this file it could see the sibling's new
    // utility class through that generated bundle instead of through the
    // mirror-root `@source` wiring under test, making the assertion below
    // pass for the wrong reason.
    fs::write(
        project.join(".gitignore"),
        ".zfb-build/\ndist/\nnode_modules/\n",
    )
    .expect("write project .gitignore");

    // No `tailwind` key -> CSS (and the Tailwind utility scan) is enabled by
    // default.
    fs::write(
        project.join("zfb.config.json"),
        "{\n  \"framework\": \"preact\"\n}\n",
    )
    .expect("write zfb.config.json");

    fs::write(
        project.join("tsconfig.json"),
        "{\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"@ushared/*\": [\"../../lib/ushared/*\"] }\n  }\n}\n",
    )
    .expect("write tsconfig.json");

    fs::write(
        project.join("pages/index.tsx"),
        "import Badge from \"@ushared/Badge\";\n\n\
         export default function HomePage() {\n  \
         return (\n    <main>\n      <Badge />\n    </main>\n  );\n}\n",
    )
    .expect("write pages/index.tsx");

    let sibling = ws_root.join("lib/ushared");
    fs::create_dir_all(&sibling).expect("create lib/ushared");
    // Boot state: no Tailwind utility class the assertion below looks for
    // exists anywhere in the fixture yet.
    fs::write(
        sibling.join("Badge.tsx"),
        "export default function Badge() {\n  \
         return <span>badge</span>;\n}\n",
    )
    .expect("write lib/ushared/Badge.tsx");

    (project, nm_handle)
}

/// Scenario E acceptance: a sibling `.tsx` edit that introduces a NEW
/// Tailwind utility class used nowhere else in the fixture refreshes the
/// served `/assets/styles.css` without a `zfb dev` restart.
///
/// Needs the Tailwind binary in addition to esbuild — skips cleanly when
/// unavailable (see `locate_tailwind`).
///
/// Falsifiability: reverting `orchestrator.rs`'s
/// `register_dynamic_dependency_watches` call to
/// `Watcher::sync_recursive_dir_watches` (the #1802 registration this test
/// exists to prove) leaves `lib/ushared` completely unwatched — nothing else
/// in this fixture ever imports it — so the edit below produces no
/// filesystem event the orchestrator ever sees, and `edit_until_served`
/// times out on `SCENARIO_DEADLINE` instead of observing the new class.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: tailwindcss v4 + esbuild — cargo test -p zfb --test \
            dev_sibling_watch_1678_e2e -- --ignored --exact \
            e2e_dev_sibling_tailwind_utility_class_refreshes_served_css \
            (ZFB_TAILWIND_BIN or the staged crates/zfb/binaries/tailwindcss-v4 \
            slot; also needs ZFB_ESBUILD_BIN or an esbuild on PATH)"]
async fn e2e_dev_sibling_tailwind_utility_class_refreshes_served_css() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_sibling_watch_1678 scenario E] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    let Some(tailwind) = locate_tailwind() else {
        eprintln!(
            "[dev_sibling_watch_1678 scenario E] no tailwindcss v4 binary available; \
             skipping. Set ZFB_TAILWIND_BIN or stage crates/zfb/binaries/tailwindcss-v4."
        );
        return;
    };

    let workspace = tempfile::tempdir().expect("scenario E fixture tempdir");
    let (project, _nm_handle) = write_tailwind_sibling_dev_fixture(workspace.path());
    let sibling_badge = workspace.path().join("lib/ushared/Badge.tsx");

    let mut session = spawn_dev(&project, &esbuild, Some(&tailwind));
    let Some(port) = wait_for_ready(&mut session).await else {
        return; // environmental skip (no V8/esbuild)
    };
    let origin = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client");
    let css_url = format!("{origin}/assets/styles.css");

    // The boot CSS pass computes `SiblingMirrorPlan` from the tsconfig alias
    // above (no import needed — claim source (b) is alias-based) and
    // publishes `lib/ushared` as a `css_mirror_roots` entry, which the
    // orchestrator's dynamic-watch registration turns into a recursive
    // directory watch — surfaced, under `ZFB_DEV_TIMING`, as the same
    // `watch-extra registered:` line Scenario C keys its wait on above. This
    // IS the #1802 registration Scenario E exists to confirm.
    wait_for_watch_extra(&session, "lib/ushared").await;

    // Baseline: the served stylesheet must be reachable and must NOT yet
    // contain the utility class the edit below introduces for the first
    // time.
    let boot_css = {
        let started = Instant::now();
        loop {
            if let Ok(response) = client.get(&css_url).send().await {
                if response.status().as_u16() == 200 {
                    break response.text().await.unwrap_or_default();
                }
            }
            assert!(
                started.elapsed() < BOOT_CONTENT_DEADLINE,
                "scenario E: GET {css_url} never answered 200 within {}s\n{}",
                BOOT_CONTENT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    assert!(
        !boot_css.contains("ff6ad5"),
        "scenario E: boot stylesheet must not already contain the sibling-only \
         utility class before the edit below introduces it\n{}",
        session.logs(),
    );

    // THE EDIT — a sibling `.tsx` file (`PathClass::Module`; the #1288 rule
    // unconditionally marks CSS dirty on this classification) gains a NEW
    // Tailwind utility class (`bg-[#ff6ad5]`) used nowhere else in the
    // fixture.
    edit_until_served(
        &client,
        &sibling_badge,
        "export default function Badge() {\n  \
         return <span class=\"bg-[#ff6ad5]\">badge</span>;\n}\n",
        &css_url,
        "ff6ad5",
        "scenario E: sibling utility-class edit refreshes /assets/styles.css",
        &session,
    )
    .await;

    session.guard.child.try_wait().ok();
}

// ---------------------------------------------------------------------------
// Issue #3163 (epic #3160) — SSR workspace-package dependency edits
// ---------------------------------------------------------------------------
//
// Confirm pass for #3161 (manifest-declared `dist/` kept when staging a
// claimed workspace package) and #3162 (the dev SSR module-dependency
// registry, `RawImportInvalidation::replace_ssr_module_deps`, folded into
// `GranularityPolicy::dynamic_dependency_paths`). Every fixture below is its
// own pnpm workspace whose SSR page imports a first-party package by NAME
// (`node_modules/<pkg>` -> `packages/<pkg>`, the link `pnpm install` makes).
// None of those packages is reached through a `?raw`/worker/plain client
// import or a tsconfig alias, so the #1678 file-parent channels and the
// `css_mirror_roots` channel never claim them: the SSR registry, plus the
// boot-only #1284 D4 out-of-root watch for an eager nested host, are the only
// ways their edits can reach the orchestrator. Readiness is always keyed on
// the initially served value, never on `watch-extra registered:` — that line
// is emitted by the very registration under test.

/// Issue #3163 — graceful-shutdown deadline for the restart scenario (same
/// value `dev_supervision_e2e.rs` uses for the identical SIGINT shape).
const GRACEFUL_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);

const ROOT_DATA_V1: &str = "ZFB3163_ROOT_DATA_V1";
const ROOT_DATA_V2: &str = "ZFB3163_ROOT_DATA_V2_EDITED";
const NESTED_DATA_V1: &str = "ZFB3163_NESTED_DATA_V1";
const NESTED_DATA_V2: &str = "ZFB3163_NESTED_DATA_V2_EDITED";
const LATE_V1: &str = "ZFB3163_LATE_V1";
const LATE_V2: &str = "ZFB3163_LATE_V2_EDITED";
const DIST_V1: &str = "ZFB3163_DIST_V1";
const DIST_V2: &str = "ZFB3163_DIST_V2_EDITED";

fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client")
}

fn json_value(marker: &str) -> String {
    format!("{{ \"v\": \"{marker}\" }}\n")
}

/// A first-party JSON package exposing every file through `"./*": "./*"`.
fn write_json_workspace_package(dir: &Path, name: &str, marker: &str) {
    fs::create_dir_all(dir).expect("create JSON workspace package dir");
    fs::write(
        dir.join("package.json"),
        format!(
            "{{ \"name\": \"{name}\", \"private\": true, \"type\": \"module\", \
             \"exports\": {{ \"./*\": \"./*\" }} }}\n"
        ),
    )
    .expect("write JSON workspace package.json");
    fs::write(dir.join("value.json"), json_value(marker)).expect("write value.json");
}

/// The project's own `node_modules` (holding the workspace links) disarms
/// the embedded-vendor SSR runtime fallback, so link each extracted embedded
/// package in beside them. The handle must outlive the dev session.
fn link_embedded_ssr_runtime(project: &Path) -> tempfile::TempDir {
    let (handle, embedded) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let node_modules = project.join("node_modules");
    fs::create_dir_all(&node_modules).expect("create project node_modules");
    for entry in fs::read_dir(&embedded).expect("read embedded node_modules") {
        let entry = entry.expect("embedded node_modules entry");
        std::os::unix::fs::symlink(entry.path(), node_modules.join(entry.file_name()))
            .expect("link embedded runtime package into project node_modules");
    }
    handle
}

/// The relative `node_modules/<name>` link `pnpm install` creates for a
/// `workspace:*` dependency.
fn link_workspace_package(project: &Path, name: &str, relative_target: &str) {
    std::os::unix::fs::symlink(relative_target, project.join("node_modules").join(name))
        .expect("link workspace package into project node_modules");
}

fn write_zfb_project_shell(project: &Path, name: &str, deps: &[&str]) {
    fs::create_dir_all(project.join("pages")).expect("create pages/");
    let deps = deps
        .iter()
        .map(|dep| format!("\"{dep}\": \"workspace:*\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        project.join("package.json"),
        format!(
            "{{ \"name\": \"{name}\", \"private\": true, \"type\": \"module\", \
             \"dependencies\": {{ {deps} }} }}\n"
        ),
    )
    .expect("write project package.json");
    fs::write(
        project.join("zfb.config.json"),
        "{\n  \"framework\": \"preact\",\n  \"tailwind\": { \"enabled\": false }\n}\n",
    )
    .expect("write zfb.config.json");
}

/// An SSR page rendering each of `values` (JS expressions) in its own `<p>`.
fn ssr_page(imports: &str, values: &[&str]) -> String {
    let paragraphs: String = values
        .iter()
        .map(|value| format!("          <p>{{{value}}}</p>\n"))
        .collect();
    format!(
        "{imports}\nexport default function WorkspaceDepPage() {{\n  return (\n    \
         <html lang=\"en\">\n      <head>\n        <meta charSet=\"utf-8\" />\n        \
         <title>ZFB3163_PAGE</title>\n      </head>\n      <body>\n        <main>\n\
         {paragraphs}        </main>\n      </body>\n    </html>\n  );\n}}\n"
    )
}

/// Scenario (a)/(d)/(e) fixture: the site IS the workspace root
/// (`packages: ['packages/*']`), so `packages/data` sits inside the project
/// root but outside every default watch root. Returns the embedded-runtime
/// handle.
fn write_root_site_3163(ws: &Path) -> tempfile::TempDir {
    fs::write(
        ws.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n",
    )
    .expect("write pnpm-workspace.yaml");
    write_zfb_project_shell(ws, "zfb3163-root-site", &["data"]);
    write_json_workspace_package(&ws.join("packages/data"), "data", ROOT_DATA_V1);
    let handle = link_embedded_ssr_runtime(ws);
    link_workspace_package(ws, "data", "../packages/data");
    fs::write(
        ws.join("pages/index.tsx"),
        ssr_page("import data from \"data/value.json\";\n", &["data.v"]),
    )
    .expect("write pages/index.tsx");
    handle
}

/// Scenario (b)/(c) fixture shell: a pnpm workspace with the site nested at
/// `apps/site`, so every `packages/*` sibling is OUTSIDE the project root and
/// staged as a claimed workspace package. Returns `(site, runtime handle)`.
fn write_nested_workspace_3163(ws: &Path, deps: &[&str]) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws.join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
    )
    .expect("write pnpm-workspace.yaml");
    fs::write(
        ws.join("package.json"),
        "{ \"name\": \"zfb3163-workspace\", \"private\": true }\n",
    )
    .expect("write workspace package.json");
    let site = ws.join("apps/site");
    write_zfb_project_shell(&site, "zfb3163-nested-site", deps);
    let handle = link_embedded_ssr_runtime(&site);
    (site, handle)
}

async fn boot_3163(
    project: &Path,
    esbuild: &Path,
    extra_env: &[(&str, &str)],
    log_tag: &str,
) -> Option<(DevSession, String)> {
    let mut session = spawn_dev_with_env(project, esbuild, None, extra_env, log_tag);
    let port = wait_for_ready(&mut session).await?;
    Some((session, format!("http://localhost:{port}/")))
}

/// Send a real SIGINT (Ctrl+C) to the dev server's process group and wait
/// for a clean exit, as a user stopping `zfb dev` would.
async fn stop_gracefully(session: &mut DevSession) {
    unsafe {
        libc::kill(-session.guard.pgid, libc::SIGINT);
    }
    let started = Instant::now();
    loop {
        if let Some(status) = session.guard.try_status() {
            assert!(
                status.success(),
                "`zfb dev` must exit 0 after SIGINT, got {status:?}\n{}",
                session.logs(),
            );
            return;
        }
        assert!(
            started.elapsed() < GRACEFUL_SHUTDOWN_DEADLINE,
            "`zfb dev` did not exit within {}s of SIGINT\n{}",
            GRACEFUL_SHUTDOWN_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Scenario (a) — root site: an edit of `packages/data/value.json`, which the
/// SSR page imports as `data/value.json`, is served without a restart.
///
/// Falsifiability (revert-proven, #3163): with the SSR set's fold removed from
/// `GranularityPolicy::dynamic_dependency_paths`, `packages/data` is watched by
/// nothing (it is in-root, so #1284 D4 skips it, and no default watch root
/// covers it) and the edit below times out.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3163_root_site_ssr_workspace_json_edit_is_served() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_sibling_watch_1678 #3163 (a)] no esbuild binary available; skipping.");
        return;
    };
    let workspace = tempfile::tempdir().expect("#3163 (a) fixture tempdir");
    let _runtime = write_root_site_3163(workspace.path());
    let Some((session, page_url)) = boot_3163(workspace.path(), &esbuild, &[], "").await else {
        return;
    };
    let client = loopback_client();

    poll_body(
        &client,
        &page_url,
        ROOT_DATA_V1,
        "#3163 (a): boot page renders the workspace JSON",
        BOOT_CONTENT_DEADLINE,
        &session,
    )
    .await;
    edit_until_served(
        &client,
        &workspace.path().join("packages/data/value.json"),
        &json_value(ROOT_DATA_V2),
        &page_url,
        ROOT_DATA_V2,
        "#3163 (a): root-site workspace JSON edit is served",
        &session,
    )
    .await;
    // The registry was populated by the SSR bundle: its parent dir is watched.
    wait_for_watch_extra(&session, "packages/data").await;
}

/// Scenario (b) — nested site (`apps/site`): an edit of the boot-imported
/// `packages/data/value.json` is served, and so is an edit of a package the
/// page only starts importing mid-session.
///
/// Falsifiability (revert-proven, #3163): the boot-imported `packages/data` is
/// also covered by the boot-only #1284 D4 watch, but `packages/late` is
/// imported after boot, so with the SSR set's fold reverted nothing watches
/// it and the last edit times out.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3163_nested_site_ssr_workspace_deps_including_post_boot_import_are_served() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_sibling_watch_1678 #3163 (b)] no esbuild binary available; skipping.");
        return;
    };
    let workspace = tempfile::tempdir().expect("#3163 (b) fixture tempdir");
    let ws = workspace.path();
    let (site, _runtime) = write_nested_workspace_3163(ws, &["data", "late"]);
    write_json_workspace_package(&ws.join("packages/data"), "data", NESTED_DATA_V1);
    write_json_workspace_package(&ws.join("packages/late"), "late", LATE_V1);
    link_workspace_package(&site, "data", "../../../packages/data");
    // Installed from the start, but not imported until mid-session.
    link_workspace_package(&site, "late", "../../../packages/late");
    let page = site.join("pages/index.tsx");
    fs::write(
        &page,
        ssr_page("import data from \"data/value.json\";\n", &["data.v"]),
    )
    .expect("write pages/index.tsx");

    let Some((session, page_url)) = boot_3163(&site, &esbuild, &[], "").await else {
        return;
    };
    let client = loopback_client();

    poll_body(
        &client,
        &page_url,
        NESTED_DATA_V1,
        "#3163 (b): boot page renders the sibling workspace JSON",
        BOOT_CONTENT_DEADLINE,
        &session,
    )
    .await;
    edit_until_served(
        &client,
        &ws.join("packages/data/value.json"),
        &json_value(NESTED_DATA_V2),
        &page_url,
        NESTED_DATA_V2,
        "#3163 (b): boot-imported sibling workspace JSON edit is served",
        &session,
    )
    .await;

    // Introduce the new dependency through an in-project edit (`pages/` is a
    // watched root, so this alone fires a tick).
    fs::write(
        &page,
        ssr_page(
            "import data from \"data/value.json\";\nimport late from \"late/value.json\";\n",
            &["data.v", "late.v"],
        ),
    )
    .expect("add the late import to pages/index.tsx");
    poll_body(
        &client,
        &page_url,
        LATE_V1,
        "#3163 (b): the post-boot import is rendered",
        SCENARIO_DEADLINE,
        &session,
    )
    .await;
    edit_until_served(
        &client,
        &ws.join("packages/late/value.json"),
        &json_value(LATE_V2),
        &page_url,
        LATE_V2,
        "#3163 (b): post-boot sibling workspace JSON edit is served",
        &session,
    )
    .await;
    wait_for_watch_extra(&session, "packages/late").await;
}

/// Scenario (c) — nested site consuming a compiled-JS-only sibling whose
/// `exports` point into `dist/` (#3161 keeps that `dist/` when staging): an
/// edit of `packages/lib/dist/value.js` is served.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3163_nested_site_compiled_dist_sibling_edit_is_served() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_sibling_watch_1678 #3163 (c)] no esbuild binary available; skipping.");
        return;
    };
    let workspace = tempfile::tempdir().expect("#3163 (c) fixture tempdir");
    let ws = workspace.path();
    let (site, _runtime) = write_nested_workspace_3163(ws, &["lib"]);
    let lib = ws.join("packages/lib");
    fs::create_dir_all(lib.join("dist")).expect("create packages/lib/dist");
    fs::write(
        lib.join("package.json"),
        "{ \"name\": \"lib\", \"private\": true, \"type\": \"module\", \"exports\": \
         { \"./value\": { \"types\": \"./dist/value.d.ts\", \"default\": \"./dist/value.js\" } } }\n",
    )
    .expect("write packages/lib/package.json");
    // The build output a compiled-JS-only package ships but does not commit.
    fs::write(lib.join(".gitignore"), "dist/\n").expect("write packages/lib/.gitignore");
    fs::write(
        lib.join("dist/value.d.ts"),
        "export declare const libValue: string;\n",
    )
    .expect("write dist/value.d.ts");
    let dist_value = lib.join("dist/value.js");
    fs::write(
        &dist_value,
        format!("export const libValue = \"{DIST_V1}\";\n"),
    )
    .expect("write dist/value.js");
    link_workspace_package(&site, "lib", "../../../packages/lib");
    fs::write(
        site.join("pages/index.tsx"),
        ssr_page("import { libValue } from \"lib/value\";\n", &["libValue"]),
    )
    .expect("write pages/index.tsx");

    let Some((session, page_url)) = boot_3163(&site, &esbuild, &[], "").await else {
        return;
    };
    let client = loopback_client();

    poll_body(
        &client,
        &page_url,
        DIST_V1,
        "#3163 (c): boot page renders the compiled dist/ export",
        BOOT_CONTENT_DEADLINE,
        &session,
    )
    .await;
    edit_until_served(
        &client,
        &dist_value,
        &format!("export const libValue = \"{DIST_V2}\";\n"),
        &page_url,
        DIST_V2,
        "#3163 (c): compiled dist/ sibling edit is served",
        &session,
    )
    .await;
}

/// Scenario (d) — #3155 reported that restarting `zfb dev` did not pick up
/// an edit. Edit the dependency while the first session runs, stop it with a
/// real SIGINT straight away (not waiting for that session to serve the
/// edit), restart over the same tree — its persisted `.zfb/` graph and
/// `.zfb-build/dev-pages` included — and require the new value. This does not
/// depend on the watcher at all, so it holds with #3162 reverted too.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3163_restart_serves_ssr_workspace_edit_made_before_the_stop() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_sibling_watch_1678 #3163 (d)] no esbuild binary available; skipping.");
        return;
    };
    let workspace = tempfile::tempdir().expect("#3163 (d) fixture tempdir");
    let _runtime = write_root_site_3163(workspace.path());
    let client = loopback_client();

    let Some((mut first, page_url)) = boot_3163(workspace.path(), &esbuild, &[], "-first").await
    else {
        return;
    };
    poll_body(
        &client,
        &page_url,
        ROOT_DATA_V1,
        "#3163 (d): first session renders the workspace JSON",
        BOOT_CONTENT_DEADLINE,
        &first,
    )
    .await;
    fs::write(
        workspace.path().join("packages/data/value.json"),
        json_value(ROOT_DATA_V2),
    )
    .expect("edit packages/data/value.json");
    stop_gracefully(&mut first).await;
    assert!(
        workspace.path().join(".zfb/graph.bin").is_file(),
        "the first session must persist its graph, so the restart is a warm one\n{}",
        first.logs(),
    );

    let Some((second, page_url)) = boot_3163(workspace.path(), &esbuild, &[], "-second").await
    else {
        return;
    };
    let body = poll_body(
        &client,
        &page_url,
        ROOT_DATA_V2,
        "#3163 (d): the restarted session serves the edit",
        BOOT_CONTENT_DEADLINE,
        &second,
    )
    .await;
    assert!(
        !body.contains(ROOT_DATA_V1),
        "the restarted session must not serve the pre-edit value\n{body}\n{}",
        second.logs(),
    );
}

/// Scenario (e) — deferred Cold boot (`ZFB_DEV_BOOT_LAZY=cold`: the SSR
/// bundle is built after bind, inside the orchestrator's boot hook). Once the
/// deferred publish has served the initial value, an edit of the dependency —
/// with no source edit or tick in between — is served.
///
/// Falsifiability (revert-proven, #3163): the deferred path never populates
/// the #1284 D4 targets, and `packages/data` is in-root anyway, so with the
/// SSR set's fold reverted the edit times out.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3163_deferred_cold_boot_ssr_workspace_json_edit_is_served() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_sibling_watch_1678 #3163 (e)] no esbuild binary available; skipping.");
        return;
    };
    let workspace = tempfile::tempdir().expect("#3163 (e) fixture tempdir");
    let _runtime = write_root_site_3163(workspace.path());
    let Some((session, page_url)) = boot_3163(
        workspace.path(),
        &esbuild,
        &[("ZFB_DEV_BOOT_LAZY", "cold")],
        "",
    )
    .await
    else {
        return;
    };
    let client = loopback_client();

    // Cold has no `dist/` seed: the page only answers 200 once the deferred
    // publish has landed, so this is also the "publish completed" signal.
    poll_body(
        &client,
        &page_url,
        ROOT_DATA_V1,
        "#3163 (e): the deferred publish renders the workspace JSON",
        BOOT_CONTENT_DEADLINE,
        &session,
    )
    .await;
    edit_until_served(
        &client,
        &workspace.path().join("packages/data/value.json"),
        &json_value(ROOT_DATA_V2),
        &page_url,
        ROOT_DATA_V2,
        "#3163 (e): workspace JSON edit after the deferred publish is served",
        &session,
    )
    .await;
    wait_for_watch_extra(&session, "packages/data").await;
}

// ---------------------------------------------------------------------------
// Issue #3190 (epic #3187, source #3181) — the first edit right after `ready`
// ---------------------------------------------------------------------------

/// Issue #3190 — poll budget after the single edit. The reporter's later
/// edits landed in 8–12 s; 30 s is well clear of that while still failing
/// fast when the edit is lost for good (#3181 waited 120 s).
const FIRST_EDIT_DEADLINE: Duration = Duration::from_secs(30);

/// Write `path` EXACTLY ONCE, then only poll `url` until it serves `marker`.
/// Returns the write-to-served latency. Unlike [`edit_until_served`] it never
/// re-issues the write: a re-write is a second FS event that can arrive after
/// the watch arms, which is exactly what hid #3181 from the #3163 tests.
async fn write_once_until_served(
    client: &reqwest::Client,
    path: &Path,
    contents: &str,
    url: &str,
    marker: &str,
    label: &str,
    session: &DevSession,
) -> Duration {
    fs::write(path, contents).unwrap_or_else(|e| panic!("edit {}: {e}", path.display()));
    let written = Instant::now();
    let mut last = String::from("no response");
    while written.elapsed() < FIRST_EDIT_DEADLINE {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if status.as_u16() == 200 && body.contains(marker) {
                    return written.elapsed();
                }
                last = format!("status={status}");
            }
            Err(error) => last = format!("request failed: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "{label}: {url} never served {marker:?} within {}s of a SINGLE write to {}; \
         last={last}\n{}",
        FIRST_EDIT_DEADLINE.as_secs(),
        path.display(),
        session.logs(),
    );
}

/// Issue #3190 — boot the scenario-(a) root site with `extra_env`, write
/// `packages/data/value.json` ONCE the instant `ready` is printed (no boot
/// page poll first), and require that single edit to be served. Returns the
/// session and page URL for follow-up edits, or `None` on an environmental
/// skip.
async fn first_edit_right_after_ready_3190(
    workspace: &Path,
    extra_env: &[(&str, &str)],
    label: &str,
) -> Option<(DevSession, String, Duration)> {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dev_sibling_watch_1678 #3190] no esbuild binary available; skipping.");
        return None;
    };
    let (session, page_url) = boot_3163(workspace, &esbuild, extra_env, "").await?;
    let latency = write_once_until_served(
        &loopback_client(),
        &workspace.join("packages/data/value.json"),
        &json_value(ROOT_DATA_V2),
        &page_url,
        ROOT_DATA_V2,
        label,
        &session,
    )
    .await;
    Some((session, page_url, latency))
}

/// Issue #3190 (#3181's repro) — root site, eager boot (no `dist/`): a single
/// edit of `packages/data/value.json` written the instant `ready` is printed
/// must be served. Later single edits in the same session are timed and
/// printed as `[#3190 timing]` lines (the #3181 latency note).
///
/// On an unthrottled small fixture the watcher is usually armed before the
/// edit lands, so this passes even without the fix; the slow-window test
/// below is the falsifiable one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3190_root_site_ssr_workspace_json_edit_right_after_ready_is_served() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let workspace = tempfile::tempdir().expect("#3190 fixture tempdir");
    let _runtime = write_root_site_3163(workspace.path());
    let Some((session, page_url, first)) = first_edit_right_after_ready_3190(
        workspace.path(),
        &[],
        "#3190: the first edit right after `ready` is served",
    )
    .await
    else {
        return;
    };
    eprintln!("[#3190 timing] first edit (0 s after ready) served after {first:?}");

    let client = loopback_client();
    for (round, marker) in [ROOT_DATA_V1, ROOT_DATA_V2].into_iter().enumerate() {
        let latency = write_once_until_served(
            &client,
            &workspace.path().join("packages/data/value.json"),
            &json_value(marker),
            &page_url,
            marker,
            "#3190: a later single edit is served",
            &session,
        )
        .await;
        eprintln!(
            "[#3190 timing] later edit #{} served after {latency:?}",
            round + 1
        );
    }
}

/// Issue #3190 — the same single edit, with both boot windows of a large site
/// widened by the existing test seams: the orchestrator (and so the watcher)
/// starts 2 s after `ready` (`ZFB_DEV_TEST_SLOW_DIGEST_MS`), and the eager
/// boot render takes 2 s (`ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS`). The edit lands
/// after the eager bundle read `value.json` and before any watch covers it,
/// so it produces no filesystem event at all.
///
/// Falsifiability (revert-proven, #3190): with the orchestrator's pre-boot
/// `unobserved_dependency_edits` call (`unobserved_ssr_dependency_edits`
/// before #3201) removed the edit is never served; it fails on the pre-fix
/// base and on v2.21.1 (`066e058`) too.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a real `zfb dev --port 0` with embedded V8 + esbuild and polls over HTTP; too slow / port-bound for the T1 gate"]
async fn e2e_3190_edit_before_the_watcher_is_armed_is_served() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let workspace = tempfile::tempdir().expect("#3190 fixture tempdir");
    let _runtime = write_root_site_3163(workspace.path());
    let Some((session, _, _)) = first_edit_right_after_ready_3190(
        workspace.path(),
        &[
            ("ZFB_DEV_TEST_SLOW_DIGEST_MS", "2000"),
            ("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS", "2000"),
        ],
        "#3190: an edit made before the watcher is armed is served",
    )
    .await
    else {
        return;
    };
    assert!(
        session
            .stderr()
            .contains("watch-arm reconcile: edited before its watch:"),
        "the edit must be recovered by the watch-arm reconcile, not a late event\n{}",
        session.logs(),
    );
}
