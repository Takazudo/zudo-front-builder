//! Level-4 `zfb dev` boot confirmation for the SSR route-contract guard's
//! dev-time warning (issue #2357, SSR Route Contract Guard epic #2351,
//! Wave 3).
//!
//! ## Why this test exists
//!
//! #2350's **primary** request is a *dev-time* warning: a `prerender =
//! false` route written as `export default async function
//! Handler(request: Request)` silently 405s on every real request,
//! because zfb calls a page's default export with the page's PROPS
//! object, never the incoming `Request` — `request.method` is always
//! `undefined`, and `Request` is a perfectly valid TS annotation for a
//! parameter that receives props, so `tsc` (and `zfb check`, sans this
//! epic) never complains. #2354 wired the shared detector
//! (`render_pipeline::build_prerender_map` /
//! `render_ssr_request_param_finding`) into `zfb dev`'s boot path via
//! `output::warn(...)`. A unit test of that helper proves the message
//! renders correctly, not that a developer running `zfb dev` ever sees
//! it — this file boots the REAL `zfb` binary and reads its captured
//! stderr, the only proof that closes the loop.
//!
//! ## Precision, not just presence
//!
//! A detector that warns on every `prerender = false` route would satisfy
//! the positive assertion trivially and be worse than no detector at all.
//! The negative fixture carries the two correct SSR-handler shapes the
//! detector's gate must never fire on (a zero-parameter API handler, and a
//! dynamic route destructuring `{ params }`) and this file asserts
//! **silence** across a real boot over them.
//!
//! ## Modeled on `wasm_ssr_dev_smoke_e2e.rs`
//!
//! That file is this crate's smallest "boot once, assert steady state"
//! dev e2e — no watcher edits, no SSE, just a real `zfb dev --port 0`
//! boot and a polled HTTP assertion. This file follows the same shape:
//! `DevServerGuard` process-group kill on `Drop`, stdout/stderr captured
//! to files (never piped — `build_terminates.rs`'s documented pipe-
//! buffer deadlock), and readiness/response assertions driven by
//! condition-keyed polling with a deadline, never a bare `sleep` (root
//! `CLAUDE.md`'s first deflaking root cause).
//!
//! **Readiness signal**: the dev server's own ready banner
//! (`parse_ready_port`, scanning captured stdout) — the same
//! deterministic boot log line every neighbouring dev e2e test in this
//! crate keys its wait on. The SSR-contract warning is written to stderr
//! earlier in the same synchronous boot path (`output::warn` inside
//! `build_dev_route_tables`, which runs before the HTTP listener starts
//! and the ready banner prints), so by the time the ready banner appears
//! any boot-time warning has already landed in the captured stderr file —
//! no separate wait is needed for it.
//!
//! ## Self-skip convention (no `#[ignore]`)
//!
//! Gated on `zfb_test_utils::locate_esbuild()`, exactly like
//! `dev_serve_e2e`/`wasm_ssr_dev_smoke_e2e`/`dev_supervision_e2e`/
//! `dev_content_reload_2063_e2e` (`crates/CLAUDE.md`'s self-skip
//! convention): health.yml always stages a pinned esbuild, so this runs,
//! unignored, on every T1 gate. Registered in `.config/nextest.toml`'s
//! `e2e-heavy` test-group as **flock-adopting**: this binary spawns a
//! real `zfb dev` process, exactly the class of binary the flock-adopting
//! bucket covers (every other `zfb dev`-spawning e2e in this crate —
//! `dev_serve_e2e`, `wasm_ssr_dev_smoke_e2e`, `dev_supervision_e2e`, and
//! the rest of the `dev_*_e2e` set — holds issue #1339's OS advisory
//! flock too). The build-only bucket is reserved for binaries that run
//! only `zfb build` with no dev server involved.
#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Serializes this file's two spawning tests against EACH OTHER (each
/// boots a full V8 + esbuild dev session; running them concurrently would
/// double memory/CPU and produce flaky boot deadlines) — same pattern as
/// `dev_serve_e2e.rs`'s `SERIAL`. Acquired AFTER the cross-binary flock
/// (see each test fn), matching the lock-ordering convention documented
/// in `dev_serve_e2e.rs`'s header.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The exact Strong-tier substring `render_ssr_request_param_finding`
/// emits (`crates/zfb/src/render_pipeline.rs`) for an annotated-`Request`
/// first parameter. Asserted verbatim so a future wording change fails
/// this test loudly instead of the assertion silently passing on
/// unrelated stderr noise.
const STRONG_TIER_SUBSTRING: &str = "the default export's first parameter is annotated \
     `Request`, but zfb calls a page's default export with the page's props object, never the \
     incoming Request";

