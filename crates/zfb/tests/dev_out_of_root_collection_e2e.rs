//! Confirm gate for epic #1548 (Collections Outside Root), issue #1552 —
//! Level-4 real `zfb dev` edit→serve loop over a collection whose configured
//! root lives OUTSIDE the project directory (`allowOutsideRoot: true`,
//! `path: "../shared-content/posts"`).
//!
//! Waves 1-2 landed the opt-in (#1549) and the dev-server canonical-path fix
//! (#1550: a boot-time `ResolvedRoots` inventory routes out-of-root roots
//! through the absolute extras watch channel, threading the same canonical
//! form through tick-candidate derivation, frontmatter-hash seeding,
//! discovery, and the manifest digest). Unit tests cover each of those
//! pieces in isolation; NONE of them prove the live dev loop actually wires
//! together end to end — the documented failure mode is SILENT DEGRADATION
//! (external content edits observed by nobody, or a broad fallback
//! re-render masking the narrowing bug), which only a real watcher + real
//! HTTP server can catch.
//!
//! ## What this proves
//!
//! - **(a) Narrow re-render**: editing ONE out-of-root `.mdx` entry re-renders
//!   only its OWN generated route, not the fan-out. The session runs under
//!   `ZFB_DEV_EAGER=1` (NOT the #1027 lazy default) specifically so this is
//!   provable: in the lazy default, a route the tick does not select is only
//!   `mark_stale`'d — an in-memory flag, never a disk write — so a broken
//!   narrowing that silently selected nothing at all (#1550's actual failure
//!   mode; see the commit message) and a hypothetical broad "select every
//!   route" fallback would BOTH leave every route's disk file untouched,
//!   making a disk-based sibling check vacuously true either way. Eager mode
//!   writes every eagerly-selected route to disk immediately, so a broad
//!   fallback becomes a REAL, catchable write to the SIBLING route's on-disk
//!   HTML file (`.zfb-build/dev-pages/posts/beta/index.html`) — its mtime AND
//!   bytes are snapshotted before the edit and asserted byte-for-byte,
//!   mtime-for-mtime unchanged afterward. Tick-candidate derivation, the
//!   exact machinery #1550 fixed, is shared between eager and lazy dispatch
//!   (only the write timing differs), so this still exercises the code path
//!   under test. Alpha's OWN eager disk write is the primary proof that
//!   narrowing engaged AT ALL (a silent-failure regression times out there);
//!   beta's untouched snapshot is the proof it did not ALSO over-select.
//! - **(b) Discovery**: creating a NEW out-of-root `.mdx` entry mid-session
//!   makes it a servable route (`GET /posts/gamma/`) with zero dev-server
//!   restart — mirrors `dev_serve_e2e.rs` scenario 8's watch-ADD discovery,
//!   but for a collection root the watcher only reaches via the absolute
//!   extras channel.
//!
//! ## Harness
//!
//! Local copy of the `dev_serve_e2e.rs` / `dev_dep_invalidation_1284_e2e.rs`
//! helpers (process group + file-log capture, `DevServerGuard` group-kill on
//! Drop, ready-banner port parse, the SSE/watcher-live handshake, tick
//! quiescence draining before each edit). Rust integration tests are
//! separate binaries and cannot import another test file's private items,
//! so this is a deliberate copy, not a new pattern.
//!
//! ## Fixture
//!
//! `tests/fixtures/dev-out-of-root-basic/`:
//! - `project/` — the `zfb dev` cwd. `zfb.config.json` declares the `posts`
//!   collection at `../shared-content/posts` with `allowOutsideRoot: true`.
//!   `pages/posts/[slug].tsx` is the sole dynamic content route (mirrors
//!   dev-loop-basic's `[slug].tsx`). `pages/index.tsx` deliberately does NOT
//!   read the collection, so it can never legitimately be affected by a
//!   content edit — keeping scenario (a)'s sibling-untouched proof narrowly
//!   scoped to the OTHER post route, not conflated with an unrelated page.
//! - `shared-content/posts/` — the out-of-root collection root, a sibling of
//!   `project/`, seeded with two entries (`alpha.mdx`, `beta.mdx`) so there
//!   are two independent dynamic routes to distinguish "own route" from
//!   "sibling route", plus `__warmup.mdx`, a third entry that exists ONLY so
//!   the watcher-live handshake has a pre-existing file to EDIT. The
//!   handshake must not CREATE entries (#1581): a genuinely-new entry
//!   correctly forces a full fan-out, which re-stamps `beta` and races
//!   scenario (a)'s baseline snapshot. See the Phase C comment.
//!
//! ## Where it runs
//!
//! CI runs it on ubuntu/inotify (`health.yml`'s `cargo nextest`, via the
//! `e2e-heavy` test-group). It has since also been run for real on macOS
//! arm64 / FSEvents during issue #1581 — which is where the FSEvents
//! `Created`-coalescing bug it now guards was found and fixed. The PR gate is
//! still ubuntu-only, so macOS parity rides the weekly `exam.yml` macOS
//! re-exam, not the PR gate.
//!
//! ## Blind spot
//!
//! Scenario (a) proves the sibling POST route is untouched. It does NOT prove
//! `pages/index.tsx` is untouched: a static route carries no params
//! provenance, so it re-renders in full via the S1 fallback on every content
//! tick regardless (the fixture's index deliberately does not read the
//! collection, so this never confuses the sibling proof). An AGGREGATE dynamic
//! page — a post index listing every entry — is likewise out of scope: nothing
//! here guards the planner's unknown-path `PageSelection::All` fallback that
//! such a page currently relies on to stay fresh (see #1581's commit message).

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

