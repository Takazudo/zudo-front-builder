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
//! ## Status — fully implemented, gated `heavy:` (issue #1290 is closed)
//!
//! The Wave-3 author wired all three scenarios into a local copy of the
//! `dev_serve_e2e` harness (`spawn_dev` / `boot_and_handshake` /
//! `poll_until_contains` / `subscribe_sse`) — these are no longer stubs.
//! They stay `#[ignore]`d (tagged `heavy:`, see crates/CLAUDE.md's taxonomy)
//! because each scenario boots a real `zfb dev` server (esbuild + embedded
//! V8 + Tailwind for symptom C) and polls it over HTTP: too slow, and too
//! reliant on a free port, for the T1 PR gate. Run locally with
//! `cargo test -p zfb --test dev_dep_invalidation_1284_e2e -- --ignored`.
//! They are kept as a separate file so the acceptance contract is
//! reviewable independently of the fix.
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

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

// Serialise the three tests: each boots a full V8 + esbuild + Tailwind dev
// session; running them concurrently would double/triple memory and produce
// flaky boot deadlines. Each test also acquires `CrossBinaryE2eLock` BEFORE
// this mutex to serialize against sibling e2e binaries (issue #1339) — see
// `zfb-test-utils/src/cross_binary_lock.rs` for the lock-ordering rationale.
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

/// The workspace root — two levels above `crates/zfb` — which owns `packages/`
/// and the pnpm store at `node_modules/.pnpm`.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root two levels above crates/zfb")
        .to_path_buf()
}

/// Resolve a package inside the workspace pnpm store
/// (`node_modules/.pnpm/<name>@<ver>*/node_modules/<name>`). Returns the
/// lowest-sorted match so the choice is deterministic across peer-injected
/// variants. The `<name>@` prefix is exact enough that e.g. `preact@` does not
/// match `preact-render-to-string@…`.
fn pnpm_store_pkg(ws: &Path, name: &str) -> Option<PathBuf> {
    let store = ws.join("node_modules").join(".pnpm");
    let prefix = format!("{name}@");
    let mut hits: Vec<PathBuf> = fs::read_dir(&store)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|s| s.starts_with(&prefix))
                .unwrap_or(false)
        })
        .map(|e| e.path().join("node_modules").join(name))
        .filter(|p| p.is_dir())
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// Provision a COMPLETE project `node_modules` mirroring the binary-embedded
/// vendor snapshot (`@takazudo/zfb`, `@takazudo/zfb-runtime`, `preact`,
/// `preact-render-to-string`, `hono`) so esbuild and the SSR host can resolve
/// the framework.
///
/// Why this is required (symptom B only): `detect_project_node_modules`
/// (crates/zfb/src/commands/build.rs) selects `<root>/node_modules` the moment
/// it exists. This fixture MUST create `node_modules/@scope/…` for the
/// symlinked workspace-dep `@import`, which flips that detection ON and SHADOWS
/// the binary-embedded vendor fallback (render_pipeline.rs
/// `embedded_node_modules`, wired in commands/bundler_input.rs). Left partial,
/// esbuild cannot resolve `@takazudo/*` / `preact*`, so the SSR boot times out.
///
/// esbuild runs with `--preserve-symlinks` OFF for a detected project
/// `node_modules`, so it canonicalises each symlink to its real workspace path
/// and resolves transitive deps (e.g. zfb-runtime's `hono`) from the real pnpm
/// layout — the same resolution shape proven by the passing
/// `bundler_workspace_pkg_alias` test. `hono` is symlinked at the top level too
/// for parity with the embedded snapshot (both point at the same store path, so
/// esbuild dedups them).
fn provision_framework_node_modules(root: &Path) {
    let ws = workspace_root();
    let nm = root.join("node_modules");
    fs::create_dir_all(nm.join("@takazudo")).expect("create node_modules/@takazudo");
    let link = |src: PathBuf, dst: PathBuf| {
        std::os::unix::fs::symlink(&src, &dst)
            .unwrap_or_else(|e| panic!("symlink {} -> {}: {e}", dst.display(), src.display()));
    };
    link(ws.join("packages/zfb"), nm.join("@takazudo").join("zfb"));
    link(
        ws.join("packages/zfb-runtime"),
        nm.join("@takazudo").join("zfb-runtime"),
    );
    for pkg in ["preact", "preact-render-to-string", "hono"] {
        let src = pnpm_store_pkg(&ws, pkg)
            .unwrap_or_else(|| panic!("pnpm store missing {pkg}; run `pnpm install`"));
        link(src, nm.join(pkg));
    }
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
async fn subscribe_sse(base: &str) -> reqwest::Response {
    let resp = zfb_test_utils::open_sse(base).await;
    assert_eq!(resp.status().as_u16(), 200, "SSE endpoint must answer 200");
    resp
}

/// Drain any in-flight watcher ticks until the SSE stream stays quiet for
/// `quiet_gap` (or `cap` elapses).
///
/// Why this exists: `boot_and_handshake` writes `content/posts/__warmup-*.md`
/// to prove the watch stream is live, and the LAST such write can leave a tick
/// in flight AFTER the handshake returns. That trailing warmup tick re-walks
/// `src/` (materialising the fixture's just-edited component as a side effect),
/// produces the new bundle, and reloads the V8 host — but its page selection is
/// the warmup content route, NOT the route consuming the edited component. The
/// edited component's OWN tick then finds the bundle byte-identical and is
/// short-circuited by the #940/#956 skip-key, so its route is never marked
/// stale and keeps serving the old bytes (the product-side edge case behind this
/// harness workaround is tracked in #1301). Draining to quiescence before each
/// edit removes that race (mirrors the effective settle `dev_serve_e2e.rs` gets
/// from its earlier scenarios before its component-edit scenario).
async fn drain_ticks_until_quiescent(base: &str, quiet_gap: Duration, cap: Duration) {
    let start = Instant::now();
    while start.elapsed() < cap {
        let sse = subscribe_sse(base).await;
        match next_sse_event_name(sse, quiet_gap).await {
            // A tick fired within the gap — the watcher is still busy; keep draining.
            Ok(Some(_)) => continue,
            // No event for `quiet_gap` (or the stream ended) — quiescent.
            _ => break,
        }
    }
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
        let sse = subscribe_sse(&base).await;
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
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a full `zfb dev` server + esbuild + embedded V8 (symptom C also needs Tailwind); too slow / port-bound for the T1 gate"]
async fn e2e_src_component_edit_rerenders_route() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
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
        // Drain any trailing warmup-handshake ticks first so the edit tick is
        // not raced into a skip-key short-circuit (see drain fn docs).
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;
        // Subscribe to SSE BEFORE the edit so we catch the watcher tick.
        let sse = subscribe_sse(&base).await;
        fs::write(
            session.root.join("src/components/Widget.tsx"),
            "export function Widget() {\n  \
             return <p data-testid=\"widget\">WIDGET-MARKER-V2</p>;\n}\n",
        )
        .expect("edit src/components/Widget.tsx");

        // Secondary signal (D3): the tick SHOULD broadcast an SSE `page` event
        // via the pages_stale gate. This is consumed best-effort, NOT hard-
        // asserted: the dedicated SSE client has no whole-response timeout, so
        // a slow-but-correct tick can still reach this read. When an event DOES
        // arrive we still assert it is `page` (a wrong event type is a real
        // regression); a clean deadline or transport error falls through to
        // the authoritative served-HTML poll below.
        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(name)) => assert_eq!(
                name.as_str(),
                "page",
                "editing src/components/Widget.tsx broadcast an unexpected SSE event \
                 (expected `page`).\n{}",
                session.logs(),
            ),
            Ok(None) | Err(_) => eprintln!(
                "[symptom-A] no SSE `page` event observed within the window; \
                 relying on the authoritative served-HTML poll (D3)."
            ),
        }

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

