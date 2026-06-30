//! Wave-3 ACCEPTANCE gate for bug #1284 (epic #1285), authored during the
//! Wave-1 diagnosis (#1286). Level 4 — real `zfb dev` edit→serve loop.
//!
//! These scenarios reproduce the THREE symptoms that the existing
//! `dev_serve_e2e.rs` scenario 4 does NOT cover (that scenario edits a
//! *directly-imported* `components/**` file, which already re-renders today via
//! the orchestrator's blunt `PageSelection::All` fallback):
//!
//! - **A** — editing a component under `src/**` (NOT a watch root) does not
//!   re-render the consuming route. Acceptance: after the fix, editing
//!   `src/components/*.tsx` makes the route serve the new marker on next
//!   request.
//! - **B** — editing a transitively-imported CSS file (incl. a symlinked
//!   workspace dep reached via `@import`) does not refresh `/assets/styles.css`.
//!   Acceptance: after the fix, editing the imported CSS makes
//!   `/assets/styles.css` serve the new bytes.
//! - **C** — a NEW Tailwind utility class added inside a component is not
//!   emitted into `/assets/styles.css` until the CSS entry is touched.
//!   Acceptance: after the fix, the new class appears in `/assets/styles.css`
//!   without touching the CSS entry.
//!
//! ## D3 — the observable (locked by #1286)
//!
//! Under the lazy dev model (`lazy_render_tick` marks routes STALE; it does NOT
//! eagerly write), the test observable is **served-HTML / served-asset on the
//! NEXT request**, polled via `poll_until_*` — NOT an eager disk write. The SSE
//! `page` event is asserted as a secondary signal (it fires via the
//! `pages_stale` gate), exactly as `dev_serve_e2e.rs` scenario 4 does. For the
//! CSS symptoms the observable is the body of `GET /assets/styles.css`.
//!
//! ## Status — these are STUBS, intentionally `#[ignore]`d
//!
//! They are tagged `#[ignore = "Level-4 e2e: implement+run on a V8 host — #1290"]` so they neither block the
//! T1 gate nor force a 15-30 min V8 first-compile in this diagnosis wave. The
//! Wave-3 author un-ignores them and wires them into the shared `dev_serve_e2e`
//! harness (reusing `spawn_dev` / `boot_and_handshake` / `poll_until_contains`
//! / `subscribe_sse`), OR fills in the `todo!()` bodies below against a local
//! copy of those helpers. They are kept as a separate file so the acceptance
//! contract is reviewable independently of the fix.
//!
//! Falsifiability is noted per scenario: revert the corresponding fix and the
//! served-on-next-request assertion times out on the OLD marker.

// ---------------------------------------------------------------------------
// Shared harness (local copy of the helpers from dev_serve_e2e.rs, adapted
// for these three scenarios). The helpers are private to that file's binary;
// Rust integration tests are separate binaries and cannot import each other's
// private items. zfb-test-utils (already a dev-dep) provides locate_esbuild,
// next_sse_event_name, and the zfb_binary! macro.
// ---------------------------------------------------------------------------

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary};

// Serialise the three tests: each boots a full V8 + esbuild + Tailwind dev
// session; running them concurrently would double/triple memory and produce
// flaky boot deadlines.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Overall wall-clock watchdog for each test.
const OVERALL_DEADLINE: Duration = Duration::from_secs(300);

/// Deadline for the dev server to print its ready banner + first `GET /` 200.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-scenario deadline for the marker to appear in the served response.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);

/// Deadline for the watcher-live handshake + per-scenario SSE tick signal.
const SSE_DEADLINE: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Harness types and helpers (mirrors dev_serve_e2e.rs)
// ---------------------------------------------------------------------------

fn base_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-loop-basic")
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

/// Extract the port from the dev ready banner (`→ ready on http://localhost:PORT/`).
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

/// Spawn `zfb dev --port 0` over the already-prepared fixture root, in its own
/// process group, with stdout/stderr captured to files. Callers copy and extend
/// the fixture BEFORE calling this.
fn spawn_dev(root: PathBuf, esbuild: &Path, extra_env: &[(&str, &str)]) -> DevSession {
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
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
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

/// Poll `url` until the body contains `needle` with status 200.
async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last_observation = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == 200 && body.contains(needle) {
                    return;
                }
                last_observation = format!("status {status}, body:\n{body}");
            }
            Err(e) => last_observation = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve a body containing {needle:?} within {}s.\n\
         Last observation: {last_observation}\n{}",
        deadline.as_secs(),
        session.logs(),
    );
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

