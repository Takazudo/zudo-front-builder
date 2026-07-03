//! dev↔build static-path parity tests (issue #1167 — C: confirm gate,
//! acceptance item 2).
//!
//! ## Why this test exists
//!
//! `zfb dev` serves `public/` via a disk fallback waterfall:
//!
//!   `PageCache → html_root → dist_root → public_root → 404`
//!
//! `zfb build` copies `public/` straight into `dist/` (via
//! `copy_public_dir`). The same URL that resolves via the `public_root`
//! fallback in dev must resolve identically after a build.
//!
//! ## Coverage
//!
//! 1. **`/` → index**: `GET /` serves the rendered `index.tsx` page (not
//!    a public file) in BOTH dev and build. Validates the PageCache / dist
//!    waterfall does not get confused by an empty `trimmed` path.
//!
//! 2. **Static file reachable at site root**: `public/img/logo.svg` is
//!    served at `/img/logo.svg` in dev (via `read_from_public`) and at
//!    the same URL after build (via `copy_public_dir` → `dist/img/logo.svg`).
//!
//! 3. **`base`/pathPrefix prefixing**: with `base: "/mysite"` set in
//!    `zfb.config.json`, the same file is reachable at `/mysite/img/logo.svg`
//!    in dev (the router mounts everything under the prefix) and in build
//!    (`copy_public_dir` copies into `dist/mysite/`).
//!
//! 4. **Route-vs-static collision precedence**: the fixture ships both a
//!    rendered `pages/foo.tsx` route (at `/foo`) AND a `public/foo` file.
//!    The rendered route MUST win over the public file in BOTH dev
//!    (page cache / html_root waterfall precedes public_root) and build
//!    (`dist/foo/index.html` from the render, `dist/foo` (raw file) from
//!    public/ — the rendered index.html wins because the server always tries
//!    `<path>/index.html` first).
//!
//! ## Test architecture
//!
//! - **Build assertions (Level 3)**: run `zfb build` as a subprocess and
//!   read the produced `dist/` tree. No HTTP server needed — direct file
//!   inspection is authoritative for the build output.
//!
//! - **Dev assertions**: run `zfb dev` as a subprocess and poll via HTTP.
//!   Reuses the boot/poll pattern from `dev_serve_e2e.rs`.
//!
//! ## Spawn / teardown discipline
//!
//! Identical to `dev_serve_e2e.rs`: process group, file-captured
//! stdout/stderr, `DevServerGuard` group-kills on Drop.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Serializes the `zfb dev`-spawning tests in this file (each boots a full
/// V8 + esbuild dev session; running them concurrently would double memory
/// and CPU and produce flaky boot deadlines — dev_serve_e2e.rs:96). The two
/// `zfb build` tests below (Test 1 + Test 2) also boot V8/esbuild but run
/// as plain sync `#[test]`s, not `#[tokio::test]`s, so they cannot take
/// this `tokio::sync::Mutex` — they rely solely on `CrossBinaryE2eLock`
/// (whose blocking, non-async `acquire()` works in both contexts) for
/// serialization, both against each other and against the two `zfb
/// dev` tests below. Every spawning test acquires `CrossBinaryE2eLock`
/// BEFORE this mutex — see `zfb-test-utils/src/cross_binary_lock.rs` for
/// the lock-ordering rationale (issue #1339).
static SERIAL: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Boot deadline reused from dev_serve_e2e.rs — generous for cold V8 +
/// esbuild.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-poll deadline for HTTP assertions after boot.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-static-parity")
}

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

struct DevSession {
    guard: DevServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn logs(&self) -> String {
        dump_logs(&self.stdout_path, &self.stderr_path)
    }
}

