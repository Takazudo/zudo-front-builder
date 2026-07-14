//! Cold-boot regression guard for issue #1598 / epic #1597.
//!
//! The fixture has one content collection with an entry route, a post index,
//! a tag page, and a paginated listing. It deliberately starts from a fresh
//! copied fixture: no `.zfb-build` directory, persisted graph, or test-seeded
//! `DepKind::Content` edge exists before the real `zfb dev` process boots.
//!
//! An edit changes both alpha's markdown body and frontmatter title. With
//! `ZFB_DEV_EAGER=1`, the test can prove the watcher tick itself rewrote all
//! expected output files without an HTTP request making a stale route fresh.
//! Today's conservative `PageSelection::All` fallback passes this test. Later
//! Content-edge narrowing must retain the aggregate edges for it to stay green.
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
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const OVERALL_DEADLINE: Duration = Duration::from_secs(240);
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

/// Falsifiability: temporarily suppressing aggregate re-render after the
/// alpha edit leaves the index, tag, or pagination output on its V1 title and
/// makes a marker poll time out. The entry route's V2 body proves the edit was
/// observed; the aggregate title checks prove frontmatter reached all readers.
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
        let post_index = html_root.join("posts/index.html");
        let tag_page = html_root.join("tags/guide/index.html");
        let pagination = html_root.join("posts/page/1/index.html");

        // Boot materialises every route from a real fixture, without any
        // test-side graph setup or inspection.
        poll_until_file_contains(&entry, "V1-BODY-ALPHA", "boot entry route", &session).await;
        for (path, phase) in [
            (&post_index, "boot post index"),
            (&tag_page, "boot tag page"),
            (&pagination, "boot paginated listing"),
        ] {
            poll_until_file_contains(path, "Alpha V1 Frontmatter", phase, &session).await;
        }

        // A handshake can leave its final tick in flight. Settle before the
        // one edit this test evaluates.
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