// Serialises this binary's single spawning test against itself (a no-op
// today, kept for parity with the sibling e2e files in case a second
// scenario is ever split out) and, via `CrossBinaryE2eLock`, against every
// OTHER e2e binary that boots a real `zfb dev`/`zfb build` process — see
// `zfb-test-utils/src/cross_binary_lock.rs` for the lock-ordering rationale.
static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Overall wall-clock watchdog for the whole test (boot + both scenarios).
const OVERALL_DEADLINE: Duration = Duration::from_secs(280);

/// Deadline for the dev server to print its ready banner + first `GET /` 200.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-assertion deadline for a marker to appear after an edit.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);

/// Deadline for the watcher-live handshake + per-scenario SSE tick signal.
const SSE_DEADLINE: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Harness (local copy — see module doc).
// ---------------------------------------------------------------------------

fn base_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-out-of-root-basic")
}

/// Recursive directory copy. Copies BOTH fixture children (`project/` and
/// `shared-content/`) into the tempdir root so their `../shared-content`
/// relative relationship survives the copy.
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

    /// The dev server's HTML write root (`dev_html_root_for` in
    /// commands/dev.rs) — the same directory the tick fan-out, the
    /// request-time lazy renderer, and `serve_page`'s disk leg all agree on.
    fn html_root(&self) -> PathBuf {
        self.root.join(".zfb-build").join("dev-pages")
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

/// Spawn `zfb dev --port 0` over the already-prepared fixture project root,
/// in its own process group, with stdout/stderr captured to files. Callers
/// copy and extend the fixture BEFORE calling this.
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

/// Poll the ON-DISK file until it contains `needle` — no HTTP traffic, so a
/// success proves the tick itself wrote the bytes (eager render).
async fn poll_until_file_contains(
    path: &Path,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if fs::read_to_string(path)
            .map(|s| s.contains(needle))
            .unwrap_or(false)
        {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] file {} did not contain {needle:?} within {}s (no HTTP request \
         was made to this route — the tick was expected to write it eagerly).\n\
         Current content: {:?}\n{}",
        path.display(),
        deadline.as_secs(),
        fs::read_to_string(path).unwrap_or_default(),
        session.logs(),
    );
}

/// Content + mtime of an on-disk HTML file, captured before an edit so the
/// AFTER state can be proven byte-for-byte and mtime-for-mtime unchanged —
/// see the module doc's "(a) Narrow re-render" section for why mtime is the
/// signal that catches a full-fallback rebuild a content-only diff would miss.
struct DiskSnapshot {
    content: String,
    mtime: SystemTime,
}

fn snapshot_file(path: &Path) -> DiskSnapshot {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {} for disk snapshot: {e}", path.display()));
    let mtime = fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {} for disk snapshot: {e}", path.display()))
        .modified()
        .expect("mtime must be supported on this platform");
    DiskSnapshot { content, mtime }
}