enum ScenarioOutcome {
    Completed,
    /// The binary exited with a known environmental skip indicator (no V8 /
    /// no esbuild / no Tailwind) — skip without failing.
    Skipped,
}

/// Phases A-C: ready-banner port discovery, HTTP readiness, and the
/// watcher-live handshake. Returns `None` when the binary exited with a
/// recognized environmental skip indicator, otherwise `(base_url, client)`.
async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
    // Phase A: discover the ephemeral port from the ready banner.
    let boot_start = Instant::now();
    let port = loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path)
            );
            if combined.contains("embed_v8")
                || combined.contains("no esbuild")
                || combined.contains("no tailwind")
                || combined.contains("tailwindcss") && combined.contains("not found")
            {
                eprintln!(
                    "[dep_inval_e2e] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild/tailwind unavailable); skipping test.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the \
                 ready banner.\n{}",
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

    // Phase B: HTTP readiness — poll GET / until 200.
    {
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
    }

    // Phase C: watcher-live handshake.
    //
    // Subscribe to SSE FIRST, then write fresh-named warmup content files
    // until the first SSE event arrives — proving the watch stream is past its
    // startup dead window. Mirrors dev_serve_e2e.rs.
    {
        let sse = subscribe_sse(&client, &base).await;
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let root = session.root.to_path_buf();
            let stop = Arc::clone(&stop);
            tokio::spawn(async move {
                let mut i = 0u32;
                while !stop.load(Ordering::SeqCst) {
                    let warmup = root.join(format!("content/posts/__warmup-{i}.md"));
                    let _ = fs::write(
                        &warmup,
                        format!("---\ntitle: warmup {i}\n---\n\nwarmup body {i}\n"),
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
            "watcher never became live: no warmup-induced SSE event within {}s\n{}",
            SSE_DEADLINE.as_secs(),
            session.logs(),
        );
    }

    Some((base, client))
}

// ---------------------------------------------------------------------------
// SYMPTOM A
// ---------------------------------------------------------------------------

/// SYMPTOM A acceptance — editing `src/components/Widget.tsx` re-renders the
/// route that imports it. Observable (D3): `GET /` serves the new marker on the
/// next request after the edit; an SSE `page` event fires.
///
/// Falsifiability: with `src/` still outside the watch roots (or the All-only
/// selection), no tick fires / the bundle is not refreshed and the route keeps
/// serving the old marker until timeout.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Level-4 e2e: implement+run on a V8 host — #1290"]
async fn e2e_src_component_edit_rerenders_route() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dep_inval_e2e symptom-A] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    // Build a dev-loop-basic-derived fixture that has `src/components/Widget.tsx`
    // imported by `pages/index.tsx`. The `src/` directory is in DEFAULT_WATCH_ROOTS
    // after the #1284 fix, so an edit to `src/**` fires a watcher tick and
    // re-renders the consuming route.
    let tmp = tempfile::tempdir().expect("create tempdir for symptom-A fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&base_fixture_dir(), &root).expect("copy dev-loop-basic fixture");

    // Add src/components/Widget.tsx with a unique initial marker.
    fs::create_dir_all(root.join("src").join("components")).expect("create src/components/");
    fs::write(
        root.join("src/components/Widget.tsx"),
        "export function Widget() {\n  \
         return <p data-testid=\"widget\">WIDGET-MARKER-V1</p>;\n}\n",
    )
    .expect("write src/components/Widget.tsx");

    // Extend pages/index.tsx to import and render Widget from src/.
    // We completely replace the file so the import is stable from boot.
    fs::write(
        root.join("pages/index.tsx"),
        r#"
import { SharedNote } from "../components/shared-note";
import { Widget } from "../src/components/Widget";

type Post = {
  slug: string;
  data: { title: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  const sorted = [...posts].sort((a, b) => a.slug.localeCompare(b.slug));
  return { props: { posts: sorted } };
}

type Props = {
  posts: Post[];
};

export default function HomePage({ posts }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-loop-basic fixture</title>
      </head>
      <body>
        <h1>dev-loop-basic</h1>
        <SharedNote />
        <Widget />
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>
              <a href={`/posts/${post.slug}`}>{post.data.title}</a>
            </li>
          ))}
        </ul>
      </body>
    </html>
  );
}
"#,
    )
    .expect("write extended pages/index.tsx");

    let mut session = spawn_dev(root, &esbuild, &[]);
    let pgid = session.guard.pgid;

    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };

        // Baseline: `GET /` serves the initial Widget marker.
        poll_until_contains(
            &client,
            &format!("{base}/"),
            "WIDGET-MARKER-V1",
            SCENARIO_DEADLINE,
            "symptom-A baseline: GET / serves Widget V1 marker",
            &session,
        )
        .await;

        // ── THE EDIT ──────────────────────────────────────────────────────
        // Subscribe to SSE BEFORE the edit so we catch the watcher tick.
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("src/components/Widget.tsx"),
            "export function Widget() {\n  \
             return <p data-testid=\"widget\">WIDGET-MARKER-V2</p>;\n}\n",
        )
        .expect("edit src/components/Widget.tsx");

        // The tick must broadcast an SSE `page` event (via pages_stale gate,
        // exactly as dev_serve_e2e.rs scenario 4 does).
        let ev = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE stream after src/components/Widget.tsx edit");
        assert_eq!(
            ev.as_deref(),
            Some("page"),
            "editing src/components/Widget.tsx must broadcast an SSE `page` event \
             — the #1284 fix added `src/` to DEFAULT_WATCH_ROOTS so the tick fires. \
             Without the fix no tick fires at all.\n{}",
            session.logs(),
        );

        // The authoritative assertion (D3): GET / on the NEXT request serves
        // the new marker. In lazy mode the poll's first 200-bearing iteration is
        // itself that triggering request, so polling is the correct test shape.
        //
        // Falsifiability: revert the #1284 `src/` watch-root addition. No tick
        // fires, no stale mark is set, and the route keeps serving the old marker
        // until this assertion times out.
        poll_until_contains(
            &client,
            &format!("{base}/"),
            "WIDGET-MARKER-V2",
            SCENARIO_DEADLINE,
            "symptom-A: GET / must serve the new Widget V2 marker after src/ component edit",
            &session,
        )
        .await;

        // Belt-and-suspenders: V1 must be gone from the served body.
        let served = client
            .get(format!("{base}/"))
            .send()
            .await
            .expect("GET / after Widget edit")
            .text()
            .await
            .unwrap_or_default();
        assert!(
            !served.contains("WIDGET-MARKER-V1"),
            "GET / still contains the old WIDGET-MARKER-V1 after the src/ edit — \
             the route was not re-rendered (lazy stale → request-time render path \
             regressed).\n{}",
            session.logs(),
        );

        ScenarioOutcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] symptom-A e2e did not finish within {}s — hang or \
             src/ edit never re-rendered the consuming route. \
             Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

