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
//! `dev-content-memo-helper` adds a cold-lazy case where the home page reads
//! entries memoized by a module-level helper that a dynamic route's `paths()`
//! populated first, so the home render never calls `getCollection()` itself.
//!
//! This is a real V8/esbuild dev E2E and therefore adopts both the in-process
//! serial mutex and the cross-binary lock used by sibling dev tests.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use zfb_graph::persist::{load_from_disk, save_to_disk, ManifestDigest};
use zfb_graph::{DepKind, DependencyGraph, PageDeps, PageId};
use zfb_test_utils::{next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const OVERALL_DEADLINE: Duration = Duration::from_secs(300);
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(60);
const SSE_DEADLINE: Duration = Duration::from_secs(30);
const GRACEFUL_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

const FIXTURE_WATCH_ROOTS: &[&str] = &[
    "pages",
    "content",
    "components",
    "layouts",
    "styles",
    "data",
    "src",
    "zfb.config.json",
    "zfb.config.ts",
];

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
        let parent = self.root.join(".zfb-build").join("dev-pages");
        let prefix = format!("session-{}-", self.guard.pgid);
        std::fs::read_dir(&parent)
            .expect("dev HTML sessions")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("this dev process's HTML session")
            .path()
    }

    fn graph_cache_path(&self) -> PathBuf {
        self.root.join(".zfb").join("graph.bin")
    }

    async fn stop_gracefully(&mut self) {
        unsafe { libc::kill(-self.guard.pgid, libc::SIGINT) };

        let start = Instant::now();
        loop {
            if let Some(status) = self.guard.try_exit_status() {
                assert!(
                    status.success(),
                    "`zfb dev` exited unsuccessfully after SIGINT ({status:?}).\n{}",
                    self.logs(),
                );
                return;
            }
            if start.elapsed() >= GRACEFUL_SHUTDOWN_DEADLINE {
                unsafe { libc::kill(-self.guard.pgid, libc::SIGKILL) };
                let _ = self.guard.child.wait();
                panic!(
                    "`zfb dev` did not exit within {}s after SIGINT.\n{}",
                    GRACEFUL_SHUTDOWN_DEADLINE.as_secs(),
                    self.logs(),
                );
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
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

/// Extract the `{n}` route count from `run_boot_render`'s Cold boot log line
/// ("dev: cold-lazy — no prebuilt dist/ seed required; {n} route(s) render
/// on first request (ZFB_DEV_BOOT_LAZY=cold)"). `None` if the line is
/// missing or the count is unparseable — see this helper's call site for
/// why a bare substring match on "cold-lazy" is not enough (codex review
/// finding, issue #1812).
fn parse_cold_lazy_route_count(log: &str) -> Option<usize> {
    let marker = "no prebuilt dist/ seed required; ";
    let start = log.find(marker)? + marker.len();
    let digits: String = log[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[derive(Clone, Copy)]
enum DevMode {
    Eager,
    Lazy,
}

/// `boot_lazy` sets `ZFB_DEV_BOOT_LAZY` explicitly (e.g. `Some("cold")`);
/// `None` keeps the pre-epic-#1806 default of removing the var entirely
/// (Off), matching every pre-existing test in this binary.
fn spawn_dev(root: PathBuf, mode: DevMode, boot_lazy: Option<&str>) -> DevSession {
    spawn_dev_with_timing(root, mode, boot_lazy, false)
}

fn spawn_dev_with_timing(
    root: PathBuf,
    mode: DevMode,
    boot_lazy: Option<&str>,
    timing: bool,
) -> DevSession {
    spawn_dev_with_env(root, mode, boot_lazy, timing, &[])
}

fn spawn_dev_with_env(
    root: PathBuf,
    mode: DevMode,
    boot_lazy: Option<&str>,
    timing: bool,
    extra_env: &[(&str, &str)],
) -> DevSession {
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
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    if matches!(mode, DevMode::Eager) {
        command.env("ZFB_DEV_EAGER", "1");
    }
    if let Some(value) = boot_lazy {
        command.env("ZFB_DEV_BOOT_LAZY", value);
    }
    if timing {
        command.env("ZFB_DEV_TIMING", "1");
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
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

fn fixture_manifest_digest(root: &Path) -> ManifestDigest {
    ManifestDigest::compute(
        root,
        &FIXTURE_WATCH_ROOTS
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
        &[
            PathBuf::from("zfb.config.json"),
            PathBuf::from("zfb.config.ts"),
        ],
    )
    .expect("compute aggregate fixture manifest digest")
}

fn graph_cache_digest(path: &Path) -> ManifestDigest {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("read persisted graph {}: {error}", path.display()));
    assert!(
        bytes.len() >= 38 && &bytes[..4] == b"ZFBG",
        "{} is not a zfb persisted graph cache",
        path.display(),
    );
    ManifestDigest::from_bytes(
        bytes[6..38]
            .try_into()
            .expect("persisted graph cache has a complete digest header"),
    )
}

fn load_cached_graph(path: &Path) -> DependencyGraph {
    let digest = graph_cache_digest(path);
    load_from_disk(path, &digest)
        .unwrap_or_else(|error| panic!("load persisted graph {}: {error}", path.display()))
        .unwrap_or_else(|| {
            panic!(
                "persisted graph {} was unexpectedly rejected",
                path.display()
            )
        })
}

fn replace_page_without_content(graph: &mut DependencyGraph, page: PageId) {
    let deps = graph
        .deps_of(&page)
        .into_iter()
        .filter(|(_, kind)| *kind != DepKind::Content)
        .collect();
    graph.upsert(PageDeps::new(page, deps));
}

/// Retag the cache after adding gamma, then make its Content edges deliberately
/// wrong. The restart must load this valid cache but replace every Content edge
/// from the fresh worker's observations before it handles an edit.
fn prepare_stale_persisted_content_graph(root: &Path) {
    let cache_path = root.join(".zfb").join("graph.bin");
    let mut graph = load_cached_graph(&cache_path);
    let gamma = root.join("content/posts/gamma.md");

    for source in [
        "pages/posts/index.tsx",
        "pages/tags/[tag].tsx",
        "pages/posts/page/[page].tsx",
    ] {
        replace_page_without_content(&mut graph, PageId::new(root.join(source)));
    }

    // A stale extra Content edge must disappear too. Adding it twice makes the
    // final saved-cache assertion prove that reconciliation leaves no duplicate
    // evidence behind.
    let home = PageId::new(root.join("pages/index.tsx"));
    let data_probe = root.join("data/restart-probe.json");
    let mut home_deps = graph
        .deps_of(&home)
        .into_iter()
        .filter(|(_, kind)| *kind != DepKind::Content)
        .collect::<Vec<_>>();
    home_deps.push((gamma.clone(), DepKind::Content));
    home_deps.push((gamma, DepKind::Content));
    // This non-Content cache edge proves the second process actually loaded the
    // retagged cache: without it, the Data edit below takes the All fallback and
    // the unrelated beta entry is marked stale.
    home_deps.push((data_probe, DepKind::Data));
    graph.upsert(PageDeps::new(home, home_deps));

    let current_digest = fixture_manifest_digest(root);
    save_to_disk(&graph, &current_digest, &cache_path)
        .unwrap_or_else(|error| panic!("retag persisted graph {}: {error}", cache_path.display()));
}

fn assert_final_reseeded_content_graph(root: &Path) {
    let graph = load_cached_graph(&root.join(".zfb").join("graph.bin"));
    let gamma = root.join("content/posts/gamma.md");
    let home = PageId::new(root.join("pages/index.tsx"));
    assert!(
        graph
            .deps_of(&home)
            .iter()
            .all(|(_, kind)| *kind != DepKind::Content),
        "the stale persisted Content edge for the home page survived live reseeding",
    );

    for source in [
        "pages/posts/[slug].tsx",
        "pages/posts/index.tsx",
        "pages/tags/[tag].tsx",
        "pages/posts/page/[page].tsx",
    ] {
        let content = graph
            .deps_of(&PageId::new(root.join(source)))
            .into_iter()
            .filter_map(|(path, kind)| (kind == DepKind::Content).then_some(path))
            .collect::<Vec<_>>();
        let unique = content.iter().collect::<BTreeSet<_>>();
        assert_eq!(
            content.len(),
            unique.len(),
            "{source} retained duplicate Content edges after restart reseeding: {content:?}",
        );
        assert!(
            unique.contains(&gamma),
            "{source} lost the post-create gamma Content edge after restart reseeding: {content:?}",
        );
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

/// Poll a served route until it returns the marker. In lazy mode this is the
/// deliberate request-time refresh observable; use the disk-only helper above
/// when a test needs to prove that a watcher tick wrote output eagerly.
async fn poll_until_response_contains(
    client: &reqwest::Client,
    url: &str,
    marker: &str,
    phase: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last_observation = String::from("(no response yet)");
    while start.elapsed() < SCENARIO_DEADLINE {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 200 && body.contains(marker) {
                    return;
                }
                last_observation = format!("status {status}, body:\n{body}");
            }
            Err(error) => last_observation = format!("request error: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] GET {url} did not serve {marker:?} within {}s. Last observation:\n\
         {last_observation}\n{}",
        SCENARIO_DEADLINE.as_secs(),
        session.logs(),
    );
}

fn assert_file_lacks(path: &Path, marker: &str, phase: &str, session: &DevSession) {
    let content = fs::read_to_string(path).unwrap_or_default();
    assert!(
        !content.contains(marker),
        "[{phase}] {} already contains {marker:?} before a request. The lazy tick must mark this \
         aggregate route stale rather than render it eagerly.\n{}",
        path.display(),
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

async fn subscribe_sse(base: &str) -> reqwest::Response {
    let response = zfb_test_utils::open_sse(base).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "SSE endpoint must answer 200"
    );
    response
}

async fn drain_ticks_until_quiescent(base: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        let sse = subscribe_sse(base).await;
        match next_sse_event_name(sse, Duration::from_millis(1500)).await {
            Ok(Some(_)) => continue,
            // A quiet window is the only purpose of this helper; the file
            // polls after each caller's edit remain the authoritative gates.
            Ok(None) => break,
            Err(error) => panic!("SSE read failed while draining watcher ticks: {error:#}"),
        }
    }
}

/// Shared by both handshake variants below: parse the ready port from the
/// log, or detect a known-unavailable-dependency skip. Touches only the
/// process's own log files — no network I/O, so it makes no page request.
async fn wait_for_ready_port(session: &mut DevSession) -> Option<u16> {
    let boot_start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = format!(
                "{}{}",
                read_log(&session.stdout_path),
                read_log(&session.stderr_path),
            );
            if combined.contains("direct-v8 = off") || combined.contains("no esbuild") {
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
            return Some(port);
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Shared by both handshake variants below: repeated edits of an
/// already-existing entry prove the watch stream is live without adding or
/// requesting a route that could race the caller's own post-edit
/// assertions.
async fn confirm_watcher_live(session: &DevSession, base: &str) {
    let sse = subscribe_sse(base).await;
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
}

/// Banner-only sibling of [`confirm_watcher_live`]: proves the watch stream
/// is live by editing a `data/` (Data-classified) probe file instead of a
/// content-collection entry (codex review finding, issue #1812).
/// `content_narrowing`'s eager carve-out (see `lazy_render_tick` in
/// `commands/dev.rs`) eagerly refreshes an edited content entry's OWN route
/// even under lazy dev render — a genuine render this test's "zero renders
/// before the defining edit" premise must not tolerate, even though it never
/// touches the routes this test later asserts on. A Data-classified edit
/// carries no such carve-out (`compute_lazy_eager_sets` returns an empty map
/// whenever the hint is `None`, which it always is for non-Content paths),
/// so every affected route is only ever marked stale, never rendered.
///
/// `probe_path`'s parent directory must already exist when `zfb dev` binds
/// (create it before spawning) — a watch target that doesn't exist yet at
/// boot is never registered (this fixture's own boot log warns exactly that
/// for the unused `data`/`components`/etc. roots).
async fn confirm_watcher_live_via_data_probe(session: &DevSession, base: &str, probe_path: &Path) {
    let sse = subscribe_sse(base).await;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let probe_path = probe_path.to_path_buf();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut revision = 0u32;
            while !stop.load(Ordering::SeqCst) {
                fs::write(&probe_path, format!("{{\"revision\":{revision}}}\n"))
                    .expect("edit handshake data probe");
                revision += 1;
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        })
    };
    let first = next_sse_event_name(sse, SSE_DEADLINE)
        .await
        .expect("read SSE stream during banner-only watcher-live handshake");
    stop.store(true, Ordering::SeqCst);
    let _ = writer.await;
    assert!(
        first.is_some(),
        "watcher never became live: no edit-induced SSE event within {}s.\n{}",
        SSE_DEADLINE.as_secs(),
        session.logs(),
    );
}

fn build_reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client")
}

async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client)> {
    let port = wait_for_ready_port(session).await?;
    let base = format!("http://localhost:{port}");
    let client = build_reqwest_client();

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

    confirm_watcher_live(session, &base).await;

    Some((base, client))
}

/// Banner-only variant for tests that must prove behavior under ZERO prior
/// page requests AND zero prior renders (issue #1812). Waits for the ready
/// banner and for the watcher to come live WITHOUT ever requesting a page
/// route or rendering any content: readiness is polled via the SSE control
/// endpoint instead of `GET /` (that endpoint is registered independently of
/// `page_root`/`page_handler` — see
/// `crates/zfb-server/src/routes.rs`), and the watcher-live handshake edits
/// `data_probe_path` (a `data/`-rooted, Data-classified file the caller must
/// pre-create) rather than a content-collection entry — see
/// [`confirm_watcher_live_via_data_probe`] for why a content edit here would
/// have been a genuine (if invisible-to-the-caller's-assertions) render.
async fn boot_and_handshake_banner_only(
    session: &mut DevSession,
    data_probe_path: &Path,
) -> Option<(String, reqwest::Client)> {
    let port = wait_for_ready_port(session).await?;
    let base = format!("http://localhost:{port}");
    let client = build_reqwest_client();

    let ready_start = Instant::now();
    loop {
        if zfb_test_utils::open_sse(&base).await.status().as_u16() == 200 {
            break;
        }
        assert!(
            ready_start.elapsed() < BOOT_DEADLINE,
            "SSE readiness endpoint never answered 200 within {}s after the ready banner.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    confirm_watcher_live_via_data_probe(session, &base, data_probe_path).await;

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

    let mut session = spawn_dev(root, DevMode::Eager, None);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, _client)) = boot_and_handshake(&mut session).await else {
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
        drain_ticks_until_quiescent(&base).await;

        let sse = subscribe_sse(&base).await;
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
            Ok(None) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed; relying on the \
                 authoritative eager on-disk output checks."
            ),
            Err(error) => panic!(
                "SSE read failed after alpha content edit: {error:#}\n{}",
                session.logs(),
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
        drain_ticks_until_quiescent(&base).await;
        let sibling_before = snapshot_file(&sibling_entry);
        let sse = subscribe_sse(&base).await;
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
            Ok(None) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed for the \
                 body-only edit; relying on the authoritative eager on-disk output checks."
            ),
            Err(error) => panic!(
                "SSE read failed after alpha body-only edit: {error:#}\n{}",
                session.logs(),
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
        drain_ticks_until_quiescent(&base).await;
        let sse = subscribe_sse(&base).await;
        fs::write(
            session.root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV1-BODY-GAMMA discovered markdown body.\n",
        )
        .expect("create a new gamma entry mid-session");
        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(_)) => {}
            // The discovered entry and aggregate output-file polls below are
            // the authoritative gates; this read only observes the tick.
            Ok(None) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE event after gamma discovery; \
                 relying on authoritative output-file polls."
            ),
            Err(error) => panic!(
                "SSE read failed after gamma discovery: {error:#}\n{}",
                session.logs(),
            ),
        }
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

        drain_ticks_until_quiescent(&base).await;
        let sse = subscribe_sse(&base).await;
        fs::write(
            session.root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV2-BODY-GAMMA first body-only edit seeds the frontmatter gate.\n",
        )
        .expect("prime the gamma frontmatter gate after discovery");
        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(_)) => {}
            // The discovered entry file poll below is the authoritative gate;
            // this read only observes the edit tick.
            Ok(None) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE event after gamma frontmatter \
                 priming edit; relying on the authoritative output-file poll."
            ),
            Err(error) => panic!(
                "SSE read failed after gamma frontmatter priming edit: {error:#}\n{}",
                session.logs(),
            ),
        }
        poll_until_file_contains(
            &discovered_entry,
            "V2-BODY-GAMMA",
            "first gamma body-only edit completes after discovery",
            &session,
        )
        .await;

        drain_ticks_until_quiescent(&base).await;
        let entry_before_gamma_edit = snapshot_file(&entry);
        let sibling_before_gamma_edit = snapshot_file(&sibling_entry);
        let sse = subscribe_sse(&base).await;
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
            Ok(None) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed for the \
                 post-discovery body-only edit; relying on the authoritative eager on-disk \
                 output checks."
            ),
            Err(error) => panic!(
                "SSE read failed after post-discovery gamma body-only edit: {error:#}\n{}",
                session.logs(),
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

/// Confirm sub-issue for epic #1806 (issue #1812): cold-lazy
/// (`ZFB_DEV_BOOT_LAZY=cold`) skips the eager boot render entirely, so
/// `complete_boot_content_provenance` (only called from the eager
/// `initial_build` success path — see `run_boot_render`) never runs and
/// `clear_boot_content_provenance` (called unconditionally on every boot,
/// eager or cold — see the boot hook right before step 4) leaves the
/// dependency graph with ZERO `DepKind::Content` edges for any pre-existing
/// collection entry. This test proves the resulting conservative
/// `PageSelection::All` fallback (an unknown content path always trips it —
/// see `zfb-build/src/policy.rs`'s `KnownContentEntries` doc comment) still
/// invalidates the entry route AND every aggregate reader even though NO
/// page has EVER been rendered or requested when the content edit lands —
/// there is no prior boot render to even be "stale" relative to.
///
/// Falsifiability: the assertion right after the banner-only handshake, that
/// every route's `.zfb-build/dev-pages` output does not exist yet, is the
/// direct proof of the "zero renders so far" premise this test exists to
/// establish, and is what actually falsifies this test under Off mode
/// (`ZFB_DEV_BOOT_LAZY` unset) — see this test's own `#[ignore]`-free
/// sibling run recorded in the PR description for the observed failure.
/// Off/Auto without a Cold conjunct fall through `run_boot_render`'s eager
/// branch, which renders every route (and completes content provenance)
/// synchronously inside the same deferred boot task this handshake waits on
/// — before this test's content edit ever lands — so those `.html` files
/// already exist by the time the assertion below runs.
#[tokio::test(flavor = "multi_thread")]
async fn cold_boot_zero_prior_requests_content_edit_refreshes_entry_and_aggregates() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create tempdir for cold zero-request fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize cold zero-request fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy cold zero-request fixture");
    assert!(
        !root.join(".zfb-build").exists(),
        "fixture must have no persisted dev graph or prior render output before cold boot"
    );

    // Pre-create the `data/` watch root (this fixture doesn't ship one) so
    // it is a live watch target from boot — a directory that doesn't exist
    // yet at boot is never registered (see this binary's own "does not
    // exist yet" boot warnings for the unused roots). The handshake edits
    // this Data-classified probe file, not a content entry, to prove the
    // watcher is live without rendering anything (codex review finding,
    // issue #1812 — see `confirm_watcher_live_via_data_probe`).
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");

    // Boot Cold explicitly. `DevMode::Lazy` (the request-time lazy render
    // strategy) is required for boot-lazy to activate at all
    // (`boot_lazy_decision`); it is also this binary's default when
    // `ZFB_DEV_EAGER` is unset.
    let mut session = spawn_dev(root, DevMode::Lazy, Some("cold"));
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };

        // A deferred-bundle FAILURE still prints this exact log line with a
        // route count of 0 (`run_boot_render`'s Cold branch logs
        // `session.route_count()` unconditionally — see its own doc comment).
        // A bare substring match would pass against a broken/empty route
        // table just as readily as a working boot (codex review finding,
        // issue #1812), so require a NONZERO count too.
        let cold_lazy_route_count = parse_cold_lazy_route_count(&read_log(&session.stdout_path));
        assert!(
            matches!(cold_lazy_route_count, Some(n) if n > 0),
            "expected dev's cold-lazy boot log line confirming ZFB_DEV_BOOT_LAZY=cold took the \
             seedless boot-lazy branch in `run_boot_render` with a NONZERO published route \
             count, not a silent fallback to the eager boot render or a broken/empty deferred \
             bundle. Parsed route count: {cold_lazy_route_count:?}\n{}",
            session.logs(),
        );

        let html_root = session.html_root();
        let entry = html_root.join("posts/alpha/index.html");
        let post_index = html_root.join("posts/index.html");
        let tag_page = html_root.join("tags/guide/index.html");
        let pagination = html_root.join("posts/page/1/index.html");
        let home = html_root.join("index.html");

        // Zero requests AND zero renders so far: Cold must not have written
        // any route to disk yet. This is the direct falsifiability proof —
        // Off/Auto without the Cold conjunct render every route eagerly
        // during the same deferred boot task the handshake above already
        // waited on, so these files would already exist by this point.
        for (path, phase) in [
            (&entry, "entry route"),
            (&post_index, "post index"),
            (&tag_page, "tag page"),
            (&pagination, "paginated listing"),
            (&home, "home route"),
        ] {
            assert!(
                !path.exists(),
                "[{phase}] {} already exists before any request or edit — cold-lazy must skip \
                 the eager boot render entirely, leaving every route unrendered until its own \
                 first request.\n{}",
                path.display(),
                session.logs(),
            );
        }

        let entry_url = format!("{base}/posts/alpha/");
        let post_index_url = format!("{base}/posts/");
        let tag_url = format!("{base}/tags/guide/");
        let pagination_url = format!("{base}/posts/page/1/");

        // The defining edit: alpha.md is a PRE-EXISTING collection entry (not
        // newly created), so the discovery hook never recorded a Content
        // edge for it either (that hook only fires for newly-created
        // files — see `KnownContentEntries`'s doc comment). With zero prior
        // renders, no `DepKind::Content` edge for alpha exists anywhere in
        // the graph, so this edit can only be handled by the conservative
        // `PageSelection::All` fallback.
        let sse = subscribe_sse(&base).await;
        fs::write(
            session.root.join("content/posts/alpha.md"),
            "---\ntitle: Alpha Cold Zero Frontmatter\ndate: 2026-01-02\ntags:\n  - guide\n---\n\nCOLD-ZERO-BODY-ALPHA edited before any request.\n",
        )
        .expect("edit existing alpha entry before any page has ever been requested");

        match next_sse_event_name(sse, SSE_DEADLINE).await {
            Ok(Some(name)) => assert_eq!(
                name.as_str(),
                "page",
                "cold zero-prior-requests alpha edit broadcast an unexpected SSE event.\n{}",
                session.logs(),
            ),
            Ok(None) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed for the cold \
                 zero-prior-requests edit; relying on the authoritative first-request response \
                 checks."
            ),
            Err(error) => panic!(
                "SSE read failed after cold zero-prior-requests alpha edit: {error:#}\n{}",
                session.logs(),
            ),
        }

        // Each of these is the route's FIRST-EVER request. A pass here
        // requires the request-time render to read the just-edited content
        // from disk, not some snapshot the (skipped) boot render or the
        // boot-time collection walk captured earlier.
        poll_until_response_contains(
            &client,
            &entry_url,
            "COLD-ZERO-BODY-ALPHA",
            "entry route serves fresh content on its first-ever request",
            &session,
        )
        .await;
        for (url, phase) in [
            (
                &post_index_url,
                "post index serves fresh content on its first-ever request",
            ),
            (
                &tag_url,
                "tag page serves fresh content on its first-ever request",
            ),
            (
                &pagination_url,
                "paginated listing serves fresh content on its first-ever request",
            ),
        ] {
            poll_until_response_contains(
                &client,
                url,
                "Alpha Cold Zero Frontmatter",
                phase,
                &session,
            )
            .await;
        }

        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] cold zero-prior-requests content invalidation dev E2E did not finish \
             within {}s. Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