/// Copy the fixture into a fresh tempdir, optionally write a modified
/// `zfb.config.json` (pass `None` to keep the default no-base config), and
/// spawn `zfb dev --port 0` in its own process group.
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path, config_override: Option<&str>) -> DevSession {
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");

    if let Some(cfg) = config_override {
        fs::write(root.join("zfb.config.json"), cfg).expect("write zfb.config.json override");
    }

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
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Wait for the ready banner and return the ephemeral port. Returns `None`
/// for known-skip reasons (no V8 / no esbuild).
async fn boot_and_get_port(session: &mut DevSession) -> Option<u16> {
    let boot_start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path)
            );
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[dev_build_static_parity] `zfb dev` exited with known-skip indicator; \
                     skipping.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before the banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(port, 0, "banner printed port 0.\n{}", session.logs());
            return Some(port);
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `url` until `status` and body contains `needle`. Panics after `deadline`.
async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == expected_status && body.contains(needle) {
                    return;
                }
                last = format!(
                    "status {status}, body start: {}",
                    &body[..body.len().min(300)]
                );
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve status={expected_status} with {needle:?} \
         within {}s. Last: {last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll `url` until it answers with `status` and body DOES NOT contain `excluded`.
async fn poll_until_status_not_containing(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    excluded: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == expected_status && !body.contains(excluded) {
                    return;
                }
                last = format!(
                    "status {status}, body start: {}",
                    &body[..body.len().min(300)]
                );
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve status={expected_status} without {excluded:?} \
         within {}s. Last: {last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Level-3 build assertions (no dev server — just read dist/ output)
// ---------------------------------------------------------------------------

/// Run `zfb build` against a copy of the fixture in `root`. Returns `false`
/// when the process exits with a known-skip indicator (V8/esbuild absent).
fn run_build(root: &Path, esbuild: &Path, config_override: Option<&str>) -> bool {
    if let Some(cfg) = config_override {
        fs::write(root.join("zfb.config.json"), cfg).expect("write zfb.config.json override");
    }

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    if !output.status.success() {
        if combined.contains("embed_v8") || combined.contains("no esbuild") {
            eprintln!(
                "[dev_build_static_parity] `zfb build` exited with known-skip indicator; \
                 skipping.\nstdout: {stdout}\nstderr: {stderr}"
            );
            return false;
        }
        panic!(
            "`zfb build` failed unexpectedly.\nstatus: {:?}\nstdout: {stdout}\nstderr: {stderr}",
            output.status,
        );
    }
    true
}

// ---------------------------------------------------------------------------
// Test 1 — Level-3 build output assertions (no-base)
// ---------------------------------------------------------------------------

/// Build output assertions for the no-base case:
///
/// - `dist/index.html` contains the index route marker (/ → rendered index)
/// - `dist/img/logo.svg` exists and contains the public-file content
///   (public/ was copied into dist/ root)
/// - `dist/foo/index.html` contains the route marker (page route wins over
///   `public/foo`)
/// - `dist/foo/index.html` does NOT contain the public-foo content
#[test]
fn build_output_no_base_static_parity() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_build_static_parity] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir for no-base build");
    let root = tmp.path();
    copy_dir(&fixture_dir(), root).expect("copy fixture");

    if !run_build(root, &esbuild, None) {
        return; // known-skip
    }

    let dist = root.join("dist");

    // Assertion 1 — / → rendered index page (not a public file).
    let index_html = dist.join("index.html");
    assert!(
        index_html.exists(),
        "dist/index.html must be emitted by `zfb build`"
    );
    let index_body = fs::read_to_string(&index_html).expect("read dist/index.html");
    assert!(
        index_body.contains("PARITY-INDEX-MARKER"),
        "dist/index.html must contain the rendered index marker; got:\n{}",
        &index_body[..index_body.len().min(500)]
    );

    // Assertion 2 — public/img/logo.svg is copied to dist/img/logo.svg.
    let logo = dist.join("img").join("logo.svg");
    assert!(
        logo.exists(),
        "public/img/logo.svg must be copied to dist/img/logo.svg by `zfb build`"
    );
    let logo_body = fs::read_to_string(&logo).expect("read dist/img/logo.svg");
    assert!(
        logo_body.contains("PARITY-PUBLIC-STATIC-SVG-CONTENT"),
        "dist/img/logo.svg must contain the public-file content; got:\n{logo_body}"
    );

    // Assertion 3 — rendered /foo route wins over public/foo.
    // The rendered route lands at dist/foo/index.html (directory-style);
    // public/foo (plain file) also lands at dist/foo. When the static
    // server probes dist/foo/index.html first it finds the rendered page.
    let foo_index = dist.join("foo").join("index.html");
    assert!(
        foo_index.exists(),
        "dist/foo/index.html must be emitted for the /foo route"
    );
    let foo_body = fs::read_to_string(&foo_index).expect("read dist/foo/index.html");
    assert!(
        foo_body.contains("PARITY-FOO-ROUTE-MARKER"),
        "dist/foo/index.html must contain the route marker (rendered page wins over public/foo); \
         got:\n{}",
        &foo_body[..foo_body.len().min(500)]
    );
    assert!(
        !foo_body.contains("THIS-IS-PUBLIC-FOO-NOT-A-ROUTE"),
        "dist/foo/index.html must NOT contain the public/foo raw content; got:\n{}",
        &foo_body[..foo_body.len().min(500)]
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Level-3 build output assertions (with base)
// ---------------------------------------------------------------------------

/// Build output assertions for the `base: "/mysite/"` case.
///
/// When `base` is set:
///
/// - Rendered HTML pages land at the **outdir root** (e.g. `dist/index.html`),
///   NOT under the base segment — `base` affects the *URL prefixing* inside
///   the HTML (`withBase()` rewrites hrefs to `/mysite/...`), not the
///   on-disk layout of the rendered files. The web server / deploy pipeline
///   is expected to serve the whole `dist/` tree under the `/mysite/` mount.
///
/// - Public files land under the base segment:
///   `public/img/logo.svg` → `dist/mysite/img/logo.svg` (because
///   `copy_public_dir` reads `config.base` and prefixes the destination).
///   This is intentional: `withBase("/img/logo.svg")` → `/mysite/img/logo.svg`,
///   and the file must physically exist under that sub-path so a static
///   server serving `dist/` directly can find it.
///
/// This test asserts BOTH behaviours to lock in the documented split:
/// rendered HTML at `dist/`, public files at `dist/mysite/`.
#[test]
fn build_output_with_base_static_parity() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_build_static_parity] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir for base build");
    let root = tmp.path();
    copy_dir(&fixture_dir(), root).expect("copy fixture");

    // Inject base: "/mysite/" via config override.
    let config_with_base = r#"{ "framework": "preact", "base": "/mysite/" }"#;
    if !run_build(root, &esbuild, Some(config_with_base)) {
        return;
    }

    let dist = root.join("dist");

    // Assertion 1 — rendered index is at dist/index.html (not under
    // dist/mysite/). `base` only affects URL prefixing in the HTML, not
    // the on-disk location of rendered pages.
    let index_html = dist.join("index.html");
    assert!(
        index_html.exists(),
        "dist/index.html must be emitted even when base = /mysite/ — `base` affects \
         URL rewriting, not the rendered-page on-disk layout"
    );
    let index_body = fs::read_to_string(&index_html).expect("read dist/index.html");
    assert!(
        index_body.contains("PARITY-INDEX-MARKER"),
        "dist/index.html must contain the rendered index marker; got:\n{}",
        &index_body[..index_body.len().min(500)]
    );

    // Assertion 2 — public/img/logo.svg lands under the base segment.
    // `copy_public_dir` prefixes the destination with the base segment
    // ("mysite") so a static server at dist/ can serve the file at
    // the `/mysite/img/logo.svg` URL that `withBase()` produces.
    let logo = dist.join("mysite").join("img").join("logo.svg");
    assert!(
        logo.exists(),
        "public/img/logo.svg must land at dist/mysite/img/logo.svg when base = /mysite/"
    );
    let logo_body = fs::read_to_string(&logo).expect("read dist/mysite/img/logo.svg");
    assert!(
        logo_body.contains("PARITY-PUBLIC-STATIC-SVG-CONTENT"),
        "dist/mysite/img/logo.svg must contain the public-file content; got:\n{logo_body}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — dev server assertions (no-base)
// ---------------------------------------------------------------------------

/// Dev server assertions for the no-base case (Level 4 — real subprocess):
///
/// - `GET /` → 200 with the rendered index marker
/// - `GET /img/logo.svg` → 200 with the public-file content
/// - `GET /foo` → 200 with the route marker (not the public/foo content)
#[tokio::test(flavor = "multi_thread")]
async fn dev_serves_no_base_static_parity() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_build_static_parity] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir for no-base dev");
    let mut session = spawn_dev(&tmp, &esbuild, None);

    let Some(port) = boot_and_get_port(&mut session).await else {
        return;
    };

    // HTTP readiness poll.
    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    {
        let start = Instant::now();
        loop {
            match client.get(format!("{base}/")).send().await {
                Ok(resp) if resp.status().as_u16() == 200 => break,
                _ => {}
            }
            assert!(
                start.elapsed() < BOOT_DEADLINE,
                "GET / never answered 200 within {}s.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // Assertion 1 — / → rendered index (not a public file).
    poll_until_contains(
        &client,
        &format!("{base}/"),
        200,
        "PARITY-INDEX-MARKER",
        SCENARIO_DEADLINE,
        "dev no-base: GET / must serve rendered index",
        &session,
    )
    .await;

    // Assertion 2 — public/img/logo.svg reachable at /img/logo.svg.
    poll_until_contains(
        &client,
        &format!("{base}/img/logo.svg"),
        200,
        "PARITY-PUBLIC-STATIC-SVG-CONTENT",
        SCENARIO_DEADLINE,
        "dev no-base: GET /img/logo.svg must serve public file",
        &session,
    )
    .await;

    // Assertion 3 — /foo serves the rendered route, not the public/foo file.
    poll_until_contains(
        &client,
        &format!("{base}/foo"),
        200,
        "PARITY-FOO-ROUTE-MARKER",
        SCENARIO_DEADLINE,
        "dev no-base: GET /foo must serve the rendered route (not public/foo)",
        &session,
    )
    .await;
    // Belt-and-braces: the public/foo raw content must NOT appear.
    poll_until_status_not_containing(
        &client,
        &format!("{base}/foo"),
        200,
        "THIS-IS-PUBLIC-FOO-NOT-A-ROUTE",
        SCENARIO_DEADLINE,
        "dev no-base: GET /foo must not bleed public/foo raw content",
        &session,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Test 4 — dev server assertions (with base prefix)
// ---------------------------------------------------------------------------

/// Dev server assertions for the `base: "/mysite/"` case:
///
/// - `GET /mysite/` → 200 with the rendered index marker
/// - `GET /mysite/img/logo.svg` → 200 with the public-file content
/// - Bare `GET /` → redirect to `/mysite/` (the dev server root redirect)
#[tokio::test(flavor = "multi_thread")]
async fn dev_serves_with_base_prefix_static_parity() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_build_static_parity] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir for base-prefix dev");
    let config_with_base = r#"{ "framework": "preact", "base": "/mysite/" }"#;
    let mut session = spawn_dev(&tmp, &esbuild, Some(config_with_base));

    let Some(port) = boot_and_get_port(&mut session).await else {
        return;
    };

    let base = format!("http://localhost:{port}");
    // Disable auto-redirect following so we can assert the redirect itself.
    let client_no_redirect = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client (no redirect)");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");

    // HTTP readiness under the base prefix.
    {
        let start = Instant::now();
        loop {
            match client.get(format!("{base}/mysite/")).send().await {
                Ok(resp) if resp.status().as_u16() == 200 => break,
                _ => {}
            }
            assert!(
                start.elapsed() < BOOT_DEADLINE,
                "GET /mysite/ never answered 200 within {}s.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // Assertion 1 — /mysite/ → rendered index.
    poll_until_contains(
        &client,
        &format!("{base}/mysite/"),
        200,
        "PARITY-INDEX-MARKER",
        SCENARIO_DEADLINE,
        "dev base-prefix: GET /mysite/ must serve rendered index",
        &session,
    )
    .await;

    // Assertion 2 — public/img/logo.svg reachable under the base prefix.
    poll_until_contains(
        &client,
        &format!("{base}/mysite/img/logo.svg"),
        200,
        "PARITY-PUBLIC-STATIC-SVG-CONTENT",
        SCENARIO_DEADLINE,
        "dev base-prefix: GET /mysite/img/logo.svg must serve public file",
        &session,
    )
    .await;

    // Assertion 3 — bare GET / → redirect to /mysite/ (base mismatch
    // redirect, issue #229). The redirect proves the dev server mounts
    // exclusively under the configured prefix.
    let redirect_resp = client_no_redirect
        .get(format!("{base}/"))
        .send()
        .await
        .expect("GET / for redirect check");
    let redirect_status = redirect_resp.status().as_u16();
    assert!(
        (300..400).contains(&redirect_status),
        "dev base-prefix: bare GET / must redirect (3xx) to the base prefix; \
         got status {redirect_status}.\n{}",
        session.logs(),
    );
    let location = redirect_resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains("/mysite/"),
        "dev base-prefix: redirect Location must point to /mysite/; got {location:?}.\n{}",
        session.logs(),
    );
}
