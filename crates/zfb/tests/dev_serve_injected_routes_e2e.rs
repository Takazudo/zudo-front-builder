//! Injected-route dev-render E2E suite — canonical Level-4 confirm gate for
//! epic #1228 (`inject-route-dev-render`). Boots a real `zfb dev` process
//! over the `package-routes-consumer` fixture and proves the full contract:
//!
//! ## What this suite proves end-to-end
//!
//! | Test | Wave | Contract |
//! |---|---|---|
//! | `dev_e2e_static_injected_route_renders` | S3 #1231 | Static `/preset-about` serves HTML + relative-import content; `virtual:` entrypoint renders; user `pages/` wins on collision; dev `"/"` reservation holds |
//! | `dev_e2e_dynamic_injected_route_renders` | S4 #1232 | `[slug]` renders two concrete URLs; `[...rest]` renders multi-segment; `[[...section]]` bare prefix AND nested AND deeply nested; root not hijacked |
//! | `dev_e2e_injected_route_hmr_content_edit_refreshes` | S5 #1233 | Content edit under a watched collection re-renders the injected route (HMR positive); node_modules restart-only contract held at unit level |
//! | `dev_e2e_trailing_slash_parity` | S6 #1234 | `/preset-about` and `/preset-about/` resolve identically (static); `/preset-docs/getting-started` and `/preset-docs/getting-started/` resolve identically (dynamic) |
//!
//! ## Design decision — fixture reuse
//!
//! All four tests run over the `package-routes-consumer` fixture (the same
//! fixture used by `build_package_routes_consumer.rs` for the build half).
//! The spec (#1234) mentions a possible `dev-loop-with-preset/` sibling, but
//! `package-routes-consumer` already carries static + dynamic + optional-
//! catchall patterns, a colliding user page (`pages/guide.tsx`), and a user
//! home (`pages/index.tsx`) — everything the confirm bullets need.  Creating
//! a separate fixture would duplicate the fixture without adding coverage, so
//! the existing one is kept (minimal change, no churn).  Each test writes its
//! own `preset.mjs` (and any supporting `pkg/*.tsx`) at runtime into a fresh
//! `tempdir` copy of the fixture, so the tests are fully isolated.
//!
//! ## Determinism / spawn discipline
//!
//! Mirrors `dev_serve_e2e.rs`: ephemeral `--port 0`, own process group,
//! stdout/stderr captured to files (never pipes), `DevServerGuard`
//! group-kills on Drop, and an overall wall-clock watchdog. Readiness is
//! an HTTP poll of `GET /` after the ready banner is parsed only to learn
//! the port.
//!
//! ## Skip conditions
//!
//! Skips (does not fail) when esbuild or `node` is unavailable, or when
//! `zfb dev` exits with a known V8/esbuild skip indicator — matching the
//! `build_package_routes_consumer.rs` and `dev_serve_e2e.rs` conventions.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary};

/// Overall wall-clock watchdog. A clean run is boot (V8 + esbuild, slow in
/// debug) plus a handful of request-time renders.
const OVERALL_DEADLINE: Duration = Duration::from_secs(280);

/// Deadline for the ready banner + `GET /` answering 200.
const BOOT_DEADLINE: Duration = Duration::from_secs(120);

/// Per-route deadline for the served marker to appear.
const ROUTE_DEADLINE: Duration = Duration::from_secs(60);

/// Deadline for SSE live-reload events (watcher-live handshake + per-scenario
/// tick signals). Mirrors `dev_serve_e2e.rs::SSE_DEADLINE`.
const SSE_DEADLINE: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("package-routes-consumer")
}

/// `true` when `node` is on PATH (the plugin host needs it).
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Recursive directory copy.
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

/// Extract the port from `→ ready on http://localhost:PORT/` (tolerates
/// ANSI styling; identical heuristic to `dev_serve_e2e.rs`).
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

/// Overwrite the fixture's `preset.mjs` with a preset that injects:
///
/// - `/preset-about` (static, relative TS import → renders),
/// - `/preset-virtual` (static, `virtual:` import → renders),
/// - `/guide` (collides with the user's `pages/guide.tsx` → user wins).
///
/// And register the `virtual:preset-banner` module the virtual page imports.
fn write_injected_preset(root: &Path) {
    let preset = r#"
export default {
  name: "consumer-preset-s3",
  setup({ injectRoute, addVirtualModule }) {
    addVirtualModule(
      "virtual:preset-banner",
      () => "export const banner = 'PRESET_VIRTUAL_BANNER';",
    );
    // Static route importing a relative TS module (renders + bundles).
    injectRoute("/preset-about", "./pkg/about.tsx");
    // Static route importing a virtual: module (must render too).
    injectRoute("/preset-virtual", "./pkg/virtual-page.tsx");
    // Collides with the user's pages/guide.tsx — user pages/ must win.
    injectRoute("/guide", "./pkg/guide-shadow.tsx");
  },
};
"#;
    fs::write(root.join("preset.mjs"), preset).expect("write injected preset.mjs");

    // The virtual-importing static entrypoint.
    let virtual_page = r#"
import { banner } from "virtual:preset-banner";

export default function VirtualPage() {
  return (
    <html lang="en">
      <head><title>Virtual</title></head>
      <body>
        <h1>CONSUMER_PRESET_VIRTUAL_MARKER</h1>
        <p>{banner}</p>
      </body>
    </html>
  );
}
"#;
    fs::write(root.join("pkg").join("virtual-page.tsx"), virtual_page)
        .expect("write pkg/virtual-page.tsx");

    // The injected /guide page that MUST lose to the user's pages/guide.tsx.
    let guide_shadow = r#"
export default function GuideShadow() {
  return (
    <html lang="en">
      <head><title>Shadow</title></head>
      <body><h1>INJECTED_GUIDE_SHADOW_MARKER</h1></body>
    </html>
  );
}
"#;
    fs::write(root.join("pkg").join("guide-shadow.tsx"), guide_shadow)
        .expect("write pkg/guide-shadow.tsx");
}