/// A restart must load the persisted graph for non-Content data edges, then
/// discard its stale Content evidence and rebuild it from the new worker. The
/// cache is intentionally retagged after gamma is created, with every aggregate
/// Content edge removed and a duplicate bogus home-page edge inserted. On the
/// second lazy session, gamma's body edit must still refresh every aggregate on
/// request, while beta and home remain untouched.
#[tokio::test(flavor = "multi_thread")]
async fn warm_restart_reseeds_content_provenance_for_lazy_aggregate_requests() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create tempdir for restart fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize restart fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy restart fixture");
    fs::create_dir_all(root.join("data")).expect("create data fixture directory");
    fs::write(root.join("data/restart-probe.json"), "{\"revision\":1}\n")
        .expect("write initial data cache probe");

    let body = async {
        let mut first = spawn_dev(root.clone(), DevMode::Lazy, None);
        let Some((first_base, first_client)) = boot_and_handshake(&mut first).await else {
            return ScenarioOutcome::Skipped;
        };

        // Create gamma before persistence. Requesting the new direct route and
        // each aggregate makes the first session's cache contain a complete
        // post-create provenance snapshot before the deliberate corruption.
        drain_ticks_until_quiescent(&first_base).await;
        let sse = subscribe_sse(&first_base).await;
        fs::write(
            root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV1-BODY-GAMMA created before restart.\n",
        )
        .expect("create gamma before persisting graph");
        let event = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE after gamma create")
            .expect("gamma create must emit a page SSE event");
        assert_eq!(
            event, "page",
            "gamma create emitted an unexpected SSE event"
        );
        poll_until_response_contains(
            &first_client,
            &format!("{first_base}/posts/gamma/"),
            "V1-BODY-GAMMA",
            "first-session gamma entry after discovery",
            &first,
        )
        .await;
        for (url, phase) in [
            ("/posts/", "first-session post index includes gamma"),
            ("/tags/guide/", "first-session tag page includes gamma"),
            ("/posts/page/1/", "first-session pagination includes gamma"),
        ] {
            poll_until_response_contains(
                &first_client,
                &format!("{first_base}{url}"),
                "V1-BODY-GAMMA",
                phase,
                &first,
            )
            .await;
        }
        drain_ticks_until_quiescent(&first_base).await;

        first.stop_gracefully().await;
        let first_cache = first.graph_cache_path();
        assert!(
            first_cache.exists(),
            "first session did not persist {} after graceful shutdown.\n{}",
            first_cache.display(),
            first.logs(),
        );
        let first_graph = load_cached_graph(&first_cache);
        assert!(
            first_graph
                .consumers_of(&root.join("content/posts/gamma.md"))
                .is_some_and(|consumers| !consumers.is_empty()),
            "the first session did not persist gamma's discovered Content edges",
        );

        // The created gamma changes the manifest since initial boot. Retag the
        // valid cache to the current fixture digest, then make only its Content
        // edges stale. The next session must load the cache but derive Content
        // provenance from its fresh worker rather than trusting it.
        prepare_stale_persisted_content_graph(&root);

        let mut second = spawn_dev(root.clone(), DevMode::Lazy, None);
        let Some((second_base, second_client)) = boot_and_handshake(&mut second).await else {
            return ScenarioOutcome::Skipped;
        };
        drain_ticks_until_quiescent(&second_base).await;

        let html_root = second.html_root();
        let home = html_root.join("index.html");
        let beta = html_root.join("posts/beta/index.html");
        let gamma = html_root.join("posts/gamma/index.html");
        let post_index = html_root.join("posts/index.html");
        let tag_page = html_root.join("tags/guide/index.html");
        let pagination = html_root.join("posts/page/1/index.html");
        // Default startup now renders on request; prime this unrelated route
        // before comparing its bytes/mtime across subsequent invalidations.
        poll_until_response_contains(
            &second_client,
            &format!("{second_base}/posts/beta/"),
            "V1-BODY-BETA",
            "restart beta entry first request",
            &second,
        )
        .await;
        poll_until_file_contains(&beta, "V1-BODY-BETA", "restart beta entry boot", &second).await;

        // This data edit proves the retagged cache actually loaded: its cached
        // Data edge selects only home. A cache miss would choose All, mark beta
        // stale, and the beta request below would rewrite its file.
        let beta_before_data = snapshot_file(&beta);
        let sse = subscribe_sse(&second_base).await;
        fs::write(root.join("data/restart-probe.json"), "{\"revision\":2}\n")
            .expect("edit persisted data cache probe");
        let event = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE after data cache probe edit")
            .expect("data cache probe edit must emit a page SSE event");
        assert_eq!(
            event, "page",
            "data cache probe emitted an unexpected SSE event"
        );
        poll_until_response_contains(
            &second_client,
            &format!("{second_base}/posts/beta/"),
            "V1-BODY-BETA",
            "cache-hit data edit leaves beta fresh",
            &second,
        )
        .await;
        assert_snapshot_unchanged(
            &beta,
            &beta_before_data,
            "cache-hit data edit must not stale beta",
            &second,
        );
        // The precise data edge deliberately staled home; make it fresh before
        // the Content-edge check so a later home rewrite can only be caused by
        // the stale persisted Content edge we injected above.
        poll_until_response_contains(
            &second_client,
            &format!("{second_base}/"),
            "Posts",
            "cache-hit data edit refreshes selected home route",
            &second,
        )
        .await;

        drain_ticks_until_quiescent(&second_base).await;
        let home_before_gamma = snapshot_file(&home);
        let beta_before_gamma = snapshot_file(&beta);
        let sse = subscribe_sse(&second_base).await;
        fs::write(
            root.join("content/posts/gamma.md"),
            "---\ntitle: Gamma Frontmatter\ndate: 2026-01-03\ntags:\n  - guide\n---\n\nV2-BODY-GAMMA body edit after warm restart.\n",
        )
        .expect("edit post-create gamma after restart");
        let event = next_sse_event_name(sse, SSE_DEADLINE)
            .await
            .expect("read SSE after restarted gamma edit")
            .expect("restarted gamma edit must emit a page SSE event");
        assert_eq!(
            event, "page",
            "restarted gamma edit emitted an unexpected SSE event"
        );

        // The direct entry remains in the lazy eager set, but aggregate routes
        // must wait for their first request. A stale persisted graph missing the
        // aggregate edges would leave these responses on their V1 body marker.
        poll_until_file_contains(
            &gamma,
            "V2-BODY-GAMMA",
            "restarted gamma direct entry stays eagerly refreshed",
            &second,
        )
        .await;
        for (path, phase) in [
            (&post_index, "post index remains stale before lazy request"),
            (&tag_page, "tag page remains stale before lazy request"),
            (&pagination, "pagination remains stale before lazy request"),
        ] {
            assert_file_lacks(path, "V2-BODY-GAMMA", phase, &second);
        }
        for (url, phase) in [
            ("/posts/", "restarted post index refreshes on request"),
            ("/tags/guide/", "restarted tag page refreshes on request"),
            (
                "/posts/page/1/",
                "restarted pagination refreshes on request",
            ),
        ] {
            poll_until_response_contains(
                &second_client,
                &format!("{second_base}{url}"),
                "V2-BODY-GAMMA",
                phase,
                &second,
            )
            .await;
        }
        assert_snapshot_unchanged(
            &beta,
            &beta_before_gamma,
            "lazy gamma edit leaves unrelated beta entry untouched",
            &second,
        );
        poll_until_response_contains(
            &second_client,
            &format!("{second_base}/"),
            "Posts",
            "unrelated home remains servable after restarted gamma edit",
            &second,
        )
        .await;
        assert_snapshot_unchanged(
            &home,
            &home_before_gamma,
            "live provenance must remove stale persisted home Content edge",
            &second,
        );

        second.stop_gracefully().await;
        assert_final_reseeded_content_graph(&root);
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] warm-restart content provenance dev E2E did not finish within {}s",
            OVERALL_DEADLINE.as_secs(),
        ),
    }
}