fn positive_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ssr-contract-dev-warning-positive")
}

fn negative_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ssr-contract-dev-warning-negative")
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

/// Owns the spawned `zfb dev` process; Drop group-kills the whole tree.
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

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
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
            read_log(&self.stdout_path),
            read_log(&self.stderr_path),
        )
    }
}

/// Extract the ephemeral port from the dev ready banner.
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.find(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

/// Copy `fixture` into a fresh tempdir and spawn `zfb dev --port 0` over
/// it, stdout/stderr captured to files.
fn spawn_dev(fixture: &Path, tmp: &tempfile::TempDir, esbuild: &Path) -> DevSession {
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(fixture, &root).expect("copy SSR contract dev-warning fixture");

    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.process_group(0);

    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Outcome of polling for the dev server's ready banner.
enum BootOutcome {
    Ready(u16),
    /// A recognized environment gate (no esbuild / no embedded-V8 build)
    /// — a legitimate self-skip, not a test failure. Carries the message
    /// to print before returning.
    Skip(String),
}

/// Poll captured stdout for the ready banner (never a bare `sleep`),
/// bailing out cleanly on a recognized environment-gate exit.
async fn wait_for_ready(session: &mut DevSession, label: &str) -> BootOutcome {
    let boot_start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path)
            );
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                return BootOutcome::Skip(format!(
                    "[ssr_contract_dev_warning_e2e:{label}] `zfb dev` exited with a known \
                     environment gate; skipping.\n{}",
                    session.logs(),
                ));
            }
            panic!(
                "[ssr_contract_dev_warning_e2e:{label}] `zfb dev` exited prematurely (status \
                 {status:?}) before its ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "[ssr_contract_dev_warning_e2e:{label}] ready banner printed port 0 instead of \
                 the ephemeral bound port.\n{}",
                session.logs(),
            );
            return BootOutcome::Ready(port);
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "[ssr_contract_dev_warning_e2e:{label}] `zfb dev` did not print a parseable ready \
             banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GET {base}{path}` until it returns `expected_status` with a body