// ---------------------------------------------------------------------------
// SYMPTOM B
// ---------------------------------------------------------------------------

/// SYMPTOM B acceptance — editing a transitively-imported CSS file (a local
/// `@import './tokens.css'` and a symlinked workspace dep `@import
/// '@scope/design-system'`) refreshes `/assets/styles.css`. Observable (D3):
/// `GET /assets/styles.css` serves the new bytes on the next request.
///
/// Falsifiability: without the resolved-`@import` watch registration, the
/// symlinked dep edit is observed by nobody and `/assets/styles.css` stays
/// stale until timeout.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Level-4 e2e: implement+run on a V8 host — #1290"]
async fn e2e_transitive_css_import_refreshes_stylesheet() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dep_inval_e2e symptom-B] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    // Build the fixture.
    let tmp = tempfile::tempdir().expect("create tempdir for symptom-B fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&base_fixture_dir(), &root).expect("copy dev-loop-basic fixture");

    // Create a "real" package directory OUTSIDE the project root. The dev
    // server resolves `@import '@scope/design-system'` through the symlinked
    // node_modules entry; canonicalize() follows the symlink to the real file
    // outside the project tree, which is the actual watcher target (#1288 D4).
    let pkg_dir = tempfile::tempdir().expect("create tempdir for fake @scope/design-system");
    let pkg_real_path = pkg_dir.path().canonicalize().expect("canonicalize pkg dir");
    // The package provides a single CSS file. Boot-time content: a known
    // V1 CSS custom property.
    let pkg_css_path = pkg_real_path.join("index.css");
    fs::write(
        &pkg_css_path,
        "/* @scope/design-system v1 */\n:root { --ds-color: #V1DS; }\n",
    )
    .expect("write @scope/design-system index.css");
    // Provide a package.json with `style` pointing at index.css so the
    // bare-package CSS resolver picks it up (#1288 css_imports).
    fs::write(
        pkg_real_path.join("package.json"),
        "{\"name\":\"@scope/design-system\",\"version\":\"1.0.0\",\"style\":\"index.css\"}\n",
    )
    .expect("write @scope/design-system package.json");

    // Symlink node_modules/@scope/design-system → the real pkg dir.
    // This mirrors a pnpm/yarn workspace symlink: `node_modules` points
    // outside the project tree, notify doesn't follow it, and without #1288
    // the real path is never registered as a watch target.
    let nm_scope_dir = root.join("node_modules").join("@scope");
    fs::create_dir_all(&nm_scope_dir).expect("create node_modules/@scope/");
    std::os::unix::fs::symlink(&pkg_real_path, nm_scope_dir.join("design-system"))
        .expect("symlink node_modules/@scope/design-system");

    // `styles/tokens.css` — the LOCAL transitive import.
    // The #1288 fix also registers this file as a watch target because it is
    // in the `@import` graph of the CSS entry.
    fs::create_dir_all(root.join("styles")).expect("create styles/");
    fs::write(
        root.join("styles/tokens.css"),
        "/* local tokens v1 */\n:root { --token-spacing: 4px; /* V1-TOKEN */ }\n",
    )
    .expect("write styles/tokens.css");

    // `styles/global.css` — the project CSS entry (zfb convention).
    // BOTH `@import` declarations must be present AT BOOT (see module docs:
    // mid-session-added imports are deliberately unregistered — #1293).
    fs::write(
        root.join("styles/global.css"),
        "@import \"tailwindcss\";\n\
         @import './tokens.css';\n\
         @import '@scope/design-system';\n\
         \n\
         body { font-family: sans-serif; }\n",
    )
    .expect("write styles/global.css");

    let mut session = spawn_dev(root, &esbuild, &[]);
    let pgid = session.guard.pgid;

    // `pkg_dir` must outlive the session — the symlink's real target must
    // remain on disk for the watcher registration to be meaningful.
    let _pkg_dir = pkg_dir;

    let css_url_fn = |base: &str| format!("{base}/assets/styles.css");

    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };

        // Baseline: GET /assets/styles.css serves a 200 (the boot CSS bundle).
        // We only check status here because the exact content depends on the
        // Tailwind binary's output — we don't assert a specific V1 token yet.
        {
            let start = Instant::now();
            loop {
                match client.get(css_url_fn(&base)).send().await {
                    Ok(resp) if resp.status().as_u16() == 200 => break,
                    _ => {}
                }
                assert!(
                    start.elapsed() < SCENARIO_DEADLINE,
                    "GET /assets/styles.css never answered 200 within {}s after boot.\n{}",
                    SCENARIO_DEADLINE.as_secs(),
                    session.logs(),
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }

        // ── EDIT 1: local transitive import (tokens.css) ──────────────────
        // The #1288 fix registers `styles/tokens.css` (resolved from the
        // `@import './tokens.css'` in `styles/global.css`) as a watch target.
        // Editing it fires a PathClass::Style tick → rerun_css refreshes the
        // asset.
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("styles/tokens.css"),
            "/* local tokens v2 */\n:root { --token-spacing: 8px; /* V2-TOKEN */ }\n",
        )
        .expect("edit styles/tokens.css");

        // SSE signal: a Style tick does not mark pages stale (no SSE `page`
        // event), but it does update the CSS asset bytes. We do NOT assert SSE
        // here — the observable is the served CSS bytes.
        // Allow the tick to settle before polling (the SSE connection gives us
        // the tick signal for page events; for CSS-only ticks we just poll).
        // The `next_sse_event_name` future is consumed but the event type may
        // be anything or nothing — we only care about the CSS asset body.
        let _ = next_sse_event_name(sse, SSE_DEADLINE).await;

        // The authoritative assertion: the new V2-TOKEN comment appears in the
        // served stylesheet. Tailwind passes the synthesised CSS through its
        // pipeline, so the custom property value appears in the output.
        //
        // Falsifiability: without the #1288 `@import`-graph watch registration,
        // tokens.css is not watched; no tick fires; the asset stays stale and
        // this assertion times out.
        poll_until_contains(
            &client,
            &css_url_fn(&base),
            "V2-TOKEN",
            SCENARIO_DEADLINE,
            "symptom-B (local import): /assets/styles.css must reflect tokens.css edit",
            &session,
        )
        .await;

        // ── EDIT 2: symlinked workspace dep (@scope/design-system) ────────
        // The #1288 fix canonicalises the `@scope/design-system` node_modules
        // symlink to the real file outside the project root and registers that
        // real path as an extra watch target. Editing the real file fires a tick.
        let sse2 = subscribe_sse(&client, &base).await;
        fs::write(
            &pkg_css_path,
            "/* @scope/design-system v2 */\n:root { --ds-color: #V2DS; }\n",
        )
        .expect("edit @scope/design-system index.css (real canonical path)");

        let _ = next_sse_event_name(sse2, SSE_DEADLINE).await;

        // The new V2DS marker must appear in the served stylesheet.
        //
        // Falsifiability: without canonicalisation + extra-watch-target
        // registration, the symlinked real file is never watched; no tick fires;
        // this assertion times out.
        poll_until_contains(
            &client,
            &css_url_fn(&base),
            "V2DS",
            SCENARIO_DEADLINE,
            "symptom-B (symlinked dep): /assets/styles.css must reflect @scope/design-system edit",
            &session,
        )
        .await;

        ScenarioOutcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] symptom-B e2e did not finish within {}s — hang or \
             transitive CSS edit never refreshed /assets/styles.css. \
             Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

