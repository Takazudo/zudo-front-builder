//! Static injected-route render E2E — real `zfb dev` serving package-owned
//! `injectRoute(...)` static routes (epic #1228, S3 #1231).
//!
//! ## Why this test exists
//!
//! S2 (#1230) put the synthesized injected-route modules into the dev
//! bundle; S3 seeds the STATIC injected routes (URL == pattern) into the
//! dev route universe so they actually render and serve under `zfb dev`.
//! No crate-level test runs the real `zfb dev` binary against an injected
//! route — `build_package_routes_consumer.rs` proves only the `zfb build`
//! path. This harness boots a real `zfb dev` over the
//! `package-routes-consumer` fixture and asserts the static injected route
//! serves real HTML over HTTP:
//!
//! - `GET /preset-about` → 200 with `CONSUMER_PRESET_ABOUT_MARKER` and the
//!   relative-import content (`Consumer Demo`, from `pkg/site-meta.ts`) —
//!   proof the static injected route renders through the lazy adapter into
//!   `html_root` and is served from disk.
//! - `GET /preset-virtual` → 200 with the marker — proof a `virtual:`
//!   -importing static injected entrypoint renders too (the preset
//!   registers the `virtual:` module via `addVirtualModule`).
//! - **Precedence:** a preset that ALSO injects `/guide` (which the user
//!   owns via `pages/guide.tsx`) must NOT shadow the user page —
//!   `GET /guide` serves `CONSUMER_USER_GUIDE_MARKER`, never the injected
//!   marker (user `pages/` wins).
//! - **Dev `/` reservation unchanged:** `GET /` serves the user home
//!   (`CONSUMER_USER_HOME_MARKER`); no injected route claims root in dev.
//!
//! ## Determinism / spawn discipline
//!
//! Mirrors `dev_serve_e2e.rs`: ephemeral `--port 0`, own process group,
//! stdout/stderr captured to files (never pipes), `DevServerGuard`
//! group-kills on Drop, and an overall wall-clock watchdog. Readiness is
//! an HTTP poll of `GET /` after the ready banner is parsed only to learn
//! the port. The injected routes are STATIC, so they are seeded + stale-
//! marked at boot and render on first request — no watcher edit needed.
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
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Overall wall-clock watchdog. A clean run is boot (V8 + esbuild, slow in
/// debug) plus a handful of request-time renders.
const OVERALL_DEADLINE: Duration = Duration::from_secs(280);

/// Deadline for the ready banner + `GET /` answering 200.
const BOOT_DEADLINE: Duration = Duration::from_secs(120);

/// Per-route deadline for the served marker to appear.
const ROUTE_DEADLINE: Duration = Duration::from_secs(60);

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
/// that injects `[slug]`, `[...rest]`, and `[[...chapter]]` patterns. The test
/// GETs concrete URLs that match each pattern and asserts:
///
/// - The response is 200 with the route's marker — proof Hono resolved params
///   inside V8 and the lazy adapter's synthetic entry rendered and was served.
/// - Different concrete URLs for the same pattern render correctly (slug
///   variance).
/// - The `[[...chapter]]` bare-prefix case (`GET /guide`) matches.
/// - The root page (`GET /`) is NOT hijacked by the `[[...chapter]]`-rooted
///   optional-catchall injected at `/guide/[[...chapter]]` — the user home
///   still wins.
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