/// containing `expected_marker`, or `RESPONSE_DEADLINE` elapses.
async fn poll_get(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    expected_status: u16,
    expected_marker: &str,
    session: &DevSession,
    label: &str,
) {
    let url = format!("{base}{path}");
    let start = Instant::now();
    loop {
        let observation = match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == expected_status && body.contains(expected_marker) {
                    return;
                }
                format!("status {status}, body:\n{body}")
            }
            Err(error) => format!("request error: {error}"),
        };
        assert!(
            start.elapsed() < RESPONSE_DEADLINE,
            "[ssr_contract_dev_warning_e2e:{label}] GET {url} did not return {expected_status} \
             with {expected_marker:?} within {}s. Last observation: {observation}\n{}",
            RESPONSE_DEADLINE.as_secs(),
            session.logs(),
        );
        // A lazy dev session builds/renders an SSR route on its first
        // request — condition-keyed polling, not a fixed delay.
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// **Positive fixture**: a real `zfb dev --port 0` boot over
/// `pages/api/broken.tsx` (`prerender = false`, default export
/// `(request: Request)`, the epic's silent-405 shape) must emit the
/// Strong-tier SSR-contract warning naming the route and file, AND the
/// dev server must still start and serve the fixture's healthy sibling
/// route normally — the warning must not degrade the dev server.
#[tokio::test(flavor = "multi_thread")]
async fn positive_fixture_warns_on_boot_and_serves_normally() {
    // This binary starts a V8 + esbuild dev session. The advisory flock is
    // acquired before the in-binary SERIAL guard, matching every other
    // flock-adopting dev e2e in this crate (issue #1339).
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[ssr_contract_dev_warning_e2e:positive] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create positive fixture tempdir");
    let mut session = spawn_dev(&positive_fixture_dir(), &tmp, &esbuild);

    let port = match wait_for_ready(&mut session, "positive").await {
        BootOutcome::Ready(port) => port,
        BootOutcome::Skip(message) => {
            eprintln!("{message}");
            return;
        }
    };

    let stderr = read_log(&session.stderr_path);
    assert!(
        stderr.contains(STRONG_TIER_SUBSTRING),
        "[ssr_contract_dev_warning_e2e:positive] boot stderr did not contain the Strong-tier \
         SSR-contract warning substring {STRONG_TIER_SUBSTRING:?}.\n{}",
        session.logs(),
    );
    assert!(
        stderr.contains("route /api/broken:"),
        "[ssr_contract_dev_warning_e2e:positive] boot stderr did not name the offending route \
         (/api/broken).\n{}",
        session.logs(),
    );
    assert!(
        stderr.contains("pages/api/broken.tsx"),
        "[ssr_contract_dev_warning_e2e:positive] boot stderr did not name the offending file \
         (pages/api/broken.tsx).\n{}",
        session.logs(),
    );

    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    // The dev server itself must still start and serve normally — the
    // warning is diagnostic-only and must never degrade the server.
    poll_get(
        &client,
        &base,
        "/",
        200,
        "POSITIVE_FIXTURE_HOME_OK",
        &session,
        "positive",
    )
    .await;

    // Bonus confirmation (not required by the acceptance criteria, but
    // it is the actual bug #2350 reports): the broken route itself keeps
    // serving too — it just silently 405s on every GET, exactly the
    // symptom the warning exists to flag, rather than crashing the dev
    // server or hanging.
    poll_get(
        &client,
        &base,
        "/api/broken",
        405,
        "BROKEN_405",
        &session,
        "positive",
    )
    .await;
}

/// **Negative fixture / precision control**: a real `zfb dev --port 0`
/// boot over a zero-parameter API handler (`pages/api/ok.tsx`) and a
/// dynamic route destructuring its first parameter
/// (`pages/items/[slug].tsx`) must emit NO SSR-contract warning at all,
/// and must serve both routes normally.
#[tokio::test(flavor = "multi_thread")]
async fn negative_fixture_boots_without_warning() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[ssr_contract_dev_warning_e2e:negative] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create negative fixture tempdir");
    let mut session = spawn_dev(&negative_fixture_dir(), &tmp, &esbuild);

    let port = match wait_for_ready(&mut session, "negative").await {
        BootOutcome::Ready(port) => port,
        BootOutcome::Skip(message) => {
            eprintln!("{message}");
            return;
        }
    };

    let stderr = read_log(&session.stderr_path);
    assert!(
        !stderr.contains(STRONG_TIER_SUBSTRING) && !stderr.contains("likely incorrect"),
        "[ssr_contract_dev_warning_e2e:negative] boot stderr contains an SSR-contract warning \
         for a fixture with no violating route (the precision control failed).\n{}",
        session.logs(),
    );

    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    poll_get(
        &client,
        &base,
        "/api/ok",
        200,
        "OK_NO_PARAM",
        &session,
        "negative",
    )
    .await;
    poll_get(
        &client,
        &base,
        "/items/widget",
        200,
        "ITEM_OK:widget",
        &session,
        "negative",
    )
    .await;
}
