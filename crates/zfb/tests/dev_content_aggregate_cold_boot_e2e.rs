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
use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

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
        self.root.join(".zfb-build").join("dev-pages")
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
fn spawn_dev(root: PathBuf, esbuild: &Path, mode: DevMode, boot_lazy: Option<&str>) -> DevSession {
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
            _ => break,
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

    let mut session = spawn_dev(root, &esbuild, DevMode::Eager, None);
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
        drain_ticks_until_quiescent(&base).await;
        let sse = subscribe_sse(&base).await;
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

        drain_ticks_until_quiescent(&base).await;
        let sse = subscribe_sse(&base).await;
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
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[content_aggregate_cold_boot_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

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
    let mut session = spawn_dev(root, &esbuild, DevMode::Lazy, Some("cold"));
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
            Ok(None) | Err(_) => eprintln!(
                "[content_aggregate_cold_boot_e2e] no SSE page event observed for the cold \
                 zero-prior-requests edit; relying on the authoritative first-request response \
                 checks."
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
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[content_aggregate_cold_boot_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

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
        let mut first = spawn_dev(root.clone(), &esbuild, DevMode::Lazy, None);
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

        let mut second = spawn_dev(root.clone(), &esbuild, DevMode::Lazy, None);
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