fn memo_helper_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dev-content-memo-helper")
}

/// Wait until stderr past `offset` holds a complete line accepted by
/// `matches`, and return that line plus the stderr offset just after it.
async fn wait_for_stderr_line_where(
    session: &DevSession,
    offset: usize,
    matches: impl Fn(&str) -> bool,
    description: &str,
    phase: &str,
) -> (String, usize) {
    let start = Instant::now();
    while start.elapsed() < SCENARIO_DEADLINE {
        let stderr = read_log(&session.stderr_path);
        let mut cursor = offset;
        let tail = stderr.get(offset..).unwrap_or_default();
        for line in tail.split_inclusive('\n') {
            cursor += line.len();
            if line.ends_with('\n') && matches(line) {
                return (line.trim_end().to_string(), cursor);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] no stderr line {description} within {}s.\n{}",
        SCENARIO_DEADLINE.as_secs(),
        session.logs(),
    );
}

/// [`wait_for_stderr_line_where`] for a line containing every `needles`
/// fragment.
async fn wait_for_stderr_line(
    session: &DevSession,
    offset: usize,
    needles: &[&str],
    phase: &str,
) -> (String, usize) {
    wait_for_stderr_line_where(
        session,
        offset,
        |line| needles.iter().all(|needle| line.contains(needle)),
        &format!("containing {needles:?}"),
        phase,
    )
    .await
}

/// A `[zfb-timing] tick()` line whose `kinds=[...]` list names `file_name`,
/// with any change kind.
fn is_tick_line_for(line: &str, file_name: &str) -> bool {
    let Some(rest) = line.split("[zfb-timing] tick(): kinds=[").nth(1) else {
        return false;
    };
    let Some((kinds, _)) = rest.split_once(']') else {
        return false;
    };
    kinds.split(", ").any(|kind| {
        kind.split_once(':')
            .is_some_and(|(name, _)| name == file_name)
    })
}

/// In-place title edits of a.md attempted before giving up on a narrowed tick.
/// macOS FSEvents can report an in-place write as `Created`, which fails the
/// orchestrator's `fan_out_safe` gate and selects every page.
const MEMO_EDIT_ATTEMPTS: u32 = 3;

/// A user helper memoizes `getCollection("articles")` at module level. Every
/// fresh worker evaluates the dynamic route's `paths()` first, which fills the
/// cache, so the home page later reads only the cached entries' `data`. Its
/// render must still count as an `articles` reader: an in-place title write to
/// a.md has to refresh `/`, while `/independent/`, which reads nothing, stays
/// untouched. The `fan_out_safe=true` tick line proves the refresh comes from
/// narrowed provenance, not the `All` fallback. When FSEvents reports the
/// write as `Created`, that tick is un-narrowed and proves nothing, so the
/// test re-renders every route and makes a new unique edit, up to
/// [`MEMO_EDIT_ATTEMPTS`] times. It never passes without a narrowed tick whose
/// plan holds exactly two pages (one without the fix).
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_memoized_collection_helper_title_edit_refreshes_home() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create tempdir for memo-helper fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize memo-helper fixture root");
    copy_dir(&memo_helper_fixture_dir(), &root).expect("copy memo-helper fixture");
    assert!(!root.join(".zfb-build").exists(), "fixture must start cold");
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");

    let mut session = spawn_dev_with_timing(root, DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };
        let route_count = parse_cold_lazy_route_count(&read_log(&session.stdout_path));
        assert!(
            matches!(route_count, Some(n) if n > 0),
            "cold-lazy boot was not confirmed; count {route_count:?}\n{}",
            session.logs()
        );

        let home_url = format!("{base}/");
        let detail_url = format!("{base}/articles/a/");
        let independent_url = format!("{base}/independent/");
        // Browser-like first visits: each route renders in the current worker
        // and records its observation before the edit lands.
        for (url, marker, phase) in [
            (&home_url, "Alpha Memo V1", "home lists the initial title"),
            (
                &detail_url,
                "Alpha Memo V1",
                "detail serves the initial title",
            ),
            (
                &independent_url,
                "MEMO-INDEPENDENT",
                "independent route renders",
            ),
        ] {
            poll_until_response_contains(&client, url, marker, phase, &session).await;
        }
        let independent_file = session.html_root().join("independent/index.html");
        let a_md = session.root.join("content/articles/a.md");
        let mut narrowed: Option<(String, String, DiskSnapshot)> = None;
        for attempt in 1..=MEMO_EDIT_ATTEMPTS {
            drain_ticks_until_quiescent(&base).await;
            let independent_before = snapshot_file(&independent_file);
            let title = format!("Alpha Memo V{} attempt {attempt}", attempt + 1);
            let offset = read_log(&session.stderr_path).len();
            let sse = subscribe_sse(&base).await;
            fs::write(&a_md, format!("---\ntitle: {title}\n---\n\nAlpha body.\n"))
                .expect("edit a.md title in place");

            match next_sse_event_name(sse, SSE_DEADLINE).await {
                Ok(Some(name)) => assert_eq!(
                    name.as_str(),
                    "page",
                    "a.md title edit broadcast an unexpected SSE event.\n{}",
                    session.logs(),
                ),
                Ok(None) => eprintln!(
                    "[content_aggregate_cold_boot_e2e] no SSE page event observed for the memo \
                     helper edit; relying on the authoritative response and timing checks."
                ),
                Err(error) => panic!(
                    "SSE read failed after the memo helper edit: {error:#}\n{}",
                    session.logs(),
                ),
            }

            let (tick, after_tick) = wait_for_stderr_line_where(
                &session,
                offset,
                |line| is_tick_line_for(line, "a.md"),
                "with a tick() naming a.md",
                "a.md edit reached the orchestrator",
            )
            .await;
            eprintln!("[content_aggregate_cold_boot_e2e] memo edit attempt {attempt}: {tick}");
            if tick.contains("fan_out_safe=true") {
                let (plan, _) = wait_for_stderr_line(
                    &session,
                    after_tick,
                    &["[zfb-timing] lazy-render plan: pages="],
                    "lazy-render plan after the narrowed a.md tick",
                )
                .await;
                eprintln!("[content_aggregate_cold_boot_e2e] memo edit attempt {attempt}: {plan}");
                narrowed = Some((title, plan, independent_before));
                break;
            }
            // An un-narrowed tick selects every page, so it cannot show which
            // routes provenance selected. Re-render every route in the current
            // worker, as the first visits did, before the next unique edit.
            for (url, marker, phase) in [
                (&home_url, title.as_str(), "home after an un-narrowed tick"),
                (
                    &detail_url,
                    title.as_str(),
                    "detail after an un-narrowed tick",
                ),
                (
                    &independent_url,
                    "MEMO-INDEPENDENT",
                    "independent after an un-narrowed tick",
                ),
            ] {
                poll_until_response_contains(&client, url, marker, phase, &session).await;
            }
        }
        let Some((title, plan, independent_before)) = narrowed else {
            panic!(
                "none of {MEMO_EDIT_ATTEMPTS} in-place a.md edits produced a fan_out_safe=true \
                 tick, so the memoized provenance was never proven by a narrowed plan.\n{}",
                session.logs(),
            );
        };
        assert!(
            plan.ends_with("pages=2"),
            "the narrowed plan must hold exactly the detail route and the memoizing home page, \
             and not /independent/: {plan}\n{}",
            session.logs(),
        );

        poll_until_response_contains(
            &client,
            &home_url,
            &title,
            "home serves the edited title read through the memoized helper",
            &session,
        )
        .await;
        poll_until_response_contains(
            &client,
            &detail_url,
            &title,
            "detail serves the edited title",
            &session,
        )
        .await;
        poll_until_response_contains(
            &client,
            &independent_url,
            "MEMO-INDEPENDENT",
            "independent route still serves",
            &session,
        )
        .await;
        assert_snapshot_unchanged(
            &independent_file,
            &independent_before,
            "a.md edit narrows past /independent/, which reads no content",
            &session,
        );
        let logs = session.logs();
        assert!(
            !logs.contains("content provenance unavailable"),
            "provenance must stay available for this scenario\n{logs}"
        );

        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] memo-helper content provenance dev E2E did not finish within {}s. \
             Process group {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

// #3823: each request-order case is an independent nextest test. The
// index-before-delete case is the passing request-order control; both cold,
// detail-first cases must pass after the deletion invalidation fix.
#[derive(Clone, Copy, Debug)]
enum DeletionSequence {
    DetailFirstTitleAndBody,
    IndexBeforeDelete,
    TitleOnly,
}

fn deletion_sequence_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dev-deletion-sequence")
}