/// SYMPTOM B acceptance — editing a transitively-imported CSS file reached via
/// a symlinked workspace dep (`@import '@scope/design-system'`, resolved through
/// a `node_modules` symlink to a real path outside the project tree) refreshes
/// `/assets/styles.css`. Observable (D3): `GET /assets/styles.css` serves the
/// new bytes on the next request.
///
/// Scope note: the local sibling `@import './tokens.css'` sub-case was dropped —
/// it is a base-resolution design mismatch, not a fixture bug (see the fixture
/// setup comment + #1300).
///
/// Falsifiability: without the resolved-`@import` watch registration, the
/// symlinked dep edit is observed by nobody and `/assets/styles.css` stays
/// stale until timeout.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a full `zfb dev` server + esbuild + embedded V8 (symptom C also needs Tailwind); too slow / port-bound for the T1 gate"]
async fn e2e_transitive_css_import_refreshes_stylesheet() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
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

    // Creating `node_modules/@scope/design-system` below flips
    // `detect_project_node_modules` ON, which shadows the binary-embedded
    // vendor snapshot. Reconstruct a complete framework `node_modules` FIRST so
    // esbuild + the SSR host can still resolve `@takazudo/*` / `preact*` (see
    // `provision_framework_node_modules` docs).
    provision_framework_node_modules(&root);

    // Create a "real" package directory OUTSIDE the project root. The dev
    // server resolves `@import '@scope/design-system'` through the symlinked
    // node_modules entry; canonicalize() follows the symlink to the real file
    // outside the project tree, which is the actual watcher target (#1288 D4).
    let pkg_dir = tempfile::tempdir().expect("create tempdir for fake @scope/design-system");
    let pkg_real_path = pkg_dir.path().canonicalize().expect("canonicalize pkg dir");
    // The package provides a single CSS file. The observable is a hand-authored
    // class selector (`.ds-marker-vN`) rather than a custom-property value:
    // Tailwind/Lightning passes user rules in an `@import`ed file through
    // verbatim, so the selector survives minification unambiguously (a
    // pseudo-hex like `#V2DS` risks being normalised or dropped as an invalid
    // color).
    let pkg_css_path = pkg_real_path.join("index.css");
    fs::write(
        &pkg_css_path,
        "/* @scope/design-system v1 */\n.ds-marker-v1 { color: #101010; }\n",
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

    // `styles/global.css` — the project CSS entry (zfb convention). Only the
    // Tailwind import and the symlinked workspace-dep `@import` are exercised
    // here.
    //
    // Deliberately NARROWED to the `@scope/design-system` sub-case: the earlier
    // draft also covered a LOCAL sibling `@import './tokens.css'`, but that path
    // is a genuine design mismatch, not a fixture bug. #1288's watcher resolves
    // `./tokens.css` relative to `styles/global.css`, whereas the Tailwind
    // engine inlines global.css into a temp entry at `working_dir =
    // project_root` and resolves `./tokens.css` against the PROJECT ROOT
    // (crates/zfb-css/src/engine.rs) — an irreconcilable base mismatch. Whether
    // a relative sibling `@import` should refresh under `zfb dev` is a design
    // decision to surface to the user, so it is intentionally out of scope for
    // this acceptance gate (spun out of #1294 into #1300).
    fs::create_dir_all(root.join("styles")).expect("create styles/");
    fs::write(
        root.join("styles/global.css"),
        "@import \"tailwindcss\";\n\
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

        // Baseline: GET /assets/styles.css serves the V1 design-system marker.
        // This is a real assertion (not just a 200 check): it proves the
        // bare-package `@import '@scope/design-system'` resolved through the
        // node_modules symlink and inlined the package CSS into the boot bundle
        // — the precondition the edit below then perturbs.
        poll_until_contains(
            &client,
            &css_url_fn(&base),
            "ds-marker-v1",
            SCENARIO_DEADLINE,
            "symptom-B baseline: /assets/styles.css must inline the @scope/design-system V1 marker",
            &session,
        )
        .await;

        // ── THE EDIT: symlinked workspace dep (@scope/design-system) ──────
        // The #1288 fix canonicalises the `@scope/design-system` node_modules
        // symlink to the real file outside the project root and registers that
        // real path as an extra watch target. Editing the real file fires a tick.
        //
        // Drain trailing warmup-handshake ticks first so the edit's tick is not
        // raced into a skip-key short-circuit (see drain fn docs).
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;
        let sse = subscribe_sse(&base).await;
        fs::write(
            &pkg_css_path,
            "/* @scope/design-system v2 */\n.ds-marker-v2 { color: #101010; }\n",
        )
        .expect("edit @scope/design-system index.css (real canonical path)");

        // A Style tick does not mark pages stale (no SSE `page` event) but it
        // does refresh the CSS asset bytes. The event type may be anything or
        // nothing here — the served CSS body below is the authoritative gate.
        let _ = next_sse_event_name(sse, SSE_DEADLINE).await;

        // The new V2 marker must appear in the served stylesheet.
        //
        // Falsifiability: without canonicalisation + extra-watch-target
        // registration, the symlinked real file is never watched; no tick fires;
        // this assertion times out on the old marker.
        poll_until_contains(
            &client,
            &css_url_fn(&base),
            "ds-marker-v2",
            SCENARIO_DEADLINE,
            "symptom-B (symlinked dep): /assets/styles.css must reflect @scope/design-system edit",
            &session,
        )
        .await;

        // Belt-and-suspenders: the old V1 marker must be gone from the asset.
        let served = client
            .get(css_url_fn(&base))
            .send()
            .await
            .expect("GET /assets/styles.css after design-system edit")
            .text()
            .await
            .unwrap_or_default();
        assert!(
            !served.contains("ds-marker-v1"),
            "/assets/styles.css still contains the old ds-marker-v1 after the \
             @scope/design-system edit — the asset was not refreshed.\n{}",
            session.logs(),
        );

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
#[ignore = "heavy: run with --ignored — Level-4 e2e; spawns a full `zfb dev` server + esbuild + embedded V8 (symptom C also needs Tailwind); too slow / port-bound for the T1 gate"]
async fn e2e_new_utility_class_in_component_is_emitted() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
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
    // The `@theme` block defines the `hgap-2xs` spacing token so that
    // `gap-x-hgap-2xs` is a REAL, emittable Tailwind v4 utility (gap utilities
    // resolve their value from the `--spacing-*` theme namespace). Without a
    // token, `gap-x-hgap-2xs` is an unknown utility Tailwind never emits, so the
    // assertion below would fail regardless of whether the Module→re-scan under
    // test works — the token makes the test actually exercise the re-scan.
    fs::write(
        root.join("styles/global.css"),
        "@import \"tailwindcss\";\n\
         \n\
         @theme {\n\
         \x20 --spacing-hgap-2xs: 0.125rem;\n\
         }\n\
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
        //
        // Drain trailing warmup ticks first so the component edit's tick is not
        // raced into a skip-key short-circuit (see drain fn docs).
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;
        let sse = subscribe_sse(&base).await;
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