// ---------------------------------------------------------------------------
// SYMPTOM C
// ---------------------------------------------------------------------------

/// SYMPTOM C acceptance — a NEW utility class added inside a component is
/// emitted into `/assets/styles.css` WITHOUT touching the CSS entry. Observable
/// (D3): `GET /assets/styles.css` contains the generated rule for the new class
/// (e.g. `gap-x-hgap-2xs`, `xl:grid-cols-[2.35fr_1fr]`) on the next request.
///
/// Falsifiability: without the Module→`mark_css` re-scan AND `src/` in the scan
/// roots, the class never enters the content scan and the stylesheet never
/// gains the rule.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Level-4 e2e: implement+run on a V8 host — #1290"]
async fn e2e_new_utility_class_in_component_is_emitted() {
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dep_inval_e2e symptom-C] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    // Build the fixture.
    let tmp = tempfile::tempdir().expect("create tempdir for symptom-C fixture");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&base_fixture_dir(), &root).expect("copy dev-loop-basic fixture");

    // `styles/global.css` — project CSS entry with the Tailwind import so the
    // CSS pipeline actually runs Tailwind's content scan and emits utility
    // class rules. The fixture starts with one known class (`font-bold`) so we
    // can confirm the Tailwind pipeline ran at boot before asserting the new
    // one appears after the component edit.
    fs::create_dir_all(root.join("styles")).expect("create styles/");
    fs::write(
        root.join("styles/global.css"),
        "@import \"tailwindcss\";\n\
         \n\
         /* symptom-C fixture: Tailwind content scan baseline */\n\
         body { font-family: sans-serif; }\n",
    )
    .expect("write styles/global.css");

    // `src/components/CardWidget.tsx` — a component that starts with a
    // commonly-generated Tailwind class (`font-bold`) at boot. The test edits
    // it mid-session to add the NEVER-BEFORE-SEEN class `gap-x-hgap-2xs`.
    // This class is chosen because:
    //   - It is a legitimate Tailwind v4 utility (gap-x with a custom value).
    //   - It looks distinctive enough that a substring match in the CSS body is
    //     unambiguous (`.gap-x-hgap-2xs`).
    // Without the #1284 fix the content scan is not re-run on Module ticks,
    // so the new class never reaches the generated stylesheet.
    fs::create_dir_all(root.join("src").join("components")).expect("create src/components/");
    fs::write(
        root.join("src/components/CardWidget.tsx"),
        "export function CardWidget() {\n  \
         return <div class=\"font-bold\">CardWidget V1</div>;\n}\n",
    )
    .expect("write src/components/CardWidget.tsx");

    // Wire CardWidget into `pages/index.tsx` so the component IS part of the
    // project's source tree and is included in the Tailwind content scan.
    fs::write(
        root.join("pages/index.tsx"),
        r#"
import { SharedNote } from "../components/shared-note";
import { CardWidget } from "../src/components/CardWidget";

type Post = {
  slug: string;
  data: { title: string };
};

export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const posts = (await getCollection("posts")) as Post[];
  const sorted = [...posts].sort((a, b) => a.slug.localeCompare(b.slug));
  return { props: { posts: sorted } };
}