fn sequence_entry(title: &str, body: &str) -> String {
    format!("---\ntitle: {title}\n---\n{body}\n")
}

/// Poll the actual index response; a disk-only check would miss request-time
/// lazy rendering. Keep the last response and request count in failure output.
async fn poll_sequence_index_without_slug(
    client: &reqwest::Client,
    url: &str,
    slug: &str,
) -> Result<String, String> {
    let start = Instant::now();
    let mut attempts = 0;
    let mut last = String::from("no response");
    while start.elapsed() < SCENARIO_DEADLINE {
        attempts += 1;
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 200 && !body.contains(slug) {
                    eprintln!(
                        "[deletion-sequence] GET / retracted {slug} after {attempts} requests in {:?}",
                        start.elapsed()
                    );
                    return Ok(body);
                }
                last = format!("status {status}; body {body}");
            }
            Err(error) => last = format!("request error: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "GET / still contains {slug} after {attempts} requests in {:?}; last observation: {last}",
        start.elapsed()
    ))
}

/// Issue #4783: a removed URL must 404 *because* it left the live route
/// table, not because a still-known route failed to render.
fn removed_route_observation(response: &reqwest::Response) -> Result<(), String> {
    let status = response.status().as_u16();
    let reason = response
        .headers()
        .get("x-zfb-dev-404-reason")
        .and_then(|v| v.to_str().ok());
    match (status, reason) {
        (404, Some("not-in-route-table")) => Ok(()),
        (status, reason) => Err(format!("status {status}, x-zfb-dev-404-reason {reason:?}")),
    }
}