fn assert_snapshot_unchanged(
    path: &Path,
    before: &DiskSnapshot,
    phase: &str,
    session: &DevSession,
) {
    let after = snapshot_file(path);
    assert_eq!(
        after.mtime,
        before.mtime,
        "[{phase}] {} mtime changed — something rewrote this untouched sibling \
         route's file. A full-rebuild fallback re-stamps every route's output \
         even when the rendered bytes end up identical, so this is exactly the \
         signal a content-only diff would miss.\n{}",
        path.display(),
        session.logs(),
    );
    assert_eq!(
        after.content,
        before.content,
        "[{phase}] {} content changed even though its mtime is unchanged \
         (unexpected either way) — the sibling route must not be touched at \
         all by an edit to a different out-of-root entry.\n{}",
        path.display(),
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
/// `quiet_gap` (or `cap` elapses). See
/// `dev_dep_invalidation_1284_e2e.rs::drain_ticks_until_quiescent` for the
/// full rationale (a trailing warmup-handshake tick can otherwise race a
/// scenario's own edit and alias its skip-key short-circuit, #1301).
async fn drain_ticks_until_quiescent(base: &str, quiet_gap: Duration, cap: Duration) {
    let start = Instant::now();
    while start.elapsed() < cap {
        let sse = subscribe_sse(base).await;
        match next_sse_event_name(sse, quiet_gap).await {
            Ok(Some(_)) => continue,
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
///
/// `shared_content_posts` is the OUT-OF-ROOT collection root the handshake
/// writes warmup entries into (see the Phase C comment below for why it
/// cannot be an arbitrary project-root file).
async fn boot_and_handshake(
    session: &mut DevSession,
    shared_content_posts: &Path,
) -> Option<(String, reqwest::Client)> {
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
                    "[out_of_root_e2e] `zfb dev` exited with a known-skip indicator \
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

    // Phase C: watcher-live handshake. Subscribe to SSE FIRST, then EDIT the
    // pre-existing `__warmup.mdx` entry (body only, frontmatter held constant)
    // until the first SSE event arrives.
    //
    // This MUST target `shared_content_posts`, not an arbitrary project-root
    // file: `DEFAULT_WATCH_ROOTS` in commands/dev.rs is `pages`, `content`,
    // `components`, `layouts`, `styles`, `data`, `src`, and the two config
    // files — a bare file dropped directly under the project root (this
    // fixture's `project/` has none of those as its own root) is never
    // watched and would hang this handshake until `SSE_DEADLINE`. Touching a
    // real collection entry also guarantees an observable SSE event (an
    // unrecognized file type could legitimately produce a no-op tick with
    // nothing broadcast).
    //
    // It must be an EDIT of an entry the fixture already ships, NOT a
    // fresh-named `__warmup-{i}.mdx` CREATE (issue #1581). A brand-new
    // collection entry is genuinely new, so its tick is correctly NOT
    // `fan_out_safe` — a new entry can change aggregate pages — and it
    // re-renders the FULL fan-out, re-stamping every route's on-disk HTML,
    // including the `beta` sibling scenario (a) snapshots as its untouched
    // baseline. That fan-out then races the `beta_before` snapshot: a run was
    // captured where `alpha.mdx` narrowed perfectly and the test STILL failed,
    // purely on a warmup tick's late write to beta. Editing a pre-existing
    // entry keeps every handshake tick narrow (it re-renders only
    // `/posts/__warmup/`), so no warmup tick can touch beta whenever it lands.
    // Holding the frontmatter constant keeps the G4 gate from tripping too.
    {
        let warmup_entry = shared_content_posts.join("__warmup.mdx");
        let sse = subscribe_sse(&base).await;
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let warmup_entry = warmup_entry.clone();
            let stop = Arc::clone(&stop);
            tokio::spawn(async move {
                let mut i = 0u32;
                while !stop.load(Ordering::SeqCst) {
                    let _ = fs::write(
                        &warmup_entry,
                        format!(
                            "---\ntitle: Warmup Out Of Root\n---\n\n\
                             V1-MARKER-WARMUP body for the warmup post. rev {i}\n"
                        ),
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
// The test: narrow re-render (a) then discovery (b), one dev session.
// ---------------------------------------------------------------------------

/// Falsifiability — (a): revert #1550's canonical-root threading (or force
/// tick-candidate derivation back onto the literal `..`-root). The out-of-root
/// event path no longer matches any candidate at all — #1550's documented
/// real-world failure mode is SILENT: nothing re-renders, so
/// `poll_until_file_contains(alpha_file, "V2-MARKER-ALPHA")` times out.
/// `assert_snapshot_unchanged` on beta additionally guards the OTHER failure
/// class this harness can reach because it runs under `ZFB_DEV_EAGER=1`: a
/// broad "select every route" fallback would eagerly re-stamp beta's disk
/// file too, which the mtime check catches even if a future regression
/// happened to leave beta's rendered bytes identical.
/// Falsifiability — (b): revert the extras-channel routing for out-of-root
/// collection roots. The watcher never observes `gamma.mdx`'s ADD event, no
/// tick fires, and the discovery poll times out with a 404/never-200 body.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_out_of_root_edit_narrows_rerender_and_discovers_new_entry() {
    // Cross-binary lock acquired BEFORE the in-binary SERIAL guard — see
    // the lock-ordering note in zfb-test-utils/src/cross_binary_lock.rs.
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[out_of_root_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create tempdir for dev-out-of-root-basic fixture");
    let tmp_root = tmp.path().canonicalize().expect("canonicalize tempdir");
    copy_dir(&base_fixture_dir(), &tmp_root).expect("copy dev-out-of-root-basic fixture");
    let project_root = tmp_root.join("project");
    let shared_content_posts = tmp_root.join("shared-content").join("posts");

    // ZFB_DEV_EAGER=1 (not the #1027 lazy default): under the default lazy
    // contract a route the tick does NOT select for eager re-render is only
    // `mark_stale`'d — a purely in-memory flag, never a disk write (see
    // commands/dev.rs `lazy_render_tick`) — so a broken out-of-root narrowing
    // that fell back to selecting every route would STILL leave beta's file
    // byte- and mtime-identical below, only silently failing to advance
    // alpha's own render too. Eager mode makes the full-fan-out fallback this
    // test guards against a REAL disk write (`make_render_callback` writes
    // every eagerly-selected route immediately), so `assert_snapshot_unchanged`
    // on beta is a genuine discriminator rather than a tautology. Tick
    // candidate derivation — the exact machinery #1550 fixed — is shared
    // between eager and lazy dispatch; only the write timing differs, so this
    // still exercises the code path under test.
    let mut session = spawn_dev(project_root, &esbuild, &[("ZFB_DEV_EAGER", "1")]);
    let pgid = session.guard.pgid;

    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session, &shared_content_posts).await
        else {
            return ScenarioOutcome::Skipped;
        };

        // ---- Baseline: both out-of-root routes serve their V1 markers ----
        poll_until_contains(
            &client,
            &format!("{base}/posts/alpha/"),
            "V1-MARKER-ALPHA",
            SCENARIO_DEADLINE,
            "baseline: GET /posts/alpha/",
            &session,
        )
        .await;
        poll_until_contains(
            &client,
            &format!("{base}/posts/beta/"),
            "V1-MARKER-BETA",
            SCENARIO_DEADLINE,
            "baseline: GET /posts/beta/",
            &session,
        )
        .await;

        let html_root = session.html_root();
        let alpha_file = html_root.join("posts/alpha/index.html");
        let beta_file = html_root.join("posts/beta/index.html");

        // The boot render is eager for every dynamic route regardless of the
        // eager/lazy dev-render mode (the boot latch, #1025) — confirm both
        // are materialised on disk with zero further HTTP traffic before
        // snapshotting beta as the untouched-sibling baseline below.
        poll_until_file_contains(
            &alpha_file,
            "V1-MARKER-ALPHA",
            SCENARIO_DEADLINE,
            "boot: /posts/alpha/ materialised on disk",
            &session,
        )
        .await;
        poll_until_file_contains(
            &beta_file,
            "V1-MARKER-BETA",
            SCENARIO_DEADLINE,
            "boot: /posts/beta/ materialised on disk",
            &session,
        )
        .await;

        // ------------------------------------------------------------------
        // Scenario (a) — narrow re-render.
        // ------------------------------------------------------------------

        // Drain trailing warmup-handshake ticks first so the edit's tick is
        // not raced into a skip-key short-circuit (see drain fn docs).
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;

        // Snapshot the sibling's disk output BEFORE the edit. This is the
        // authoritative "narrowing engaged" proof (see module doc).
        let beta_before = snapshot_file(&beta_file);

        let sse = subscribe_sse(&base).await;
        fs::write(
            shared_content_posts.join("alpha.mdx"),
            "---\ntitle: Alpha Out Of Root\n---\n\nV2-MARKER-ALPHA body for the alpha post.\n",
        )
        .expect("edit out-of-root alpha.mdx");

        // Secondary signal (best-effort, matches dev_serve_e2e/dev_dep_invalidation
        // idiom): an SSE `page` event should fire. A timeout falls through to the
        // authoritative disk poll below rather than failing here.
        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(name)) => assert_eq!(
                name.as_str(),
                "page",
                "editing the out-of-root alpha.mdx broadcast an unexpected SSE \
                 event (expected `page`).\n{}",
                session.logs(),
            ),
            Ok(None) | Err(_) => eprintln!(
                "[out_of_root_e2e scenario-a] no SSE `page` event observed within \
                 the window; relying on the authoritative disk poll."
            ),
        }

        // Own route: eagerly re-rendered, proven with zero HTTP traffic to
        // /posts/alpha/ (an HTTP poll could not distinguish eager from a
        // request-triggered lazy render).
        poll_until_file_contains(
            &alpha_file,
            "V2-MARKER-ALPHA",
            SCENARIO_DEADLINE,
            "scenario a: out-of-root alpha edit eagerly renders its own route",
            &session,
        )
        .await;

        // Sibling route: must be byte-for-byte, mtime-for-mtime untouched —
        // the crux of the narrow-vs-fallback distinction.
        assert_snapshot_unchanged(
            &beta_file,
            &beta_before,
            "scenario a: sibling /posts/beta/ must not be eagerly rewritten by \
             the alpha edit",
            &session,
        );

        // Belt-and-suspenders over HTTP: alpha serves the new marker, beta
        // still serves its original body with no cross-route bleed.
        poll_until_contains(
            &client,
            &format!("{base}/posts/alpha/"),
            "V2-MARKER-ALPHA",
            SCENARIO_DEADLINE,
            "scenario a: GET /posts/alpha/ serves the new marker",
            &session,
        )
        .await;
        let beta_body = client
            .get(format!("{base}/posts/beta/"))
            .send()
            .await
            .expect("GET /posts/beta/ after alpha edit")
            .text()
            .await
            .unwrap_or_default();
        assert!(
            beta_body.contains("V1-MARKER-BETA") && !beta_body.contains("V2-MARKER-ALPHA"),
            "scenario a: /posts/beta/ must keep serving its original body with no \
             bleed from the alpha edit; got:\n{beta_body}\n{}",
            session.logs(),
        );

        // ------------------------------------------------------------------
        // Scenario (b) — discovery: a brand-new out-of-root entry is served
        // without a dev-server restart.
        // ------------------------------------------------------------------
        drain_ticks_until_quiescent(&base, Duration::from_millis(1500), Duration::from_secs(20))
            .await;
        let sse = subscribe_sse(&base).await;
        fs::write(
            shared_content_posts.join("gamma.mdx"),
            "---\ntitle: Gamma Out Of Root\n---\n\nV1-MARKER-GAMMA body for the gamma post.\n",
        )
        .expect("create out-of-root gamma.mdx");

        // Best-effort SSE observation; the discovery poll below is authoritative.
        let _ = next_sse_event_name(sse, SSE_DEADLINE).await;

        poll_until_contains(
            &client,
            &format!("{base}/posts/gamma/"),
            "V1-MARKER-GAMMA",
            SCENARIO_DEADLINE,
            "scenario b: newly created out-of-root entry is discovered and served \
             without a dev-server restart",
            &session,
        )
        .await;

        ScenarioOutcome::Completed
    };

    let outcome = tokio::time::timeout(OVERALL_DEADLINE, body).await;
    match outcome {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] out-of-root dev-loop e2e did not finish within {}s — hang, \
             the alpha edit never re-rendered its own route, the beta sibling was \
             unexpectedly rewritten, or the new gamma entry was never discovered. \
             Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}