type Props = {
  posts: Post[];
};

export default function HomePage({ posts }: Props) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <title>dev-loop-basic fixture</title>
      </head>
      <body>
        <h1>dev-loop-basic</h1>
        <SharedNote />
        <CardWidget />
        <ul>
          {posts.map((post) => (
            <li key={post.slug}>
              <a href={`/posts/${post.slug}`}>{post.data.title}</a>
            </li>
          ))}
        </ul>
      </body>
    </html>
  );
}
"#,
    )
    .expect("write extended pages/index.tsx");

    let mut session = spawn_dev(root, &esbuild, &[]);
    let pgid = session.guard.pgid;

    let css_url_fn = |base: &str| format!("{base}/assets/styles.css");

    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };

        // Baseline: GET /assets/styles.css serves a 200.
        // We do not assert `font-bold` specifically because Tailwind v4's
        // generated output format varies (it may inline or merge utilities
        // differently). The key assertion is that a NEW class added mid-session
        // eventually appears without touching the CSS entry.
        {
            let start = Instant::now();
            loop {
                match client.get(css_url_fn(&base)).send().await {
                    Ok(resp) if resp.status().as_u16() == 200 => break,
                    _ => {}
                }
                assert!(
                    start.elapsed() < SCENARIO_DEADLINE,
                    "GET /assets/styles.css never answered 200 within {}s after boot.\n{}",
                    SCENARIO_DEADLINE.as_secs(),
                    session.logs(),
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }

        // ── THE EDIT ──────────────────────────────────────────────────────
        // Add `gap-x-hgap-2xs` to the component WITHOUT touching styles/global.css.
        // Before #1284: the Module tick does NOT re-run the Tailwind content scan,
        // so this new class never enters the stylesheet.
        // After  #1284: a Module tick with a `.tsx` change fires mark_css which
        // triggers a Tailwind re-scan, surfacing the new class.
        //
        // We subscribe to SSE to observe the tick, but a Module tick may or may
        // not emit a `page` event depending on the route fan-out. The authoritative
        // assertion is the served CSS body.
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("src/components/CardWidget.tsx"),
            "export function CardWidget() {\n  \
             return <div class=\"font-bold gap-x-hgap-2xs\">CardWidget V2</div>;\n}\n",
        )
        .expect("edit src/components/CardWidget.tsx to add gap-x-hgap-2xs");

        // Wait for any SSE event from the tick (page or css — either signals
        // the tick completed). Ignore timeout: the CSS poll below is the
        // authoritative gate.
        let _ = next_sse_event_name(sse, SSE_DEADLINE).await;

        // The authoritative assertion (D3): GET /assets/styles.css on the next
        // request must contain a CSS rule for `gap-x-hgap-2xs`. Tailwind v4
        // emits utility classes as `.gap-x-hgap-2xs { … }` or within a layer.
        // We match the class selector fragment to stay output-format-agnostic.
        //
        // Falsifiability: revert the #1284 Module→mark_css re-scan addition.
        // The Tailwind content scan is not re-run on this tick; the class
        // is never seen; this assertion times out on the old CSS body.
        poll_until_contains(
            &client,
            &css_url_fn(&base),
            "gap-x-hgap-2xs",
            SCENARIO_DEADLINE,
            "symptom-C: /assets/styles.css must emit the new gap-x-hgap-2xs rule \
             after the component edit (without touching the CSS entry)",
            &session,
        )
        .await;

        ScenarioOutcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] symptom-C e2e did not finish within {}s — hang or \
             new Tailwind class was never emitted into /assets/styles.css after \
             component edit. Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}
