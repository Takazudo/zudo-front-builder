//! Cold-boot regression guard for issue #1598 / epic #1597.
//!
//! The fixture has one content collection with an entry route, a post index,
//! a tag page, and a paginated listing. It deliberately starts from a fresh
//! copied fixture: no `.zfb-build` directory, persisted graph, or test-seeded
//! `DepKind::Content` edge exists before the real `zfb dev` process boots.
//!
//! A frontmatter edit first proves every aggregate reader receives the new
//! title. A second, body-only edit then proves content provenance narrows past
//! beta's unrelated entry route while still rewriting every aggregate reader.
//! Finally it creates gamma mid-session and repeats that narrow body-only
//! assertion after discovery. With `ZFB_DEV_EAGER=1`, every check is an
//! on-disk watcher-tick write; no HTTP request can make a stale route fresh.
//!
//! This is a real V8/esbuild dev E2E and therefore adopts both the in-process
//! serial mutex and the cross-binary lock used by sibling dev tests.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const OVERALL_DEADLINE: Duration = Duration::from_secs(300);
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);
const SSE_DEADLINE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-content-aggregate-cold-boot")
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

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

struct DevSession {
    root: PathBuf,
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

    fn html_root(&self) -> PathBuf {
        self.root.join(".zfb-build").join("dev-pages")
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(idx) = rest.find("http://") {
        let candidate = &rest[idx + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or("");
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

fn spawn_dev(root: PathBuf, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log");

    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env("ZFB_DEV_EAGER", "1")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.process_group(0);

    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        root,
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

async fn poll_until_file_contains(path: &Path, marker: &str, phase: &str, session: &DevSession) {
    let start = Instant::now();
    while start.elapsed() < SCENARIO_DEADLINE {
        if fs::read_to_string(path)
            .map(|contents| contents.contains(marker))
            .unwrap_or(false)
        {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    panic!(
        "[{phase}] {} did not contain {marker:?} within {}s. No HTTP request was made to this \
         route after the edit, so a pass requires an eager watcher-tick write. Current content: \
         {:?}\n{}",
        path.display(),
        SCENARIO_DEADLINE.as_secs(),
        fs::read_to_string(path).unwrap_or_default(),
        session.logs(),
    );
}

/// Content and modification time of one eager-rendered output. The mtime
/// catches a broad fallback even when re-rendering happens to produce the
/// same bytes for the unrelated sibling route.
struct DiskSnapshot {
    content: String,
    mtime: SystemTime,
}

fn snapshot_file(path: &Path) -> DiskSnapshot {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {} for disk snapshot: {error}", path.display()));
    let mtime = fs::metadata(path)
        .unwrap_or_else(|error| panic!("stat {} for disk snapshot: {error}", path.display()))
        .modified()
        .expect("output filesystem must expose modification times");
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
        "[{phase}] {} was rewritten even though it is an unrelated entry route. \
         A full content fallback re-stamps this file even when its rendered bytes are unchanged.\n{}",
        path.display(),
        session.logs(),
    );
    assert_eq!(
        after.content,
        before.content,
        "[{phase}] {} changed despite being an unrelated entry route.\n{}",
        path.display(),
        session.logs(),
    );
}

async fn subscribe_sse(client: &reqwest::Client, base: &str) -> reqwest::Response {
    let response = client
        .get(format!("{base}/__zfb/reload"))
        .send()
        .await
        .expect("subscribe to /__zfb/reload");
    assert_eq!(
        response.status().as_u16(),
        200,
        "SSE endpoint must answer 200"
    );
    response
}

async fn drain_ticks_until_quiescent(client: &reqwest::Client, base: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        let sse = subscribe_sse(client, base).await;
        match next_sse_event_name(sse, Duration::from_millis(1500)).await {
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
}

async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
    let boot_start = Instant::now();
    let port = loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path),
            );
            if combined.contains("embed_v8")
                || combined.contains("no esbuild")
                || combined.contains("no tailwind")
                || combined.contains("tailwindcss") && combined.contains("not found")
            {
                eprintln!(
                    "[content_aggregate_cold_boot_e2e] known unavailable dependency; skipping.\n{}",
                    session.logs(),
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing a ready banner.\n{}",
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

    let ready_start = Instant::now();
    loop {
        if matches!(
            client.get(format!("{base}/")).send().await,
            Ok(response) if response.status().as_u16() == 200
        ) {
            break;
        }
        assert!(
            ready_start.elapsed() < BOOT_DEADLINE,
            "GET / never answered 200 within {}s after the ready banner.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Repeated edits of an already-existing entry prove the watch stream is
    // live without adding a route that could race the post-edit assertions.
    let sse = subscribe_sse(&client, &base).await;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let warmup = session.root.join("content/posts/__warmup.md");
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut revision = 0u32;
            while !stop.load(Ordering::SeqCst) {
                fs::write(
                    &warmup,
                    format!(
                        "---\ntitle: Warmup\ndate: 2025-01-01\ntags:\n  - warmup\n---\n\nWarmup revision {revision}.\n"
                    ),
                )
                .expect("edit existing warmup content entry");
                revision += 1;
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
        "watcher never became live: no edit-induced SSE event within {}s.\n{}",
        SSE_DEADLINE.as_secs(),
        session.logs(),
    );

    Some((base, client))
}

enum ScenarioOutcome {
    Completed,
    Skipped,
}

/// Falsifiability: suppressing aggregate re-render after either alpha edit
/// leaves the index, tag, or pagination output stale. Reverting provenance
/// seeding makes the body-only edit take the graph's `All` fallback and
/// re-stamps beta; the sibling mtime assertion catches that even if beta's
/// rendered bytes happen to remain identical.
#[tokio::test(flavor = "multi_thread")]
async fn cold_boot_content_edit_rerenders_entry_and_all_aggregates() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[content_aggregate_cold_boot_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let temp = tempfile::tempdir().expect("create tempdir for cold-boot fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy cold-boot fixture");
    assert!(
        !root.join(".zfb-build").exists(),
        "fixture must have no persisted dev graph or prior render output before cold boot"
    );

    let mut session = spawn_dev(root, &esbuild);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) = boot_and_handshake(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };

        let html_root = session.html_root();
        let entry = html_root.join("posts/alpha/index.html");
        let sibling_entry = html_root.join("posts/beta/index.html");
        let discovered_entry = html_root.join("posts/gamma/index.html");
        let post_index = html_root.join("posts/index.html");
        let tag_page = html_root.join("tags/guide/index.html");
        let pagination = html_root.join("posts/page/1/index.html");

        // Boot materialises every route from a real fixture, without any
        // test-side graph setup or inspection.
        poll_until_file_contains(&entry, "V1-BODY-ALPHA", "boot entry route", &session).await;
        poll_until_file_contains(
            &sibling_entry,
            "V1-BODY-BETA",
            "boot sibling entry route",
            &session,
        )
        .await;
        for (path, phase) in [
            (&post_index, "boot post index"),
            (&tag_page, "boot tag page"),
            (&pagination, "boot paginated listing"),
        ] {
            poll_until_file_contains(path, "Alpha V1 Frontmatter", phase, &session).await;
        }

        // A handshake can leave its final tick in flight. Settle before the
        // frontmatter edit this test evaluates first.
        drain_ticks_until_quiescent(&client, &base).await;

        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("content/posts/alpha.md"),
            "---\ntitle: Alpha V2 Frontmatter\ndate: 2026-01-02\ntags:\n  - guide\n---\n\nV2-BODY-ALPHA updated markdown body.\n",
        )
        .expect("edit existing alpha entry body and frontmatter");

        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(name)) => assert_eq!(
                name.as_str(),
                "page",
                "alpha content edit broadcast an unexpected SSE event.\n{}",
                session.logs(),
            ),
            Ok(None) | Err(_) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed; relying on the \
                 authoritative eager on-disk output checks."
            ),
        }

        // No HTTP request touches these routes after the edit. Each V2 marker
        // therefore requires the eager watcher tick to rewrite the output.
        poll_until_file_contains(
            &entry,
            "V2-BODY-ALPHA",
            "entry body rerender after alpha edit",
            &session,
        )
        .await;
        poll_until_file_contains(
            &entry,
            "Alpha V2 Frontmatter",
            "entry frontmatter rerender after alpha edit",
            &session,
        )
        .await;
        for (path, phase) in [
            (
                &post_index,
                "post index rerender after alpha frontmatter edit",
            ),
            (&tag_page, "tag page rerender after alpha frontmatter edit"),
            (
                &pagination,
                "paginated listing rerender after alpha frontmatter edit",
            ),
        ] {
            poll_until_file_contains(path, "Alpha V2 Frontmatter", phase, &session).await;
        }

        // Frontmatter intentionally makes the legacy per-source narrowing
        // gate conservative. After its aggregate regression is proven above,
        // hold frontmatter stable and make a body-only edit: this is the
        // precise graph-narrowing case where beta must remain untouched.
        drain_ticks_until_quiescent(&client, &base).await;
        let sibling_before = snapshot_file(&sibling_entry);
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("content/posts/alpha.md"),
            "---\ntitle: Alpha V2 Frontmatter\ndate: 2026-01-02\ntags:\n  - guide\n---\n\nV3-BODY-ALPHA body-only narrowed markdown edit.\n",
        )
        .expect("make a body-only edit to existing alpha entry");

        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(name)) => assert_eq!(
                name.as_str(),
                "page",
                "alpha body-only edit broadcast an unexpected SSE event.\n{}",
                session.logs(),
            ),
            Ok(None) | Err(_) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed for the \
                 body-only edit; relying on the authoritative eager on-disk output checks."
            ),
        }

        poll_until_file_contains(
            &entry,
            "V3-BODY-ALPHA",
            "entry body rerender after alpha body-only edit",
            &session,
        )
        .await;
        for (path, phase) in [
            (
                &post_index,
                "post index rerender after alpha body-only edit",
            ),
            (&tag_page, "tag page rerender after alpha body-only edit"),
            (
                &pagination,
                "paginated listing rerender after alpha body-only edit",
            ),
        ] {
            poll_until_file_contains(path, "V3-BODY-ALPHA", phase, &session).await;
        }
        assert_snapshot_unchanged(
            &sibling_entry,
            &sibling_before,
            "alpha body-only edit narrows past the unrelated beta entry route",
            &session,
        );

        // A newly created entry must re-bundle, re-walk membership, and gain
        // both its dynamic entry route and all aggregate readers. The first
        // subsequent body edit intentionally primes the existing frontmatter
        // gate; the second is the narrow-after-discovery assertion.
        drain_ticks_until_quiescent(&client, &base).await;
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV1-BODY-GAMMA discovered markdown body.\n",
        )
        .expect("create a new gamma entry mid-session");
        let _ = next_sse_event_name(sse, SSE_DEADLINE).await;
        poll_until_file_contains(
            &discovered_entry,
            "V1-BODY-GAMMA",
            "new gamma entry is rendered after discovery",
            &session,
        )
        .await;
        for (path, phase) in [
            (&post_index, "post index includes discovered gamma"),
            (&tag_page, "tag page includes discovered gamma"),
            (&pagination, "pagination includes discovered gamma"),
        ] {
            poll_until_file_contains(path, "V1-BODY-GAMMA", phase, &session).await;
        }

        drain_ticks_until_quiescent(&client, &base).await;
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV2-BODY-GAMMA first body-only edit seeds the frontmatter gate.\n",
        )
        .expect("prime the gamma frontmatter gate after discovery");
        let _ = next_sse_event_name(sse, SSE_DEADLINE).await;
        poll_until_file_contains(
            &discovered_entry,
            "V2-BODY-GAMMA",
            "first gamma body-only edit completes after discovery",
            &session,
        )
        .await;

        drain_ticks_until_quiescent(&client, &base).await;
        let entry_before_gamma_edit = snapshot_file(&entry);
        let sibling_before_gamma_edit = snapshot_file(&sibling_entry);
        let sse = subscribe_sse(&client, &base).await;
        fs::write(
            session.root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV3-BODY-GAMMA second body-only edit narrows after discovery.\n",
        )
        .expect("make a narrowed gamma body-only edit after discovery");
        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(name)) => assert_eq!(
                name.as_str(),
                "page",
                "gamma body-only edit broadcast an unexpected SSE event.\n{}",
                session.logs(),
            ),
            Ok(None) | Err(_) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed for the \
                 post-discovery body-only edit; relying on the authoritative eager on-disk \
                 output checks."
            ),
        }
        poll_until_file_contains(
            &discovered_entry,
            "V3-BODY-GAMMA",
            "gamma entry rerender after narrowed body-only edit",
            &session,
        )
        .await;
        for (path, phase) in [
            (
                &post_index,
                "post index rerender after narrowed gamma body-only edit",
            ),
            (
                &tag_page,
                "tag page rerender after narrowed gamma body-only edit",
            ),
            (
                &pagination,
                "pagination rerender after narrowed gamma body-only edit",
            ),
        ] {
            poll_until_file_contains(path, "V3-BODY-GAMMA", phase, &session).await;
        }
        assert_snapshot_unchanged(
            &entry,
            &entry_before_gamma_edit,
            "post-discovery gamma edit narrows past alpha entry route",
            &session,
        );
        assert_snapshot_unchanged(
            &sibling_entry,
            &sibling_before_gamma_edit,
            "post-discovery gamma edit narrows past beta entry route",
            &session,
        );

        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] cold-boot aggregate dev E2E did not finish within {}s. Process group \
             {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}