/// Spawn `zfb dev --port 0` over a fresh copy of the consumer fixture
/// (with the S3 injected preset), in its own process group, output to
/// files. Returns the session + the canonical root + the node_modules
/// TempDir handle (must outlive the session).
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path) -> (DevSession, PathBuf, tempfile::TempDir) {
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");
    write_injected_preset(&root);

    // Symlink node_modules to the extracted embedded @takazudo tree so the
    // plugin host + JSX runtime resolve (same as build_package_routes_consumer).
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

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
    // Control the render mode exclusively (lazy default); strip inherited
    // switches that would flip it (mirrors dev_serve_e2e.rs).
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    (
        DevSession {
            guard: DevServerGuard { child, pgid },
            stdout_path,
            stderr_path,
        },
        root,
        nm_handle,
    )
}

/// Poll `url` until the body contains `needle` with status 200. Panics
/// with logs after `deadline`.
async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
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
                if status == 200 && body.contains(needle) {
                    return;
                }
                last = format!("status {status}, body:\n{body}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve {needle:?} within {}s.\nLast: {last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

enum Outcome {
    Completed,
    Skipped,
}

/// Like `poll_until_contains`, but also fails fast (panics immediately) if
/// the server responds with any 3xx redirect instead of a 200.  Used for
/// trailing-slash assertions where the test client has `Policy::none()`:
/// without this guard, a 301/308 regression would silently burn through the
/// full `ROUTE_DEADLINE` before panicking with a misleading "marker not found"
/// message.
async fn poll_until_200_not_redirect(
    client: &reqwest::Client,
    url: &str,
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
                if (300..400).contains(&status) {
                    let location = resp
                        .headers()
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("(none)")
                        .to_string();
                    panic!(
                        "[{phase}] GET {url} returned a {status} redirect to {location:?} — \
                         trailing-slash normalization regression: the dev server must serve \
                         a 200 directly for the trailing-slash variant, not redirect.\n{}",
                        session.logs(),
                    );
                }
                let body = resp.text().await.unwrap_or_default();
                if status == 200 && body.contains(needle) {
                    return;
                }
                last = format!("status {status}, body:\n{body}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve {needle:?} within {}s.\nLast: {last}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
}

/// The full static-injected-route render contract against a real `zfb dev`.
#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_static_injected_route_renders() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[injected_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !node_available() {
        eprintln!("[injected_e2e] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir for consumer fixture");
    let (mut session, _root, _nm) = spawn_dev(&tmp, &esbuild);
    let pgid = session.guard.pgid;

    let body = async {
        // Phase A: discover the port from the ready banner. A premature exit
        // with a V8/esbuild skip indicator → skip the whole test.
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
                        "[injected_e2e] `zfb dev` exited with a known-skip indicator; \
                         skipping.\n{}",
                        session.logs(),
                    );
                    return Outcome::Skipped;
                }
                panic!(
                    "`zfb dev` exited prematurely (status {status:?}) before the ready \
                     banner.\n{}",
                    session.logs(),
                );
            }
            if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
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
            .timeout(Duration::from_secs(10))
            .build()
            .expect("build reqwest client");

        // Phase B: readiness — GET / answers 200.
        {
            let start = Instant::now();
            loop {
                if let Ok(resp) = client.get(format!("{base}/")).send().await {
                    if resp.status().as_u16() == 200 {
                        break;
                    }
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

        // --- The core acceptance: static injected route renders real HTML ---

        // /preset-about: marker + the relative-import content (proof the
        // package route's `./site-meta` import bundled and rendered).
        poll_until_contains(
            &client,
            &format!("{base}/preset-about"),
            "CONSUMER_PRESET_ABOUT_MARKER",
            ROUTE_DEADLINE,
            "static injected /preset-about renders",
            &session,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/preset-about"),
            "Consumer Demo",
            ROUTE_DEADLINE,
            "relative import bundled into the injected route",
            &session,
        )
        .await;

        // /preset-virtual: a `virtual:`-importing static entrypoint renders.
        poll_until_contains(
            &client,
            &format!("{base}/preset-virtual"),
            "CONSUMER_PRESET_VIRTUAL_MARKER",
            ROUTE_DEADLINE,
            "virtual:-importing static injected route renders",
            &session,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/preset-virtual"),
            "PRESET_VIRTUAL_BANNER",
            ROUTE_DEADLINE,
            "virtual: module content reached the rendered injected page",
            &session,
        )
        .await;

        // --- Precedence: user pages/ wins on a colliding pattern ---

        // The preset ALSO injects /guide; the user owns pages/guide.tsx.
        // The user's marker must be served — the injected shadow must NOT.
        poll_until_contains(
            &client,
            &format!("{base}/guide"),
            "CONSUMER_USER_GUIDE_MARKER",
            ROUTE_DEADLINE,
            "user pages/ wins on a colliding injected pattern",
            &session,
        )
        .await;
        {
            let body = client
                .get(format!("{base}/guide"))
                .send()
                .await
                .expect("GET /guide")
                .text()
                .await
                .unwrap_or_default();
            assert!(
                !body.contains("INJECTED_GUIDE_SHADOW_MARKER"),
                "the injected /guide shadow must NOT win over the user page.\nbody:\n{body}\n{}",
                session.logs(),
            );
        }

        // --- Dev "/" reservation unchanged: root is the USER home ---

        {
            let body = client
                .get(format!("{base}/"))
                .send()
                .await
                .expect("GET /")
                .text()
                .await
                .unwrap_or_default();
            assert!(
                body.contains("CONSUMER_USER_HOME_MARKER"),
                "GET / must serve the user's home page (dev `/` reservation unchanged); \
                 no injected route may claim root in dev.\nbody:\n{body}\n{}",
                session.logs(),
            );
        }

        Outcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(Outcome::Completed) | Ok(Outcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] static-injected-route dev E2E did not finish within {}s — \
                 a hang, or a static injected route never rendered. Process group {pgid} \
                 will be killed.\n{}",
                OVERALL_DEADLINE.as_secs(),
                session.logs(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// S4 (#1232) — dynamic injected-route render E2E
// ---------------------------------------------------------------------------

/// Write a preset that covers the three dynamic pattern families from S4:
///
/// - `/preset-docs/[slug]` — single-segment param (the primary test case).
/// - `/archive/[...rest]` — required catch-all (one or more segments).
/// - `/help/[[...section]]` — optional catch-all (zero or more segments).
///   `/help` has no user `pages/` file, so the bare-prefix zero-segment
///   case is reachable through the injected fallback.
///
/// All entrypoints reflect the matched param in the body so the test can
/// assert the dynamic dispatch reached the render.
///
/// Note on the `[[...chapter]]`-vs-user-page interplay (design record §7):
/// the user's `pages/guide.tsx` owns `/guide` as a concrete URL in
/// `url_index`. If the pattern had been `/guide/[[...chapter]]`, the
/// zero-segment case (`/guide`) would have been answered by the user page
/// (it wins via `lookup_by_url` before the fallback is tried). We use
/// `/help/[[...section]]` (no user page at `/help`) to exercise the true
/// bare-prefix matching path.
fn write_dynamic_injected_preset(root: &Path) {
    let preset = r#"
export default {
  name: "consumer-preset-s4",
  setup({ injectRoute }) {
    // [slug] — primary dynamic case: /preset-docs/[slug]
    injectRoute("/preset-docs/[slug]", "./pkg/docs.tsx");
    // [...rest] — required catch-all
    injectRoute("/archive/[...rest]", "./pkg/archive.tsx");
    // [[...section]] — optional catch-all, bare prefix + nested
    // (/help has no user pages/ file so the zero-segment case is reachable)
    injectRoute("/help/[[...section]]", "./pkg/help-optional.tsx");
  },
};
"#;
    fs::write(root.join("preset.mjs"), preset).expect("write dynamic preset.mjs");

    // [...rest] entrypoint.
    let archive = r#"
export default function ArchivePage() {
  return (
    <html lang="en">
      <head><title>Archive</title></head>
      <body>
        <h1>DYNAMIC_ARCHIVE_MARKER</h1>
      </body>
    </html>
  );
}
"#;
    fs::write(root.join("pkg").join("archive.tsx"), archive).expect("write pkg/archive.tsx");

    // [[...section]] entrypoint: optional catch-all.
    let help_optional = r#"
export default function HelpOptional() {
  return (
    <html lang="en">
      <head><title>Help</title></head>
      <body>
        <h1>DYNAMIC_HELP_OPTIONAL_MARKER</h1>
      </body>
    </html>
  );
}
"#;
    fs::write(root.join("pkg").join("help-optional.tsx"), help_optional)
        .expect("write pkg/help-optional.tsx");
}

/// The full dynamic-injected-route render contract against a real `zfb dev`
/// (epic #1228, S4 #1232).
///
/// Boots `zfb dev` over the `package-routes-consumer` fixture with a preset
/// that injects `[slug]`, `[...rest]`, and `[[...section]]` patterns. The test
/// GETs concrete URLs that match each pattern and asserts:
///
/// - The response is 200 with the route's marker — proof Hono resolved params
///   inside V8 and the lazy adapter's synthetic entry rendered and was served.
/// - Different concrete URLs for the same pattern render correctly (slug
///   variance).
/// - The `[[...section]]` bare-prefix case (`GET /help`) matches (no user
///   `pages/help.tsx` exists, so the optional-catchall zero-segment case
///   goes through the dynamic fallback).
/// - The root page (`GET /`) is NOT hijacked by the dynamic injected
///   fallback — the user home still wins.
#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_dynamic_injected_route_renders() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dynamic_injected_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !node_available() {
        eprintln!("[dynamic_injected_e2e] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir for consumer fixture");

    // Boot with the dynamic preset.
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");
    write_dynamic_injected_preset(&root);

    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

    let stdout_path = root.join(".zfb-dev-stdout-s4.log");
    let stderr_path = root.join(".zfb-dev-stderr-s4.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log");

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
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    let mut session = DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path: stdout_path.clone(),
        stderr_path: stderr_path.clone(),
    };
    // `nm_handle` must outlive the dev session.
    let _nm = nm_handle;

    let body = async {
        // Phase A: discover the port from the ready banner.
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
                        "[dynamic_injected_e2e] `zfb dev` exited with a known-skip indicator; \
                         skipping.\n{}",
                        session.logs(),
                    );
                    return Outcome::Skipped;
                }
                panic!(
                    "`zfb dev` exited prematurely (status {status:?}) before the ready banner.\n{}",
                    session.logs(),
                );
            }
            if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
                break port;
            }
            assert!(
                boot_start.elapsed() < BOOT_DEADLINE,
                "`zfb dev` did not print a ready banner within {}s.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        let base = format!("http://localhost:{port}");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("build reqwest client");

        // Phase B: readiness — GET / answers 200.
        {
            let start = Instant::now();
            loop {
                if let Ok(resp) = client.get(format!("{base}/")).send().await {
                    if resp.status().as_u16() == 200 {
                        break;
                    }
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

        // --- [slug] dynamic injected route: /preset-docs/[slug] ---

        // /preset-docs/getting-started: primary acceptance case.
        poll_until_contains(
            &client,
            &format!("{base}/preset-docs/getting-started"),
            "CONSUMER_PRESET_DOCS_MARKER_getting-started",
            ROUTE_DEADLINE,
            "[slug] dynamic injected /preset-docs/getting-started renders",
            &session,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/preset-docs/getting-started"),
            "Consumer Demo",
            ROUTE_DEADLINE,
            "relative import bundled into the dynamic injected [slug] route",
            &session,
        )
        .await;

        // A different slug — same pattern, different concrete URL.
        poll_until_contains(
            &client,
            &format!("{base}/preset-docs/api-reference"),
            "CONSUMER_PRESET_DOCS_MARKER_api-reference",
            ROUTE_DEADLINE,
            "[slug] dynamic injected /preset-docs/api-reference renders",
            &session,
        )
        .await;

        // --- [...rest] required catch-all: /archive/[...rest] ---

        poll_until_contains(
            &client,
            &format!("{base}/archive/2024/01"),
            "DYNAMIC_ARCHIVE_MARKER",
            ROUTE_DEADLINE,
            "[...rest] dynamic injected /archive/2024/01 renders",
            &session,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/archive/deep/a/b/c"),
            "DYNAMIC_ARCHIVE_MARKER",
            ROUTE_DEADLINE,
            "[...rest] dynamic injected /archive/deep/a/b/c renders",
            &session,
        )
        .await;

        // --- [[...section]] optional catch-all: /help/[[...section]] ---

        // Bare prefix (zero-segment case): /help.
        // `/help` has no user pages/ file, so this exercises the true
        // zero-segment match through the dynamic fallback.
        poll_until_contains(
            &client,
            &format!("{base}/help"),
            "DYNAMIC_HELP_OPTIONAL_MARKER",
            ROUTE_DEADLINE,
            "[[...section]] bare-prefix /help renders (zero-segment case)",
            &session,
        )
        .await;

        // Nested path (one segment).
        poll_until_contains(
            &client,
            &format!("{base}/help/intro"),
            "DYNAMIC_HELP_OPTIONAL_MARKER",
            ROUTE_DEADLINE,
            "[[...section]] nested /help/intro renders",
            &session,
        )
        .await;

        // Deeply nested (multiple segments).
        poll_until_contains(
            &client,
            &format!("{base}/help/a/b/c"),
            "DYNAMIC_HELP_OPTIONAL_MARKER",
            ROUTE_DEADLINE,
            "[[...section]] deeply nested /help/a/b/c renders",
            &session,
        )
        .await;

        // --- Dev "/" reservation unchanged: root is the USER home ---
        // A root-scoped optional catchall `/[[...catch]]` would be able to
        // match `/`, but that pattern is not injected here. We verify the
        // user's home page is still served — belt-and-suspenders check that
        // our dynamic fallback does not accidentally capture `/`.
        {
            let body = client
                .get(format!("{base}/"))
                .send()
                .await
                .expect("GET /")
                .text()
                .await
                .unwrap_or_default();
            assert!(
                body.contains("CONSUMER_USER_HOME_MARKER"),
                "GET / must serve the user's home page — the dynamic injected fallback must \
                 not capture root.\nbody:\n{body}\n{}",
                session.logs(),
            );
        }

        Outcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(Outcome::Completed) | Ok(Outcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] dynamic-injected-route dev E2E did not finish within {}s — \
                 a hang, or a dynamic injected route never rendered. Process group {pgid} \
                 will be killed.\n{}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                OVERALL_DEADLINE.as_secs(),
                "check logs below",
                read_log(&stdout_path),
                read_log(&stderr_path),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// S5 (#1233) — HMR: consumer content edits refresh injected routes
// ---------------------------------------------------------------------------
//
// ## Contract being tested
//
// A content edit under a watched collection fires a watcher tick that
// rebuilds the content snapshot and swaps the route tables.  After the swap
// `note_table_swap` bumps the stale-tick generation and
// `mark_injected_seeds_stale` re-claims every static injected route's
// output_path.  The NEXT request to the injected URL re-renders against the
// fresh snapshot — so the served HTML reflects the edited content WITHOUT a
// dev-server restart.
//
// ## Negative (node_modules restart-only) — asserted at the unit-logic level
//
// Injected-route entrypoints live under `node_modules/@takazudo/…` which is
// deliberately absent from `DEFAULT_WATCH_ROOTS` (S5 / epic #1228 §4).
// Editing the package's own compiled source does NOT fire a watcher event
// and therefore does NOT hot-refresh the injected route — a `zfb dev`
// restart is required.  The "no event for a node_modules change" precondition
// is asserted cheaply at the unit-logic level:
//   `crates/zfb/src/commands/dev.rs::tests::default_watch_roots_excludes_node_modules`
//   `crates/zfb/src/commands/dev.rs::tests::derive_watch_roots_never_adds_node_modules`
// A live-boot negative assertion ("wait N seconds, assert NO SSE event") is
// inherently racy — a timing window too short produces false passes, a
// window too long makes the suite slow.  The unit assertion that the watcher
// root set excludes node_modules is the correct and falsifiable precondition.

/// Set up the fixture for the HMR test:
///
/// 1. Overwrite `zfb.config.json` to declare an `articles` collection under
///    `content/articles`.
/// 2. Create two initial article files.
/// 3. Write `pkg/articles-page.tsx` — an injected page that calls
///    `getCollection("articles")` and lists article titles in the body.
/// 4. Write `preset.mjs` to inject `/preset-articles` backed by that page.
///
/// Returns the path to the article used as the HMR edit target.
fn setup_hmr_fixture(root: &Path) -> PathBuf {
    // 1. Patch the config to declare the collection.
    let config_json = r#"
{
  "framework": "preact",
  "plugins": [{ "name": "./preset.mjs" }],
  "collections": [
    { "name": "articles", "path": "content/articles" }
  ]
}
"#;
    fs::write(root.join("zfb.config.json"), config_json.trim_start())
        .expect("write zfb.config.json with articles collection");

    // 2. Create the collection directory + two initial articles.
    let articles_dir = root.join("content").join("articles");
    fs::create_dir_all(&articles_dir).expect("create content/articles");
    fs::write(
        articles_dir.join("intro.md"),
        "---\ntitle: Introduction\n---\n\nIntro body.\n",
    )
    .expect("write intro.md");
    let edit_target = articles_dir.join("feature.md");
    fs::write(
        &edit_target,
        "---\ntitle: V1-HMR-ARTICLE-TITLE\n---\n\nFeature body.\n",
    )
    .expect("write feature.md");

    // 3. Write the injected page that lists article titles via getCollection.
    //    Uses getStaticProps so the collection is read at render time and the
    //    per-tick content snapshot drives the output.
    let articles_page = r#"
type Article = {
  slug: string;
  data: { title: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const articles = (await getCollection("articles")) as Article[];
  const sorted = [...articles].sort((a, b) => a.slug.localeCompare(b.slug));
  return { props: { articles: sorted } };
}

type Props = { articles: Article[] };

export default function ArticlesPage({ articles }: Props) {
  return (
    <html lang="en">
      <head><title>Articles</title></head>
      <body>
        <h1>INJECTED_ARTICLES_MARKER</h1>
        <ul>
          {articles.map((a) => (
            <li key={a.slug}>{a.data.title}</li>
          ))}
        </ul>
      </body>
    </html>
  );
}
"#;
    fs::write(root.join("pkg").join("articles-page.tsx"), articles_page)
        .expect("write pkg/articles-page.tsx");

    // 4. Write the preset that injects /preset-articles.
    let preset = r#"
export default {
  name: "consumer-preset-s5-hmr",
  setup({ injectRoute }) {
    // Static injected route that reads the `articles` collection.
    // A content edit under content/articles DOES live-refresh this route
    // via the per-tick snapshot rebuild + per-swap stale-mark (S5 / #1233).
    injectRoute("/preset-articles", "./pkg/articles-page.tsx");
  },
};
"#;
    fs::write(root.join("preset.mjs"), preset).expect("write HMR preset.mjs");

    edit_target
}

/// Subscribe to the dev server's SSE live-reload endpoint.
/// Must be called BEFORE the edit whose tick it is meant to observe.
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

/// S5 HMR contract: editing a watched collection file (content/articles/…)
/// causes a static injected route that reads that collection via
/// `getCollection(…)` to re-render and serve the updated content — without
/// a `zfb dev` restart.
///
/// ## Test level
///
/// Level 4 — real `zfb dev` process, real watcher, real V8 render,
/// real HTTP response.  Required because the HMR seam spans the watcher
/// tick, the content snapshot, the V8 bundle, and the lazy renderer; a
/// lower level cannot cover this end-to-end path.
///
/// ## Negative (node_modules restart-only)
///
/// The node_modules watch-exclusion contract is asserted at the unit-logic
/// level — see the module-level comment above and the two unit tests in
/// `crates/zfb/src/commands/dev.rs::tests`.  This test focuses on the
/// positive path only.
#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_injected_route_hmr_content_edit_refreshes() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[hmr_injected_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !node_available() {
        eprintln!("[hmr_injected_e2e] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir for HMR fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");

    // Set up the HMR-specific fixture contents (collection + injected page).
    let edit_target = setup_hmr_fixture(&root);

    // Symlink node_modules (same discipline as S3/S4 tests).
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

    let stdout_path = root.join(".zfb-dev-stdout-s5.log");
    let stderr_path = root.join(".zfb-dev-stderr-s5.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    let mut session = DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path: stdout_path.clone(),
        stderr_path: stderr_path.clone(),
    };
    // Keep the node_modules TempDir alive for the whole test.
    let _nm = nm_handle;

    let body = async {
        // ── Phase A: port discovery ──────────────────────────────────────
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
                        "[hmr_injected_e2e] `zfb dev` exited with a known-skip indicator; \
                         skipping.\n{}",
                        session.logs(),
                    );
                    return Outcome::Skipped;
                }
                panic!(
                    "`zfb dev` exited prematurely (status {status:?}) before the ready banner.\n{}",
                    session.logs(),
                );
            }
            if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
                break port;
            }
            assert!(
                boot_start.elapsed() < BOOT_DEADLINE,
                "`zfb dev` did not print a ready banner within {}s.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        let base = format!("http://localhost:{port}");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("build reqwest client");

        // ── Phase B: HTTP readiness ───────────────────────────────────────
        {
            let start = Instant::now();
            loop {
                if let Ok(resp) = client.get(format!("{base}/")).send().await {
                    if resp.status().as_u16() == 200 {
                        break;
                    }
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

        // ── Phase C: watcher-live handshake ───────────────────────────────
        //
        // Subscribe to SSE FIRST, then write fresh-named warmup content files
        // under `content/articles/` (the watched collection root) until the
        // first SSE event arrives — proving the watch stream is past its
        // startup dead window. Mirrors the dev_serve_e2e.rs handshake pattern.
        //
        // Warmup slugs (`__warmup-N`) never collide with the asserted article.
        {
            let sse = subscribe_sse(&client, &base).await;
            let stop = Arc::new(AtomicBool::new(false));
            let articles_dir = root.join("content").join("articles");
            let writer = {
                let articles_dir = articles_dir.clone();
                let stop = Arc::clone(&stop);
                tokio::spawn(async move {
                    let mut i = 0u32;
                    while !stop.load(Ordering::SeqCst) {
                        let warmup = articles_dir.join(format!("__warmup-{i}.md"));
                        let _ = fs::write(
                            &warmup,
                            format!("---\ntitle: warmup {i}\n---\n\nwarmup body.\n"),
                        );
                        i += 1;
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                })
            };
            let first = next_sse_event_name(sse, SSE_DEADLINE)
                .await
                .expect("read SSE stream during watcher-live handshake");
            stop.store(true, Ordering::SeqCst);
            let _ = writer.await;
            assert!(
                first.is_some(),
                "watcher never became live: no warmup-induced SSE event within {}s \
                 (watcher stream failed to start or lazy-stale gate regressed).\n{}",
                SSE_DEADLINE.as_secs(),
                session.logs(),
            );
        }

        // ── Phase D: baseline — /preset-articles serves V1 title ─────────
        //
        // Confirm the injected route renders at all before the HMR edit.
        poll_until_contains(
            &client,
            &format!("{base}/preset-articles"),
            "INJECTED_ARTICLES_MARKER",
            ROUTE_DEADLINE,
            "HMR baseline: /preset-articles serves the injected marker",
            &session,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/preset-articles"),
            "V1-HMR-ARTICLE-TITLE",
            ROUTE_DEADLINE,
            "HMR baseline: /preset-articles lists the V1 article title",
            &session,
        )
        .await;

        // ── Phase E: HMR positive — content edit refreshes injected route ─
        //
        // The mechanism (S3 / S5, epic #1228 §4):
        //   edit → watcher tick → assemble_and_bundle_dev rebuilds the content
        //   snapshot → new bundle bytes → Phase-B skip fails → full
        //   refresh_bundle_and_routes runs → note_table_swap bumps the
        //   generation → mark_injected_seeds_stale re-stales /preset-articles
        //   → the next GET re-renders with the fresh snapshot and serves the
        //   new title.
        //
        // The SSE `page` event proves a full tick fired (not just a file
        // create/warmup aliasing the signal — the V2 marker asserts the edit
        // actually reached the renderer).
        {
            let sse = subscribe_sse(&client, &base).await;
            fs::write(
                &edit_target,
                "---\ntitle: V2-HMR-ARTICLE-TITLE\n---\n\nFeature body updated.\n",
            )
            .expect("write V2 article title");

            // Wait for the SSE `page` event that signals the tick completed.
            let ev = next_sse_event_name(sse, SSE_DEADLINE)
                .await
                .expect("read SSE stream after content edit");
            assert_eq!(
                ev.as_deref(),
                Some("page"),
                "a content edit in a watched collection must broadcast an SSE `page` \
                 event — the HMR tick did not fire or the stale-gate regressed.\n{}",
                session.logs(),
            );

            // The authoritative assertion: the injected route now serves the
            // V2 title.  Polled (not single-shot) because the lazy renderer
            // writes on the first request AFTER the tick's stale-mark.
            poll_until_contains(
                &client,
                &format!("{base}/preset-articles"),
                "V2-HMR-ARTICLE-TITLE",
                ROUTE_DEADLINE,
                "HMR positive: /preset-articles must serve the V2 title after content edit",
                &session,
            )
            .await;

            // Belt-and-suspenders: the V1 title must be gone.
            let body = client
                .get(format!("{base}/preset-articles"))
                .send()
                .await
                .expect("GET /preset-articles after HMR edit")
                .text()
                .await
                .unwrap_or_default();
            assert!(
                !body.contains("V1-HMR-ARTICLE-TITLE"),
                "V1 article title must NOT appear after the HMR edit; \
                 the injected route is serving stale content.\nbody:\n{body}\n{}",
                session.logs(),
            );
        }

        Outcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(Outcome::Completed) | Ok(Outcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] HMR injected-route E2E did not finish within {}s — \
                 a hang, or the injected route never reflected the content edit. \
                 Process group {pgid} will be killed.\n{}",
                OVERALL_DEADLINE.as_secs(),
                session.logs(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// S6 (#1234) — CONFIRM gate: trailing-slash parity
// ---------------------------------------------------------------------------
//
// ## Contract being tested
//
// The render-on-request hook receives a **prefix-stripped, query-stripped**
// path (`render_hook.rs:99-104`).  `lookup_by_url` normalizes trailing slash,
// `index.html` duality, and percent-encoding before consulting `url_index`
// (`dev.rs:2703-2722`), and `output_path` is derived from the already-
// normalized URL, so `/preset-about` and `/preset-about/` resolve to the same
// `preset-about/index.html` — matching a normal static page.  For dynamic
// injected routes the same normalization runs inside `pattern_matches` /
// `render_stale_route`'s fallback, so `/preset-docs/a` and `/preset-docs/a/`
// also resolve identically.  No new normalization code is needed — this test
// proves the existing paths carry it correctly.
//
// ## Why a separate test
//
// S3/S4 focus on *presence* of the marker; they don't exercise the trailing-
// slash variant.  Adding trailing-slash assertions there would interleave
// concerns into already long test bodies.  A dedicated test with a clear name
// is easier for the manager to identify and run during the confirm gate.

/// Write a preset that injects both a static and a dynamic route, with no
/// user-page collision, for trailing-slash parity checks only.
fn write_trailing_slash_preset(root: &Path) {
    let preset = r#"
export default {
  name: "consumer-preset-s6-trailing-slash",
  setup({ injectRoute }) {
    // Static: /preset-about (URL == pattern, seeded into url_index at boot).
    injectRoute("/preset-about", "./pkg/about.tsx");
    // Dynamic: /preset-docs/[slug] (request-time synthetic entry).
    injectRoute("/preset-docs/[slug]", "./pkg/docs.tsx");
  },
};
"#;
    fs::write(root.join("preset.mjs"), preset).expect("write trailing-slash preset.mjs");
}

/// S6 CONFIRM gate: trailing-slash parity for both static and dynamic
/// injected routes under a real `zfb dev` (epic #1228, S6 #1234).
///
/// Asserts that a trailing slash is transparent:
///
/// - `GET /preset-about` and `GET /preset-about/` both return 200 with
///   `CONSUMER_PRESET_ABOUT_MARKER` — the static injected route is served
///   from the same `html_root/preset-about/index.html` for both URL forms.
/// - `GET /preset-docs/getting-started` and
///   `GET /preset-docs/getting-started/` both return 200 with the docs
///   marker — the dynamic injected route's `output_path` derivation runs on
///   the already-normalized URL, so the trailing-slash variant hits the same
///   cached render.
///
/// ## Test level
///
/// Level 4 — real `zfb dev` process, real HTTP requests. Required because
/// trailing-slash normalization spans the HTTP layer (`lookup_by_url`,
/// `url_index_lookup_keys`, `output_path` derivation) in a way no lower
/// level can exercise end-to-end.
#[tokio::test(flavor = "multi_thread")]
async fn dev_e2e_trailing_slash_parity() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[trailing_slash_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !node_available() {
        eprintln!("[trailing_slash_e2e] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir for trailing-slash fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");
    write_trailing_slash_preset(&root);

    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

    let stdout_path = root.join(".zfb-dev-stdout-s6.log");
    let stderr_path = root.join(".zfb-dev-stderr-s6.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    let mut session = DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path: stdout_path.clone(),
        stderr_path: stderr_path.clone(),
    };
    // Keep the node_modules TempDir alive for the whole test.
    let _nm = nm_handle;

    let body = async {
        // Phase A: discover the port from the ready banner.
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
                        "[trailing_slash_e2e] `zfb dev` exited with a known-skip indicator; \
                         skipping.\n{}",
                        session.logs(),
                    );
                    return Outcome::Skipped;
                }
                panic!(
                    "`zfb dev` exited prematurely (status {status:?}) before the ready banner.\n{}",
                    session.logs(),
                );
            }
            if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
                break port;
            }
            assert!(
                boot_start.elapsed() < BOOT_DEADLINE,
                "`zfb dev` did not print a ready banner within {}s.\n{}",
                BOOT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        let base = format!("http://localhost:{port}");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            // Do NOT follow redirects: a trailing-slash redirect (301/308)
            // instead of a 200 would indicate the dev server is not serving
            // the file at the trailing-slash URL directly, which is a
            // regression in `lookup_by_url` normalization.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build reqwest client");

        // Phase B: readiness — GET / answers 200.
        {
            // Use a separate client that follows redirects for the readiness
            // check (the root URL itself may redirect or be served directly).
            let ready_client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .build()
                .expect("build ready client");
            let start = Instant::now();
            loop {
                if let Ok(resp) = ready_client.get(format!("{base}/")).send().await {
                    if resp.status().as_u16() == 200 {
                        break;
                    }
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

        // --- Trailing-slash parity: STATIC injected route ---

        // Without trailing slash (canonical form).
        poll_until_contains(
            &client,
            &format!("{base}/preset-about"),
            "CONSUMER_PRESET_ABOUT_MARKER",
            ROUTE_DEADLINE,
            "static injected /preset-about (no trailing slash) renders",
            &session,
        )
        .await;

        // With trailing slash — must serve the same marker, not redirect.
        // `lookup_by_url` normalizes trailing slash before consulting
        // `url_index` (`dev.rs:2703-2722`), so this must be a 200 with
        // the same content, not a 301/308.  The no-redirect `client` makes
        // a 3xx status bypass the `status == 200` guard below and fall
        // through to an explicit redirect-regression panic.
        poll_until_200_not_redirect(
            &client,
            &format!("{base}/preset-about/"),
            "CONSUMER_PRESET_ABOUT_MARKER",
            ROUTE_DEADLINE,
            "static injected /preset-about/ (trailing slash)",
            &session,
        )
        .await;

        // --- Trailing-slash parity: DYNAMIC injected route ---

        // Without trailing slash.
        poll_until_contains(
            &client,
            &format!("{base}/preset-docs/getting-started"),
            "CONSUMER_PRESET_DOCS_MARKER_getting-started",
            ROUTE_DEADLINE,
            "dynamic injected /preset-docs/getting-started (no trailing slash) renders",
            &session,
        )
        .await;

        // With trailing slash — the dynamic fallback derives `output_path`
        // from the normalized URL (`build_output_path_for_resolved_url` runs
        // on the already-stripped path), so both variants produce the same
        // `preset-docs/getting-started/index.html` and serve the same HTML.
        poll_until_200_not_redirect(
            &client,
            &format!("{base}/preset-docs/getting-started/"),
            "CONSUMER_PRESET_DOCS_MARKER_getting-started",
            ROUTE_DEADLINE,
            "dynamic injected /preset-docs/getting-started/ (trailing slash)",
            &session,
        )
        .await;

        Outcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(Outcome::Completed) | Ok(Outcome::Skipped) => {}
        Err(_) => {
            panic!(
                "[watchdog] trailing-slash parity dev E2E did not finish within {}s — \
                 a hang, or an injected route did not serve identical content for its \
                 trailing-slash variant. Process group {pgid} will be killed.\n{}\n\
                 --- stdout ---\n{}\n--- stderr ---\n{}",
                OVERALL_DEADLINE.as_secs(),
                "check logs below",
                read_log(&stdout_path),
                read_log(&stderr_path),
            );
        }
    }
}