async fn poll_sequence_deleted_detail(client: &reqwest::Client, url: &str) -> Result<u16, String> {
    let start = Instant::now();
    let mut attempts = 0;
    let mut last = String::from("no response");
    while start.elapsed() < SCENARIO_DEADLINE {
        attempts += 1;
        match client.get(url).send().await {
            Ok(response) => match removed_route_observation(&response) {
                Ok(()) => {
                    eprintln!(
                        "[deletion-sequence] deleted detail returned 404 not-in-route-table after {attempts} requests in {:?}",
                        start.elapsed()
                    );
                    return Ok(404);
                }
                Err(observation) => last = observation,
            },
            Err(error) => last = format!("request error: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "deleted detail did not return 404 not-in-route-table after {attempts} requests in {:?}; last observation: {last}",
        start.elapsed()
    ))
}

async fn run_deleted_collection_entry_sequence(sequence: DeletionSequence) {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create deletion-sequence fixture root");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&deletion_sequence_fixture_dir(), &root).expect("copy deletion-sequence fixture");
    let app_root = root.join("app");
    // Render markdown bodies in both surviving consumers so a body-only edit
    // has an observable response, and add an unrelated route for narrowing.
    let index_page = app_root.join("pages/index.tsx");
    let index_source = fs::read_to_string(&index_page).expect("read sequence index page");
    assert!(index_source.contains("<p key={entry.slug}>"));
    fs::write(
        &index_page,
        index_source
            .replace("<p key={entry.slug}>", "<div key={entry.slug}>")
            .replace("</p>", "<entry.Content components={{}} /></div>"),
    )
    .expect("make sequence index render entry bodies");
    let detail_page = app_root.join("pages/[slug].tsx");
    let detail_source = fs::read_to_string(&detail_page).expect("read sequence detail page");
    assert!(detail_source.contains("{entry ? String(entry.data.title) : \"Not found\"}"));
    fs::write(
        &detail_page,
        detail_source.replace(
            "{entry ? String(entry.data.title) : \"Not found\"}",
            "{entry ? <><h1>{String(entry.data.title)}</h1><entry.Content components={{}} /></> : \"Not found\"}",
        ),
    )
    .expect("make sequence detail render entry bodies");
    fs::write(
        app_root.join("pages/unrelated.tsx"),
        "export default function Unrelated() { return <html><body>UNRELATED_ROUTE</body></html>; }\n",
    )
    .expect("add unrelated sequence route");
    assert!(
        !app_root.join(".zfb-build").exists(),
        "fixture must start cold"
    );
    fs::create_dir_all(app_root.join("data")).expect("create watched data directory");
    let probe = app_root.join("data/handshake-probe.json");
    fs::write(&probe, "{\"revision\":0}\n").expect("seed watcher probe");
    // Keep the source report's workspace/app topology while using this
    // checkout's compiled zfb binary instead of installing published packages.
    let mut session = spawn_dev(app_root, DevMode::Lazy, Some("cold"));
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) = boot_and_handshake_banner_only(&mut session, &probe).await
        else {
            panic!(
                "{sequence:?}: deletion probe was not observed; scenario did not run\n{}",
                session.logs()
            );
        };
        let route_count = parse_cold_lazy_route_count(&read_log(&session.stdout_path));
        assert!(
            matches!(route_count, Some(n) if n > 0),
            "{sequence:?}: cold-lazy boot was not confirmed; count {route_count:?}\n{}",
            session.logs()
        );

        let slug = "watch-sequence";
        let source = session.root.join(format!("src/content/docs/{slug}.md"));
        let detail_url = format!("{base}/{slug}/");
        let index_url = format!("{base}/");
        let first_title = "FIRST_SEQUENCE_TITLE";
        let updated_title = "UPDATED_SEQUENCE_TITLE";
        let first_body = "Body";
        let updated_body = if matches!(sequence, DeletionSequence::TitleOnly) {
            first_body
        } else {
            "Updated body"
        };
        let started = Instant::now();

        fs::write(&source, sequence_entry(first_title, first_body))
            .expect("create collection entry");
        poll_until_response_contains(
            &client,
            &detail_url,
            first_title,
            "created detail title",
            &session,
        )
        .await;
        eprintln!(
            "[deletion-sequence] {sequence:?} created detail observed at {:?}",
            started.elapsed()
        );

        fs::write(&source, sequence_entry(updated_title, updated_body))
            .expect("edit collection entry");
        assert_eq!(
            fs::read_to_string(&source).expect("read edited collection entry"),
            sequence_entry(updated_title, updated_body),
            "edited source must contain the requested title and body before the detail poll"
        );
        poll_until_response_contains(
            &client,
            &detail_url,
            updated_title,
            "edited detail title",
            &session,
        )
        .await;
        eprintln!("[deletion-sequence] {sequence:?} edited detail observed at {:?}; title={updated_title}; body={updated_body}", started.elapsed());

        if matches!(sequence, DeletionSequence::IndexBeforeDelete) {
            poll_until_response_contains(
                &client,
                &index_url,
                updated_title,
                "control index before unlink",
                &session,
            )
            .await;
            eprintln!(
                "[deletion-sequence] {sequence:?} updated index observed before unlink at {:?}",
                started.elapsed()
            );
        }

        let index_file = session.html_root().join("index.html");
        eprintln!(
            "[deletion-sequence] {sequence:?} before unlink at {:?}: index_exists={}, index_contains_slug={}",
            started.elapsed(),
            index_file.exists(),
            fs::read_to_string(&index_file).map(|s| s.contains(slug)).unwrap_or(false)
        );
        fs::remove_file(&source).expect("delete collection entry");
        eprintln!(
            "[deletion-sequence] {sequence:?} unlinked at {:?}; GET / polling begins",
            started.elapsed()
        );

        // Keep both post-delete observations even if the index is stale.
        let index_result = poll_sequence_index_without_slug(&client, &index_url, slug).await;
        let detail_result = poll_sequence_deleted_detail(&client, &detail_url).await;
        let diagnostic_logs = if std::env::var_os("ZFB_DELETION_DIAGNOSTIC").is_some() {
            session.logs()
        } else {
            String::new()
        };
        eprintln!(
            "[deletion-sequence] {sequence:?} final at {:?}: index={:?}; detail={detail_result:?}; index_disk_contains_slug={}\n{}",
            started.elapsed(),
            index_result,
            fs::read_to_string(&index_file).map(|s| s.contains(slug)).unwrap_or(false),
            diagnostic_logs
        );
        assert!(
            index_result.is_ok(),
            "{sequence:?}: {}\n{}",
            index_result.unwrap_err(),
            session.logs()
        );
        assert!(
            detail_result.is_ok(),
            "{sequence:?}: {}\n{}",
            detail_result.unwrap_err(),
            session.logs()
        );
        let seed_url = format!("{base}/seed/");
        let unrelated_url = format!("{base}/unrelated/");
        poll_until_response_contains(
            &client,
            &seed_url,
            "Seed body",
            "surviving detail before body edit",
            &session,
        )
        .await;
        poll_until_response_contains(
            &client,
            &unrelated_url,
            "UNRELATED_ROUTE",
            "unrelated route before body edit",
            &session,
        )
        .await;
        drain_ticks_until_quiescent(&base).await;
        let unrelated_file = session.html_root().join("unrelated/index.html");
        let unrelated_before = snapshot_file(&unrelated_file);
        fs::write(
            session.root.join("src/content/docs/seed.md"),
            "---\ntitle: Seed\n---\nSURVIVING_BODY_AFTER_DELETE\n",
        )
        .expect("make body-only edit to surviving entry");
        poll_until_response_contains(
            &client,
            &seed_url,
            "SURVIVING_BODY_AFTER_DELETE",
            "surviving detail after body-only edit",
            &session,
        )
        .await;
        poll_until_response_contains(
            &client,
            &index_url,
            "SURVIVING_BODY_AFTER_DELETE",
            "aggregate index after body-only edit",
            &session,
        )
        .await;
        poll_until_response_contains(
            &client,
            &unrelated_url,
            "UNRELATED_ROUTE",
            "unrelated route after body-only edit",
            &session,
        )
        .await;
        assert_snapshot_unchanged(
            &unrelated_file,
            &unrelated_before,
            "post-delete body-only edit narrows past unrelated route",
            &session,
        );
        let logs = session.logs();
        assert!(
            !logs.contains("outside collection"),
            "{sequence:?}: stale membership warning\n{logs}"
        );
        assert!(
            !logs.contains("content provenance unavailable after lazy request render"),
            "{sequence:?}: provenance unavailable\n{logs}"
        );
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(Duration::from_secs(360), body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] deletion sequence {sequence:?} exceeded 360s; process group {pgid} will be killed.\n{}",
            session.logs()
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_deleted_entry_detail_first_title_and_body() {
    run_deleted_collection_entry_sequence(DeletionSequence::DetailFirstTitleAndBody).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_deleted_entry_index_before_delete_control() {
    run_deleted_collection_entry_sequence(DeletionSequence::IndexBeforeDelete).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_deleted_entry_title_only() {
    run_deleted_collection_entry_sequence(DeletionSequence::TitleOnly).await;
}

/// Poll until `url` answers 404 with the `not-in-route-table` reason. Used
/// for routes that a collection change legitimately removed; returns the last
/// observation on timeout.
async fn poll_until_status_404(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let start = Instant::now();
    let mut last = String::from("no response");
    while start.elapsed() < SCENARIO_DEADLINE {
        match client.get(url).send().await {
            Ok(response) => match removed_route_observation(&response) {
                Ok(()) => return Ok(()),
                Err(observation) => last = observation,
            },
            Err(error) => last = format!("request error: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(last)
}

/// Poll `url` until it answers 200 with a body that satisfies `accept`.
async fn poll_until_body_matches(
    client: &reqwest::Client,
    url: &str,
    accept: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let start = Instant::now();
    let mut last = String::from("no response");
    while start.elapsed() < SCENARIO_DEADLINE {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 200 && accept(&body) {
                    return Ok(());
                }
                last = format!("status {status}, body:\n{body}");
            }
            Err(error) => last = format!("request error: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(last)
}

/// Write `bytes` to an existing file through the same inode
/// (`O_WRONLY|O_TRUNC`, never `O_CREAT`), so the watcher reports an in-place
/// modification rather than the create/rename shape an atomic save produces.
/// An atomic save would route the restore through watch-ADD discovery, which
/// already folds changed route sources into the tick and hides #4764.
/// The truncate and the write can surface as separate ticks, as a real
/// in-place editor save can; every assertion below polls for an end state.
fn write_in_place(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let inode_before = std::os::unix::fs::MetadataExt::ino(&fs::metadata(path).expect("stat"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .expect("open existing file in place");
    file.write_all(bytes).expect("write in place");
    file.sync_all().expect("fsync in-place write");
    drop(file);
    let inode_after = std::os::unix::fs::MetadataExt::ino(&fs::metadata(path).expect("stat"));
    assert_eq!(
        inode_before, inode_after,
        "in-place write must keep the inode"
    );
}

#[derive(Clone, Copy, Debug)]
enum EmptyCollectionRecovery {
    /// The paginated routes were served before the collection emptied (#4764).
    VisitedFirst,
    /// No paginated route was ever requested before it disappeared.
    NeverRequested,
    /// Like `VisitedFirst`, but every write is an atomic save
    /// (temp sibling + rename), which reaches the watcher as Created/Removed.
    VisitedFirstAtomicSave,
}

/// Write `bytes` the way an atomic-saving editor does: a temp sibling in the
/// same directory, then `rename` over the target (a new inode).
fn write_atomic(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(format!(
        ".{}.e2e-tmp",
        path.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&tmp, bytes).expect("write atomic temp sibling");
    fs::rename(&tmp, path).expect("rename temp sibling over target");
}

/// Issue #4764: an in-place invalid-frontmatter edit used to empty the
/// collection (ZB024), and restoring the original bytes had to bring each
/// paginated URL back with its original membership. Since #4781 the failed
/// snapshot is rejected and the last good generation keeps serving, so the
/// invalid phase asserts that instead; the restore assertions are unchanged.
///
/// The same session then proves an ordinary edit still propagates to the
/// returned pages and that a genuine deletion still shrinks the pagination.
async fn run_empty_collection_recovery(variant: EmptyCollectionRecovery) {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create empty-collection recovery fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize empty-collection recovery fixture");
    copy_dir(&fixture_dir(), &root).expect("copy empty-collection recovery fixture");
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");

    // Timing lines (selected pages, route-table misses) make a failure here
    // diagnosable from the panic output alone.
    let mut session = spawn_dev_with_timing(root, DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };

        // Dates sort the pagination newest first: alpha, beta, warmup.
        let page_urls: Vec<String> = (1..=3).map(|n| format!("{base}/posts/page/{n}/")).collect();
        let expected_page_titles = ["Alpha V1 Frontmatter", "Beta Frontmatter", "Warmup"];
        let page_has_only = |expected: &'static str| {
            move |body: &str| {
                body.contains(expected)
                    && [
                        "Alpha V1 Frontmatter",
                        "Beta Frontmatter",
                        "<h2>Warmup</h2>",
                    ]
                    .iter()
                    .filter(|other| !other.contains(expected))
                    .all(|other| !body.contains(other))
            }
        };
        let index_url = format!("{base}/posts/");
        let beta_url = format!("{base}/posts/beta/");
        let tag_url = format!("{base}/tags/guide/");

        let write: fn(&Path, &[u8]) =
            if matches!(variant, EmptyCollectionRecovery::VisitedFirstAtomicSave) {
                write_atomic
            } else {
                write_in_place
            };
        if !matches!(variant, EmptyCollectionRecovery::NeverRequested) {
            for (url, title) in page_urls.iter().zip(expected_page_titles) {
                poll_until_body_matches(&client, url, page_has_only(title))
                    .await
                    .unwrap_or_else(|last| {
                        panic!(
                            "[{variant:?}] baseline {url} not served: {last}\n{}",
                            session.logs()
                        )
                    });
            }
        }
        poll_until_response_contains(
            &client,
            &index_url,
            "Beta Frontmatter",
            "baseline post index",
            &session,
        )
        .await;

        // Invalid YAML fails the content snapshot (ZB024). Issue #4781: the
        // failed candidate is rejected, so the last successful generation
        // keeps serving instead of an empty collection. An in-place write can
        // surface its truncation as a separate, valid tick, so beta's own
        // title may be the truncated generation's; alpha and the three
        // pagination routes prove the collection never emptied.
        let beta_path = session.root.join("content/posts/beta.md");
        let original_beta = fs::read(&beta_path).expect("read original beta bytes");
        let offset = read_log(&session.stderr_path).len();
        write(
            &beta_path,
            b"---\ntitle: [unclosed\ndate: 2026-01-01\n---\n\nbroken\n",
        );
        wait_for_stderr_line(
            &session,
            offset,
            &["was not published"],
            "invalid beta rejected as a content candidate",
        )
        .await;
        assert!(
            read_log(&session.stderr_path)[offset..].contains("ZB024"),
            "[{variant:?}] the rejected snapshot must be reported as ZB024\n{}",
            session.logs()
        );
        drain_ticks_until_quiescent(&base).await;
        let response = client.get(&index_url).send().await.expect("GET index");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        assert!(
            status == 200 && text.contains("Alpha V1 Frontmatter"),
            "[{variant:?}] the post index must keep the last good generation: {status}\n\
             {text}\n{}",
            session.logs()
        );
        for url in &page_urls {
            let response = client.get(url).send().await.expect("GET page");
            assert_eq!(
                response.status().as_u16(),
                200,
                "[{variant:?}] {url} must stay in the last good route table\n{}",
                session.logs()
            );
        }
        assert!(
            !read_log(&session.stderr_path).contains("will see empty collections"),
            "[{variant:?}] dev must not publish an empty generation\n{}",
            session.logs()
        );

        // Restore the exact original bytes, in place.
        write(&beta_path, &original_beta);
        for (url, title) in page_urls.iter().zip(expected_page_titles) {
            poll_until_body_matches(&client, url, page_has_only(title))
                .await
                .unwrap_or_else(|last| {
                    panic!(
                        "[{variant:?}] {url} did not return with {title:?} after the original \
                         bytes were restored in place: {last}\n{}",
                        session.logs()
                    )
                });
        }
        for (url, marker) in [
            (&index_url, "Beta Frontmatter"),
            (&beta_url, "V1-BODY-BETA"),
            (&tag_url, "Alpha V1 Frontmatter"),
        ] {
            poll_until_response_contains(&client, url, marker, "restored route", &session).await;
        }

        // A later ordinary edit still propagates to a returned page.
        write(
            &session.root.join("content/posts/alpha.md"),
            b"---\ntitle: Alpha Recovered Edit\ndate: 2026-01-02\ntags:\n  - guide\n---\n\nV2-BODY-ALPHA after recovery.\n",
        );
        poll_until_response_contains(
            &client,
            &page_urls[0],
            "Alpha Recovered Edit",
            "ordinary edit after recovery",
            &session,
        )
        .await;

        // A genuine deletion still shrinks the pagination: page 3 goes away
        // and warmup moves to page 2.
        fs::remove_file(&beta_path).expect("delete beta");
        poll_until_status_404(&client, &page_urls[2])
            .await
            .unwrap_or_else(|last| {
                panic!(
                    "[{variant:?}] /posts/page/3/ survived the deletion: {last}\n{}",
                    session.logs()
                )
            });
        poll_until_body_matches(&client, &page_urls[1], page_has_only("Warmup"))
            .await
            .unwrap_or_else(|last| {
                panic!(
                    "[{variant:?}] /posts/page/2/ did not move to warmup after the deletion: \
                     {last}\n{}",
                    session.logs()
                )
            });
        poll_until_status_404(&client, &beta_url)
            .await
            .unwrap_or_else(|last| {
                panic!(
                    "[{variant:?}] deleted beta entry still served: {last}\n{}",
                    session.logs()
                )
            });

        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(Duration::from_secs(360), body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] empty-collection recovery {variant:?} exceeded 360s; process group {pgid} \
             will be killed.\n{}",
            session.logs()
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_invalid_frontmatter_recovery_restores_visited_pagination() {
    run_empty_collection_recovery(EmptyCollectionRecovery::VisitedFirst).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_invalid_frontmatter_recovery_serves_never_requested_pagination() {
    run_empty_collection_recovery(EmptyCollectionRecovery::NeverRequested).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_invalid_frontmatter_recovery_with_atomic_saves() {
    run_empty_collection_recovery(EmptyCollectionRecovery::VisitedFirstAtomicSave).await;
}

fn post_with_tags(title: &str, date: &str, tags: &[&str], body: &str) -> Vec<u8> {
    let tags = if tags.is_empty() {
        "tags: []\n".to_string()
    } else {
        format!(
            "tags:\n{}",
            tags.iter()
                .map(|t| format!("  - {t}\n"))
                .collect::<String>()
        )
    };
    format!("---\ntitle: {title}\ndate: {date}\n{tags}---\n\n{body}\n").into_bytes()
}

/// #4771 lifecycle rows driven only by VALID metadata edits, all in place:
///
/// - a tag rename keeps the route count but swaps a URL (`/tags/guide/` goes,
///   `/tags/fresh/` comes);
/// - clearing every tag takes `tags/[tag]`'s `paths()` to zero routes, and
///   restoring one tag must bring its URL back on the first request;
/// - after that recovery another edit propagates, and a warm restart over the
///   same tree serves the latest bytes without resurrecting retired URLs.
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_valid_tag_edits_swap_empty_and_restore_routes_across_restart() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create tag lifecycle fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize tag lifecycle fixture");
    copy_dir(&fixture_dir(), &root).expect("copy tag lifecycle fixture");
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");
    let posts = root.join("content/posts");
    let (alpha, beta, warmup) = (
        posts.join("alpha.md"),
        posts.join("beta.md"),
        posts.join("__warmup.md"),
    );

    let mut session = spawn_dev_with_timing(root.clone(), DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };
        let url = |tag: &str| format!("{base}/tags/{tag}/");
        let expect_404 = |tag: &'static str, phase: &'static str| {
            let url = url(tag);
            let client = client.clone();
            async move {
                poll_until_status_404(&client, &url)
                    .await
                    .unwrap_or_else(|last| panic!("[{phase}] {url} still served: {last}"));
            }
        };

        // Baseline: visit every tag route.
        for (tag, marker) in [
            ("guide", "Alpha V1 Frontmatter"),
            ("other", "Beta Frontmatter"),
            ("warmup", "Warmup"),
        ] {
            poll_until_response_contains(&client, &url(tag), marker, "baseline tag", &session)
                .await;
        }

        // Same route count, different URL set.
        write_in_place(
            &alpha,
            &post_with_tags(
                "Alpha V1 Frontmatter",
                "2026-01-02",
                &["fresh"],
                "V1-BODY-ALPHA swap.",
            ),
        );
        poll_until_response_contains(
            &client,
            &url("fresh"),
            "Alpha V1 Frontmatter",
            "swapped-in tag",
            &session,
        )
        .await;
        expect_404("guide", "swapped-out tag").await;

        // Every tag cleared: tags/[tag] has zero routes.
        for (path, title, date, body) in [
            (
                &alpha,
                "Alpha V1 Frontmatter",
                "2026-01-02",
                "V1-BODY-ALPHA cleared.",
            ),
            (
                &beta,
                "Beta Frontmatter",
                "2026-01-01",
                "V1-BODY-BETA cleared.",
            ),
            (&warmup, "Warmup", "2025-01-01", "Warmup cleared."),
        ] {
            write_in_place(path, &post_with_tags(title, date, &[], body));
        }
        for tag in ["fresh", "other", "warmup"] {
            expect_404(tag, "all tags cleared").await;
        }

        // One tag returns; its URL must serve on the first request.
        write_in_place(
            &alpha,
            &post_with_tags(
                "Alpha V1 Frontmatter",
                "2026-01-02",
                &["guide"],
                "V1-BODY-ALPHA back.",
            ),
        );
        poll_until_response_contains(
            &client,
            &url("guide"),
            "Alpha V1 Frontmatter",
            "returned tag",
            &session,
        )
        .await;
        expect_404("fresh", "retired tag stays retired").await;

        // A later edit propagates to the returned route.
        write_in_place(
            &alpha,
            &post_with_tags(
                "Alpha After Return",
                "2026-01-02",
                &["guide"],
                "V2-BODY-ALPHA.",
            ),
        );
        poll_until_response_contains(
            &client,
            &url("guide"),
            "Alpha After Return",
            "edit after return",
            &session,
        )
        .await;

        // Warm restart over the same tree.
        session.stop_gracefully().await;
        let mut second = spawn_dev_with_timing(root.clone(), DevMode::Lazy, Some("cold"), true);
        let Some((second_base, second_client)) =
            boot_and_handshake_banner_only(&mut second, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };
        poll_until_response_contains(
            &second_client,
            &format!("{second_base}/tags/guide/"),
            "Alpha After Return",
            "warm restart serves latest",
            &second,
        )
        .await;
        for tag in ["fresh", "other", "warmup"] {
            let url = format!("{second_base}/tags/{tag}/");
            poll_until_status_404(&second_client, &url)
                .await
                .unwrap_or_else(|last| {
                    panic!(
                        "[warm restart] retired {url} resurrected: {last}\n{}",
                        second.logs()
                    )
                });
        }
        second.stop_gracefully().await;
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(Duration::from_secs(360), body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] tag lifecycle exceeded 360s; process group {pgid} will be killed.\n{}",
            session.logs()
        ),
    }
}

const MEMO_INVALID_B: &[u8] = b"---\ntitle: [unclosed\n---\n\nBeta body.\n";

fn memo_session_root(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize memo-helper fixture root");
    copy_dir(&memo_helper_fixture_dir(), &root).expect("copy memo-helper fixture");
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");
    (root, data_probe_path)
}

fn home_lists(body: &str, titles: &[&str]) -> bool {
    body.contains("MEMO-HOME") && titles.iter().all(|title| body.contains(title))
}

/// Issues #4781 / #4776: the home page reads entries through a module-level
/// memo that a dynamic route's `paths()` fills. An in-place invalid
/// frontmatter write to b.md fails the content snapshot (ZB024). The failed
/// candidate is rejected, so the last successful generation keeps serving:
/// home keeps both titles and `/articles/b/` keeps its route. Restoring the
/// original bytes changes nothing visible, and a later valid, different
/// title reaches home and the detail page without a restart, which is the
/// recovery the memoized helper used to lose after an empty generation.
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_memoized_helper_invalid_frontmatter_keeps_last_good_and_recovers() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create tempdir for memo-helper fixture");
    let (root, data_probe_path) = memo_session_root(&temp);
    let mut session = spawn_dev_with_timing(root, DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };
        let home_url = format!("{base}/");
        let detail_a = format!("{base}/articles/a/");
        let detail_b = format!("{base}/articles/b/");
        for (url, marker, phase) in [
            (&home_url, "Beta Memo V1", "baseline home"),
            (&detail_a, "Alpha Memo V1", "baseline detail a"),
            (&detail_b, "Beta Memo V1", "baseline detail b"),
        ] {
            poll_until_response_contains(&client, url, marker, phase, &session).await;
        }

        let b_md = session.root.join("content/articles/b.md");
        let original_b = fs::read(&b_md).expect("read original b.md");
        let offset = read_log(&session.stderr_path).len();
        write_in_place(&b_md, MEMO_INVALID_B);
        wait_for_stderr_line(
            &session,
            offset,
            &["was not published"],
            "invalid b.md rejected as a content candidate",
        )
        .await;
        assert!(
            read_log(&session.stderr_path)[offset..].contains("ZB024"),
            "the rejected snapshot must be reported as ZB024\n{}",
            session.logs()
        );
        drain_ticks_until_quiescent(&base).await;
        for (url, marker) in [
            (&home_url, "Alpha Memo V1"),
            (&home_url, "Beta Memo V1"),
            (&detail_b, "Beta Memo V1"),
        ] {
            let response = client.get(url).send().await.expect("GET during failure");
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            assert!(
                status == 200 && text.contains(marker),
                "{url} must keep the last good generation ({marker}) while the snapshot \
                 fails; got {status}:\n{text}\n{}",
                session.logs()
            );
        }
        assert!(
            !read_log(&session.stderr_path).contains("will see empty collections"),
            "dev must not publish an empty generation for a failed snapshot\n{}",
            session.logs()
        );

        write_in_place(&b_md, &original_b);
        drain_ticks_until_quiescent(&base).await;
        poll_until_body_matches(&client, &home_url, |body| {
            home_lists(body, &["Alpha Memo V1", "Beta Memo V1"])
        })
        .await
        .unwrap_or_else(|last| {
            panic!(
                "home lost its titles after the original bytes were restored: {last}\n{}",
                session.logs()
            )
        });

        write_in_place(&b_md, MEMO_INVALID_B);
        write_in_place(
            &b_md,
            b"---\ntitle: Beta Memo Repaired\n---\n\nBeta body.\n",
        );
        poll_until_body_matches(&client, &home_url, |body| {
            home_lists(body, &["Alpha Memo V1", "Beta Memo Repaired"])
        })
        .await
        .unwrap_or_else(|last| {
            panic!(
                "home never showed the repaired b.md title through the memoized helper: \
                 {last}\n{}",
                session.logs()
            )
        });
        poll_until_response_contains(
            &client,
            &detail_b,
            "Beta Memo Repaired",
            "detail b after repair",
            &session,
        )
        .await;

        write_in_place(
            &session.root.join("content/articles/a.md"),
            b"---\ntitle: Alpha After Recovery\n---\n\nAlpha body.\n",
        );
        poll_until_body_matches(&client, &home_url, |body| {
            home_lists(body, &["Alpha After Recovery", "Beta Memo Repaired"])
        })
        .await
        .unwrap_or_else(|last| {
            panic!(
                "a later title edit did not reach home after recovery: {last}\n{}",
                session.logs()
            )
        });
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] memo-helper last-good dev E2E exceeded {}s; process group {pgid} will \
             be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

/// Issue #4781: a legitimately empty collection is a successful snapshot.
/// Removing every entry removes their URLs, and adding one back restores its
/// URL and the aggregate, all without a restart and without ZB024.
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_successful_empty_collection_removes_and_restores_urls() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create empty-collection fixture");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize empty-collection fixture");
    copy_dir(&fixture_dir(), &root).expect("copy empty-collection fixture");
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");
    let posts = root.join("content/posts");
    let entries: Vec<(PathBuf, Vec<u8>)> = fs::read_dir(&posts)
        .expect("list posts")
        .map(|entry| {
            let path = entry.expect("post entry").path();
            let bytes = fs::read(&path).expect("read post");
            (path, bytes)
        })
        .collect();

    let mut session = spawn_dev_with_timing(root, DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };
        let index_url = format!("{base}/posts/");
        let beta_url = format!("{base}/posts/beta/");
        poll_until_response_contains(&client, &beta_url, "V1-BODY-BETA", "baseline", &session)
            .await;
        poll_until_response_contains(
            &client,
            &index_url,
            "Beta Frontmatter",
            "baseline index",
            &session,
        )
        .await;

        for (path, _) in &entries {
            fs::remove_file(path).expect("remove post");
        }
        poll_until_status_404(&client, &beta_url)
            .await
            .unwrap_or_else(|last| {
                panic!(
                    "beta survived an empty collection: {last}\n{}",
                    session.logs()
                )
            });
        poll_until_body_matches(&client, &index_url, |body| {
            body.contains("Post index") && !body.contains("Beta Frontmatter")
        })
        .await
        .unwrap_or_else(|last| {
            panic!("the index kept a removed entry: {last}\n{}", session.logs())
        });

        let (beta_path, beta_bytes) = entries
            .iter()
            .find(|(path, _)| path.ends_with("beta.md"))
            .expect("fixture has beta.md");
        fs::write(beta_path, beta_bytes).expect("recreate beta");
        poll_until_response_contains(&client, &beta_url, "V1-BODY-BETA", "beta returns", &session)
            .await;
        poll_until_response_contains(
            &client,
            &index_url,
            "Beta Frontmatter",
            "index lists beta again",
            &session,
        )
        .await;
        let logs = session.logs();
        assert!(
            !logs.contains("ZB024"),
            "an empty collection is not a snapshot failure\n{logs}"
        );
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] successful empty collection E2E exceeded {}s; process group {pgid} \
             will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

#[derive(Clone, Copy, Debug)]
enum FirstBootShape {
    /// Default cold-lazy boot: the bundle is deferred past bind.
    ColdDeferred,
    /// `ZFB_DEV_DEFER_BUNDLE=0`: the eager pre-bind boot falls back to the scaffold.
    ColdEagerBoot,
    /// `ZFB_DEV_EAGER=1`: no request-time render hook at all.
    Eager,
}

/// Issue #4781: when the very first content snapshot fails there is no last
/// good generation. The server stays up with unpublished routes, so a dev
/// 404 says `unavailable` rather than claiming the URL is gone, and fixing
/// the source publishes the routes without a restart.
async fn run_first_boot_content_failure(shape: FirstBootShape) {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create tempdir for memo-helper fixture");
    let (root, _) = memo_session_root(&temp);
    let b_md = root.join("content/articles/b.md");
    fs::write(&b_md, MEMO_INVALID_B).expect("break b.md before boot");

    let (mode, boot_lazy) = match shape {
        FirstBootShape::Eager => (DevMode::Eager, None),
        _ => (DevMode::Lazy, Some("cold")),
    };
    let extra: &[(&str, &str)] = match shape {
        FirstBootShape::ColdEagerBoot => &[("ZFB_DEV_DEFER_BUNDLE", "0")],
        _ => &[],
    };
    let mut session = spawn_dev_with_env(root, mode, boot_lazy, true, extra);
    let pgid = session.guard.pgid;
    let body = async {
        let Some(port) = wait_for_listening_port(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };
        let base = format!("http://localhost:{port}");
        let client = build_reqwest_client();
        // The boot hook runs after the watcher is armed, so an edit made
        // after this line is observed.
        wait_for_stderr_line(
            &session,
            0,
            &["[zfb-timing] boot: render complete"],
            "boot hook finished",
        )
        .await;
        assert_ready_banner(&session, false, "rejected first generation");
        let logs = session.logs();
        assert!(
            logs.contains("ZB024"),
            "[{shape:?}] the failed first snapshot must report ZB024\n{logs}"
        );
        assert!(
            !logs.contains("renderer disabled"),
            "[{shape:?}] a content failure must keep a recoverable session\n{logs}"
        );
        for url in [format!("{base}/"), format!("{base}/articles/a/")] {
            let response = client.get(&url).send().await.expect("GET before repair");
            assert_eq!(response.status().as_u16(), 404, "[{shape:?}] {url}");
            assert_eq!(
                response
                    .headers()
                    .get("x-zfb-dev-404-reason")
                    .and_then(|v| v.to_str().ok()),
                Some("unavailable"),
                "[{shape:?}] {url} has no published route table yet\n{}",
                session.logs()
            );
        }

        write_in_place(&b_md, b"---\ntitle: Beta Memo Fixed\n---\n\nBeta body.\n");
        poll_until_body_matches(&client, &format!("{base}/"), |body| {
            home_lists(body, &["Alpha Memo V1", "Beta Memo Fixed"])
        })
        .await
        .unwrap_or_else(|last| {
            panic!(
                "[{shape:?}] home never recovered after the source was fixed: {last}\n{}",
                session.logs()
            )
        });
        poll_until_response_contains(
            &client,
            &format!("{base}/articles/b/"),
            "Beta Memo Fixed",
            "detail b after first-boot repair",
            &session,
        )
        .await;
        wait_for_ready_banner(&session, "first-boot content repair").await;
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] first-boot content failure {shape:?} exceeded {}s; process group \
             {pgid} will be killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn first_boot_content_failure_recovers_cold_deferred() {
    run_first_boot_content_failure(FirstBootShape::ColdDeferred).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_boot_content_failure_recovers_cold_eager_boot() {
    run_first_boot_content_failure(FirstBootShape::ColdEagerBoot).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_boot_content_failure_recovers_eager() {
    run_first_boot_content_failure(FirstBootShape::Eager).await;
}

/// The bind line's port. A rejected first generation withholds the `ready on`
/// banner (#4261: it announces published routes), so first-boot failure
/// tests find the port here and expect the banner only after recovery.
async fn wait_for_listening_port(session: &mut DevSession) -> Option<u16> {
    let start = Instant::now();
    loop {
        let stdout = read_log(&session.stdout_path);
        if let Some(port) = stdout
            .lines()
            .filter_map(|line| line.split("listening on ").nth(1))
            .filter_map(|rest| rest.split_whitespace().next())
            .filter_map(|addr| addr.rsplit_once(':'))
            .find_map(|(_, port)| port.parse().ok())
        {
            return Some(port);
        }
        if session.guard.try_exit_status().is_some() {
            panic!("`zfb dev` exited before binding.\n{}", session.logs());
        }
        assert!(
            start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` never printed its bind line.\n{}",
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_ready_banner(session: &DevSession, phase: &str) {
    let start = Instant::now();
    while parse_ready_port(&read_log(&session.stdout_path)).is_none() {
        assert!(
            start.elapsed() < SCENARIO_DEADLINE,
            "[{phase}] the ready banner never followed recovery\n{}",
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn assert_ready_banner(session: &DevSession, expected: bool, phase: &str) {
    let ready = parse_ready_port(&read_log(&session.stdout_path)).is_some();
    assert_eq!(
        ready,
        expected,
        "[{phase}] ready banner presence\n{}",
        session.logs()
    );
}

fn slug_agreement_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dev-paths-slug-agreement")
}

fn slug_session_root(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let root = temp
        .path()
        .canonicalize()
        .expect("canonicalize slug-agreement fixture root");
    copy_dir(&slug_agreement_fixture_dir(), &root).expect("copy slug-agreement fixture");
    fs::create_dir_all(root.join("data")).expect("create handshake-probe data directory");
    let data_probe_path = root.join("data/handshake-probe.json");
    fs::write(&data_probe_path, "{\"revision\":0}\n").expect("write initial handshake probe");
    (root, data_probe_path)
}

async fn get_status_and_body(client: &reqwest::Client, url: &str) -> (u16, Option<String>, String) {
    let response = client.get(url).send().await.expect("GET");
    let status = response.status().as_u16();
    let reason = response
        .headers()
        .get("x-zfb-dev-404-reason")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (status, reason, response.text().await.unwrap_or_default())
}

/// Issue #4782: the fixture's helper throws when an entry's filename slug
/// disagrees with its frontmatter `slug`, as the Field Notes original helper
/// does. A filename-only rename makes `paths()` throw. Dev rejects that
/// candidate route table instead of publishing one without the article URLs,
/// so the last good routes keep serving; restoring the filename and later
/// valid edits recover without a restart. Controls: a valid slug rename moves
/// the URL (old `not-in-route-table`, new 200), a deletion removes a URL, and
/// a successful empty `paths()` result removes every article URL.
#[tokio::test(flavor = "multi_thread")]
async fn cold_lazy_failed_paths_keeps_last_good_route_table() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create slug-agreement fixture");
    let (root, data_probe_path) = slug_session_root(&temp);
    let mut session = spawn_dev_with_timing(root, DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some((base, client)) =
            boot_and_handshake_banner_only(&mut session, &data_probe_path).await
        else {
            return ScenarioOutcome::Skipped;
        };
        let home = format!("{base}/");
        let url = |slug: &str| format!("{base}/articles/{slug}/");
        for (target, marker) in [
            (home.clone(), "Beta Slug"),
            (url("a"), "Alpha Slug"),
            (url("b"), "Beta Slug"),
        ] {
            poll_until_response_contains(&client, &target, marker, "baseline", &session).await;
        }

        let articles = session.root.join("content/articles");
        let (a_md, renamed) = (articles.join("a.md"), articles.join("a-renamed.md"));
        let offset = read_log(&session.stderr_path).len();
        fs::rename(&a_md, &renamed).expect("filename-only rename");
        wait_for_stderr_line(
            &session,
            offset,
            &["paths() failed in dev", "was not published"],
            "filename-only rename rejected",
        )
        .await;
        drain_ticks_until_quiescent(&base).await;
        for (target, marker) in [
            (url("a"), "Alpha Slug"),
            (url("b"), "Beta Slug"),
            (home.clone(), "Alpha Slug"),
        ] {
            let (status, _, text) = get_status_and_body(&client, &target).await;
            assert!(
                status == 200 && text.contains(marker),
                "{target} must keep the last good route ({marker}) while paths() fails; got \
                 {status}:\n{text}\n{}",
                session.logs()
            );
        }
        let (status, reason, _) = get_status_and_body(&client, &url("a-renamed")).await;
        assert_eq!(
            (status, reason.as_deref()),
            (404, Some("not-in-route-table")),
            "the rejected candidate's URL must not be published\n{}",
            session.logs()
        );
        assert!(
            !read_log(&session.stderr_path).contains("not expanded in dev"),
            "a failed paths() must not publish a partial route table\n{}",
            session.logs()
        );

        fs::rename(&renamed, &a_md).expect("restore the filename");
        write_in_place(
            &articles.join("b.md"),
            b"---\ntitle: Beta Slug Edited\nslug: b\n---\n\nBeta body.\n",
        );
        poll_until_body_matches(&client, &home, |body| {
            body.contains("Alpha Slug") && body.contains("Beta Slug Edited")
        })
        .await
        .unwrap_or_else(|last| panic!("home did not recover: {last}\n{}", session.logs()));
        poll_until_response_contains(
            &client,
            &url("b"),
            "Beta Slug Edited",
            "detail b after recovery",
            &session,
        )
        .await;

        let c_md = articles.join("c.md");
        fs::write(
            &c_md,
            b"---\ntitle: Alpha Slug\nslug: c\n---\n\nAlpha body.\n",
        )
        .expect("write the renamed entry");
        fs::remove_file(&a_md).expect("remove the old entry");
        poll_until_response_contains(&client, &url("c"), "Alpha Slug", "valid rename", &session)
            .await;
        poll_until_status_404(&client, &url("a"))
            .await
            .unwrap_or_else(|last| {
                panic!(
                    "the old URL survived a valid rename: {last}\n{}",
                    session.logs()
                )
            });

        fs::remove_file(articles.join("b.md")).expect("delete b");
        poll_until_status_404(&client, &url("b"))
            .await
            .unwrap_or_else(|last| panic!("b survived its deletion: {last}\n{}", session.logs()));

        fs::remove_file(&c_md).expect("delete the last entry");
        poll_until_status_404(&client, &url("c"))
            .await
            .unwrap_or_else(|last| {
                panic!(
                    "c survived an empty paths() result: {last}\n{}",
                    session.logs()
                )
            });
        poll_until_body_matches(&client, &home, |body| {
            body.contains("SLUG-HOME") && !body.contains("Alpha Slug")
        })
        .await
        .unwrap_or_else(|last| panic!("home kept a removed entry: {last}\n{}", session.logs()));
        assert!(
            !read_log(&session.stderr_path)[offset..]
                .lines()
                .filter(|line| line.contains("paths() failed in dev"))
                .any(|line| !line.contains("a-renamed")),
            "only the filename-only rename is a paths() failure\n{}",
            session.logs()
        );
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] failed paths() dev E2E exceeded {}s; process group {pgid} will be \
             killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}

/// Issue #4782 at first boot: `paths()` fails before any route table was
/// published, so dev 404s say `unavailable`, and fixing the filename
/// publishes the routes without a restart.
#[tokio::test(flavor = "multi_thread")]
async fn first_boot_failed_paths_recovers_without_restart() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;

    let temp = tempfile::tempdir().expect("create slug-agreement fixture");
    let (root, _) = slug_session_root(&temp);
    let articles = root.join("content/articles");
    fs::rename(articles.join("a.md"), articles.join("a-renamed.md")).expect("break a.md");
    let mut session = spawn_dev_with_timing(root, DevMode::Lazy, Some("cold"), true);
    let pgid = session.guard.pgid;
    let body = async {
        let Some(port) = wait_for_listening_port(&mut session).await else {
            return ScenarioOutcome::Skipped;
        };
        let base = format!("http://localhost:{port}");
        let client = build_reqwest_client();
        wait_for_stderr_line(
            &session,
            0,
            &["[zfb-timing] boot: render complete"],
            "boot hook finished",
        )
        .await;
        assert_ready_banner(&session, false, "rejected first generation");
        assert!(
            read_log(&session.stderr_path).contains("paths() failed in dev"),
            "the first route table must be rejected\n{}",
            session.logs()
        );
        for target in [format!("{base}/"), format!("{base}/articles/b/")] {
            let (status, reason, _) = get_status_and_body(&client, &target).await;
            assert_eq!(
                (status, reason.as_deref()),
                (404, Some("unavailable")),
                "{target} has no published route table yet\n{}",
                session.logs()
            );
        }
        fs::rename(articles.join("a-renamed.md"), articles.join("a.md")).expect("fix a.md");
        poll_until_response_contains(
            &client,
            &format!("{base}/articles/a/"),
            "Alpha Slug",
            "first-boot paths recovery",
            &session,
        )
        .await;
        poll_until_response_contains(&client, &format!("{base}/"), "Beta Slug", "home", &session)
            .await;
        wait_for_ready_banner(&session, "first-boot paths repair").await;
        ScenarioOutcome::Completed
    };

    match tokio::time::timeout(OVERALL_DEADLINE, body).await {
        Ok(ScenarioOutcome::Completed) | Ok(ScenarioOutcome::Skipped) => {}
        Err(_) => panic!(
            "[watchdog] first-boot paths() failure exceeded {}s; process group {pgid} will be \
             killed.\n{}",
            OVERALL_DEADLINE.as_secs(),
            session.logs(),
        ),
    }
}
