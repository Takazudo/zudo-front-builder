//! Level-4 confirmation for Scratch Dir (epic #3339, sub-issue #3347): a live
//! `zfb dev --scratch-dir` session keeps serving its own `--define` while a
//! `zfb build` and a `zfb check` run against the same project, each in its own
//! scratch dir. The contract under test is `research/3318-scratch-dir-design.md`.
//!
//! Scenario (`concurrent_dev_build_check_are_isolated`):
//!
//! 1. A plain `zfb build --define __ORIGIN__='"B"'` fills the shared `dist/`
//!    with the WRONG define and records one hashed `dist/assets/islands-*.js`.
//! 2. `zfb dev --scratch-dir .zfb-build/session-a --define __ORIGIN__='"A"'`
//!    stays alive. It must log the single "ignoring prebuilt" line (#3344) and
//!    keep its bundle, `dev-pages`, `dev-assets` under `session-a`.
//! 3. While dev holds `session-a`, `zfb build` (session-b, define B) and
//!    `zfb check` (session-c) both succeed, and neither touches dev's bundle.
//! 4. One write per MDX file (a warmup entry, then `alpha`), each followed by
//!    a wait for its tick (`l-lessons-dev-watcher-narrowing`: write once,
//!    never loop rewriting).
//! 5. Dev serves `A` in SSR and in the island bundle before and after the
//!    recompile, 404s the prebuilt hashed asset that still sits in `dist/`,
//!    and its SSR/islands `zudoReactBuild` tokens agree with each other and
//!    differ from the build's (#3345).
//!
//! The other tests cover the placement rule (R2) at process level and a
//! Cloudflare-adapter build whose scratch dir lies outside the project.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const OVERALL_DEADLINE: Duration = Duration::from_secs(300);
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const TICK_DEADLINE: Duration = Duration::from_secs(45);
const SERVE_DEADLINE: Duration = Duration::from_secs(60);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

const SESSION_A: &str = ".zfb-build/session-a";
const SESSION_B: &str = ".zfb-build/session-b";
const SESSION_C: &str = ".zfb-build/session-c";
const LOCK_FILE: &str = ".zfb-scratch.lock";
const DEFINE_A: &str = "__ORIGIN__=\"A\"";
const DEFINE_B: &str = "__ORIGIN__=\"B\"";

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

struct Finished {
    success: bool,
    stdout: String,
    stderr: String,
}

impl Finished {
    fn combined(&self) -> String {
        format!(
            "--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout, self.stderr
        )
    }
}

fn run_zfb(root: &Path, esbuild: &Path, args: &[&str]) -> Finished {
    let output = Command::new(zfb_binary!())
        .args(args)
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env_remove("ZFB_SCRATCH_DIR")
        .output()
        .unwrap_or_else(|error| panic!("spawn `zfb {}`: {error}", args.join(" ")));
    Finished {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn run_zfb_ok(root: &Path, esbuild: &Path, args: &[&str]) -> Finished {
    let finished = run_zfb(root, esbuild, args);
    assert!(
        finished.success,
        "`zfb {}` must exit 0\n{}",
        args.join(" "),
        finished.combined()
    );
    finished
}

fn quoted_after(haystack: &str, prefix: &str) -> String {
    let rest = haystack
        .split(prefix)
        .nth(1)
        .unwrap_or_else(|| panic!("{prefix:?} not found in:\n{haystack}"));
    rest.split('"').next().unwrap().to_owned()
}

/// `data-zfb-build="<token>"`: the SSR-side `zudoReactBuild` identity.
fn html_build_token(html: &str) -> String {
    quoted_after(html, "data-zfb-build=\"")
}

fn island_asset_name(html: &str) -> String {
    let rest = html
        .split("/assets/islands")
        .nth(1)
        .unwrap_or_else(|| panic!("no islands script in:\n{html}"));
    let tail = rest.split('"').next().unwrap();
    format!("islands{tail}")
}

fn assert_origin(html: &str, island_bundle: &str, expected: &str, other: &str, context: &str) {
    assert!(
        html.contains(&format!("SSR-ORIGIN-{expected}")),
        "[{context}] SSR HTML must carry origin {expected}:\n{html}"
    );
    assert!(
        !html.contains(&format!("SSR-ORIGIN-{other}")),
        "[{context}] SSR HTML must not carry origin {other}:\n{html}"
    );
    let has_mark = |value: &str| {
        let compact: String = island_bundle
            .split("originMark")
            .skip(1)
            .map(|rest| {
                rest.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        compact.contains(&format!(":\"{value}\""))
    };
    assert!(
        has_mark(expected),
        "[{context}] island bundle must carry originMark {expected}"
    );
    assert!(
        !has_mark(other),
        "[{context}] island bundle must not carry originMark {other}"
    );
}

struct DevGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl Drop for DevGuard {
    fn drop(&mut self) {
        unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

struct DevSession {
    guard: DevGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

impl DevSession {
    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            read_log(&self.stdout_path),
            read_log(&self.stderr_path),
        )
    }
}

fn spawn_dev(root: &Path, esbuild: &Path, extra_args: &[&str]) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let mut command = Command::new(zfb_binary!());
    command
        .args(["dev", "--port", "0"])
        .args(extra_args)
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env("ZFB_DEV_TIMING", "1")
        .env_remove("ZFB_SCRATCH_DIR")
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()));
    command.process_group(0);
    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let idx = log.find("http://")?;
    let token = log[idx + "http://".len()..].split_whitespace().next()?;
    let digits: String = token
        .split_once(':')?
        .1
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

async fn wait_for_ready_port(session: &mut DevSession) -> u16 {
    let started = Instant::now();
    loop {
        if let Some(status) = session.guard.child.try_wait().expect("try_wait `zfb dev`") {
            panic!(
                "`zfb dev` exited early ({status:?}) before its ready banner.\n{}",
                session.logs()
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            return port;
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "no ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn tick_line_mentions(line: &str, filename: &str) -> bool {
    line.strip_prefix("[zfb-timing] tick(): kinds=[")
        .and_then(|rest| rest.rsplit_once(']'))
        .is_some_and(|(kinds, _)| {
            kinds.split(", ").any(|kind| {
                kind.split_once(':')
                    .is_some_and(|(name, _)| name == filename)
            })
        })
}

/// First complete stderr line after byte `offset` accepted by `matches`,
/// with the offset just past it.
fn log_line_since(
    stderr: &str,
    offset: usize,
    matches: impl Fn(&str) -> bool,
) -> Option<(String, usize)> {
    let mut end = offset;
    for line in stderr.get(offset..)?.split_inclusive('\n') {
        end += line.len();
        if line.ends_with('\n') && matches(line.trim_end()) {
            return Some((line.trim_end().to_string(), end));
        }
    }
    None
}

async fn wait_for_log_line_since(
    session: &DevSession,
    offset: usize,
    description: &str,
    matches: impl Fn(&str) -> bool,
) -> (String, usize) {
    let started = Instant::now();
    while started.elapsed() < TICK_DEADLINE {
        if let Some(found) = log_line_since(&read_log(&session.stderr_path), offset, &matches) {
            return found;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "no {description} after stderr byte {offset} within {}s.\n{}",
        TICK_DEADLINE.as_secs(),
        session.logs()
    );
}

/// Edit `path` exactly once and wait until the tick for `filename` has run
/// and drained its stale outcome.
async fn edit_once_and_wait_for_tick(
    session: &DevSession,
    path: &Path,
    filename: &str,
    contents: &str,
) {
    let offset = read_log(&session.stderr_path).len();
    fs::write(path, contents).expect("edit content entry once");
    let (_, tick_end) =
        wait_for_log_line_since(session, offset, &format!("tick for {filename}"), |line| {
            tick_line_mentions(line, filename)
        })
        .await;
    wait_for_log_line_since(session, tick_end, "tick outcome drain", |line| {
        line.starts_with("[zfb-timing] stale probe: drained pages_stale=")
    })
    .await;
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client")
}

async fn get(client: &reqwest::Client, url: &str) -> (u16, String) {
    let response = client
        .get(url)
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET {url}: {error}"));
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

async fn poll_until_contains(
    client: &reqwest::Client,
    url: &str,
    marker: &str,
    session: &DevSession,
) {
    let started = Instant::now();
    let mut last = String::from("(no response)");
    while started.elapsed() < SERVE_DEADLINE {
        if let Ok(response) = client.get(url).send().await {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if status == 200 && body.contains(marker) {
                return;
            }
            last = format!("status {status}:\n{body}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "GET {url} never served {marker:?} within {}s. Last: {last}\n{}",
        SERVE_DEADLINE.as_secs(),
        session.logs()
    );
}

/// What dev serves right now, for the before/after-recompile comparison.
struct DevView {
    html: String,
    island_bundle: String,
    ssr_token: String,
}

async fn dev_view(client: &reqwest::Client, base: &str, session: &DevSession) -> DevView {
    let (status, html) = get(client, &format!("{base}/")).await;
    assert_eq!(status, 200, "GET / on dev\n{}", session.logs());
    let asset = island_asset_name(&html);
    let (status, island_bundle) = get(client, &format!("{base}/assets/{asset}")).await;
    assert_eq!(
        status,
        200,
        "GET /assets/{asset} on dev\n{}",
        session.logs()
    );
    let ssr_token = html_build_token(&html);
    DevView {
        html,
        island_bundle,
        ssr_token,
    }
}

fn assert_dev_view(view: &DevView, build_token: &str, context: &str) {
    assert_origin(&view.html, &view.island_bundle, "A", "B", context);
    assert!(
        view.island_bundle.contains(&view.ssr_token),
        "[{context}] dev's islands bundle must carry the same zudoReactBuild token as its SSR \
         HTML ({}); hydration would reject a mismatch",
        view.ssr_token
    );
    assert_ne!(
        view.ssr_token, build_token,
        "[{context}] dev (define A) and build (define B) must not share a zudoReactBuild token"
    );
}

fn dir_entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read_dir {}: {error}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn concurrent_scenario() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[scratch_dir_concurrent_e2e] no esbuild binary available; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().canonicalize().expect("canonicalize root");
    copy_dir(&fixture_dir("scratch-dir-concurrent"), &root).expect("copy fixture");

    // Step 1: a prebuilt dist/ that carries the WRONG define.
    run_zfb_ok(&root, &esbuild, &["build", "--define", DEFINE_B]);
    let prebuilt_html = read_text(&root.join("dist/index.html"));
    let prebuilt_asset = island_asset_name(&prebuilt_html);
    let build_token = html_build_token(&prebuilt_html);
    assert!(
        root.join("dist/assets").join(&prebuilt_asset).is_file(),
        "step 1 must emit dist/assets/{prebuilt_asset}"
    );
    let default_bundle = read_bytes(&root.join(".zfb-build/bundle.mjs"));

    // Step 2: a live session dev with a different define.
    let mut session = spawn_dev(
        &root,
        &esbuild,
        &["--scratch-dir", SESSION_A, "--define", DEFINE_A],
    );
    let port = wait_for_ready_port(&mut session).await;
    let base = format!("http://localhost:{port}");
    let client = build_client();
    let started = Instant::now();
    loop {
        if matches!(client.get(format!("{base}/")).send().await, Ok(r) if r.status().as_u16() == 200)
        {
            break;
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "GET / never answered 200\n{}",
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let combined_logs = format!(
        "{}\n{}",
        read_log(&session.stdout_path),
        read_log(&session.stderr_path)
    );
    assert_eq!(
        combined_logs.matches("ignoring prebuilt").count(),
        1,
        "dev must log the 'ignoring prebuilt' line exactly once (#3344)\n{}",
        session.logs()
    );
    let session_a = root.join(SESSION_A);
    for name in [LOCK_FILE, "bundle.mjs", "dev-pages", "dev-assets"] {
        assert!(
            session_a.join(name).exists(),
            "dev must keep {name} under session-a; found {:?}",
            dir_entry_names(&session_a)
        );
    }
    assert!(
        !root.join(".zfb").exists(),
        "a scratch-dir dev must not create the shared .zfb/ (graph.bin lives in session-a)"
    );

    let before = dev_view(&client, &base, &session).await;
    assert_dev_view(&before, &build_token, "before build/check");
    let (status, _) = get(&client, &format!("{base}/assets/{prebuilt_asset}")).await;
    assert_eq!(
        status, 404,
        "dev must not serve the prebuilt dist/assets/{prebuilt_asset} (#3344)"
    );

    // Same scratch dir as the live dev: the lock refuses (#3342).
    let contended = run_zfb(
        &root,
        &esbuild,
        &["check", "--skip-tsc", "--scratch-dir", SESSION_A],
    );
    assert!(!contended.success, "{}", contended.combined());
    assert!(
        contended.stderr.contains("in use by another zfb process"),
        "second command on session-a must fail on the scratch lock\n{}",
        contended.combined()
    );

    // Step 3: build and check run beside the live dev.
    let session_a_bundle = read_bytes(&session_a.join("bundle.mjs"));
    run_zfb_ok(
        &root,
        &esbuild,
        &["build", "--scratch-dir", SESSION_B, "--define", DEFINE_B],
    );
    // `--skip-tsc`: the fixture has no TypeScript install for tsc to find.
    run_zfb_ok(
        &root,
        &esbuild,
        &["check", "--skip-tsc", "--scratch-dir", SESSION_C],
    );

    assert_eq!(
        read_bytes(&session_a.join("bundle.mjs")),
        session_a_bundle,
        "a concurrent build/check must not rewrite dev's session bundle"
    );
    assert_eq!(
        read_bytes(&root.join(".zfb-build/bundle.mjs")),
        default_bundle,
        "a scratch-dir build must not write the default .zfb-build/bundle.mjs"
    );
    let session_b = root.join(SESSION_B);
    assert_ne!(
        read_bytes(&session_b.join("bundle.mjs")),
        session_a_bundle,
        "session-b (define B) and session-a (define A) bundles must differ"
    );
    assert_eq!(
        dir_entry_names(&root.join(SESSION_C)),
        vec![LOCK_FILE.to_owned()],
        "check writes nothing zfb-owned into its scratch dir"
    );

    // The build carries B, including its islands token.
    let built_html = read_text(&root.join("dist/index.html"));
    let built_asset = island_asset_name(&built_html);
    let built_bundle = read_text(&root.join("dist/assets").join(&built_asset));
    assert_origin(&built_html, &built_bundle, "B", "A", "scratch build output");
    assert_eq!(
        html_build_token(&built_html),
        build_token,
        "the same define yields the same token across build invocations"
    );
    assert!(
        root.join("dist/assets").join(&prebuilt_asset).is_file(),
        "the prebuilt asset must still sit in dist/ so the 404 below proves dev never reads it"
    );

    let during = dev_view(&client, &base, &session).await;
    assert_dev_view(&during, &build_token, "after build/check");
    assert_eq!(during.ssr_token, before.ssr_token);
    let (status, _) = get(&client, &format!("{base}/assets/{prebuilt_asset}")).await;
    assert_eq!(status, 404, "dev must still 404 the prebuilt asset");

    // Step 4: watcher live, then ONE real edit.
    wait_for_log_line_since(&session, 0, "boot render completion", |line| {
        line == "[zfb-timing] boot: render complete"
    })
    .await;
    edit_once_and_wait_for_tick(
        &session,
        &root.join("content/posts/__warmup.mdx"),
        "__warmup.mdx",
        "---\ntitle: Warmup\ndate: 2025-01-01\n---\n\nWarmup revision 1.\n",
    )
    .await;
    poll_until_contains(
        &client,
        &format!("{base}/posts/__warmup"),
        "Warmup revision 1.",
        &session,
    )
    .await;
    poll_until_contains(
        &client,
        &format!("{base}/posts/alpha"),
        "V1-BODY-ALPHA",
        &session,
    )
    .await;
    edit_once_and_wait_for_tick(
        &session,
        &root.join("content/posts/alpha.mdx"),
        "alpha.mdx",
        "---\ntitle: Alpha\ndate: 2025-01-01\n---\n\nV2-BODY-ALPHA edited body.\n",
    )
    .await;
    poll_until_contains(
        &client,
        &format!("{base}/posts/alpha"),
        "V2-BODY-ALPHA",
        &session,
    )
    .await;

    // Step 5: still A after the recompile, tokens still agree.
    let after = dev_view(&client, &base, &session).await;
    assert_dev_view(&after, &build_token, "after MDX recompile");
    let (status, _) = get(&client, &format!("{base}/assets/{prebuilt_asset}")).await;
    assert_eq!(
        status, 404,
        "dev must 404 the prebuilt asset after recompile"
    );

    // Graceful stop persists the graph inside the session, not in `.zfb/`.
    unsafe { libc::kill(session.guard.child.id() as libc::pid_t, libc::SIGINT) };
    let stopped = Instant::now();
    while session.guard.child.try_wait().unwrap().is_none() {
        assert!(
            stopped.elapsed() < SHUTDOWN_DEADLINE,
            "dev did not exit on SIGINT\n{}",
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        session_a.join("graph.bin").is_file(),
        "graph.bin must be persisted under session-a; found {:?}",
        dir_entry_names(&session_a)
    );
    assert!(
        !root.join(".zfb").exists(),
        "no shared .zfb/graph.bin may appear"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_dev_build_check_are_isolated() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    if tokio::time::timeout(OVERALL_DEADLINE, concurrent_scenario())
        .await
        .is_err()
    {
        panic!("scenario exceeded {}s", OVERALL_DEADLINE.as_secs());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn in_project_scratch_dir_outside_zfb_build_is_rejected() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().canonicalize().unwrap();
    copy_dir(&fixture_dir("scratch-dir-concurrent"), &root).expect("copy fixture");
    let esbuild = locate_esbuild().unwrap_or_default();

    let finished = run_zfb(
        &root,
        &esbuild,
        &["check", "--skip-tsc", "--scratch-dir", "tmp-scratch"],
    );
    assert!(!finished.success, "{}", finished.combined());
    assert!(
        finished.stderr.contains("(R2)"),
        "R2 placement rule must name itself\n{}",
        finished.combined()
    );
    assert!(
        !root.join("tmp-scratch").exists(),
        "a rejected scratch dir must not be created"
    );

    let via_env = Command::new(zfb_binary!())
        .args(["check", "--skip-tsc"])
        .current_dir(&root)
        .env("ZFB_SCRATCH_DIR", "tmp-scratch")
        .output()
        .expect("spawn `zfb check`");
    assert!(!via_env.status.success());
    assert!(
        String::from_utf8_lossy(&via_env.stderr).contains("(R2)"),
        "ZFB_SCRATCH_DIR must hit the same rule"
    );
}

// ── Cloudflare adapter with an out-of-project scratch dir ─────────────────

const SSR_WASM_VALUE: u8 = 29;
const SSR_WASM: &[u8] = &[
    0x00,
    0x61,
    0x73,
    0x6d,
    0x01,
    0x00,
    0x00,
    0x00,
    0x01,
    0x05,
    0x01,
    0x60,
    0x00,
    0x01,
    0x7f,
    0x03,
    0x02,
    0x01,
    0x00,
    0x07,
    0x0a,
    0x01,
    0x06,
    0x61,
    0x6e,
    0x73,
    0x77,
    0x65,
    0x72,
    0x00,
    0x00,
    0x0a,
    0x06,
    0x01,
    0x04,
    0x00,
    0x41,
    SSR_WASM_VALUE,
    0x0b,
];
const SSG_WASM: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, 0x03,
    0x02, 0x01, 0x00, 0x07, 0x0a, 0x01, 0x06, 0x61, 0x6e, 0x73, 0x77, 0x65, 0x72, 0x00, 0x00, 0x0a,
    0x06, 0x01, 0x04, 0x00, 0x41, 17, 0x0b,
];

/// Node ESM loader hook: turns `import x from "./y.wasm"` into a
/// `WebAssembly.Module`, the shape Workers (and the emitted runtime page)
/// expect.
const WASM_LOADER_HOOK: &str = r#"import { readFileSync } from "node:fs";
export async function load(url, context, nextLoad) {
  if (url.endsWith(".wasm")) {
    const bytes = readFileSync(new URL(url));
    return {
      format: "module",
      shortCircuit: true,
      source: `export default new WebAssembly.Module(Uint8Array.from(${JSON.stringify([...bytes])}));`,
    };
  }
  return nextLoad(url, context);
}
"#;

const WORKER_DRIVER: &str = r#"import { register } from "node:module";
import { pathToFileURL } from "node:url";
register(pathToFileURL(process.argv[2]).href);
const worker = (await import(pathToFileURL(process.argv[3]).href)).default;
const env = { ASSETS: { fetch: async () => new Response("not found", { status: 404 }) } };
const ctx = { waitUntil: () => undefined, passThroughOnException: () => undefined };
const response = await worker.fetch(new Request("https://worker.test/runtime"), env, ctx);
console.log(JSON.stringify({ status: response.status, body: await response.text() }));
"#;

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_adapter_build_with_out_of_project_scratch_dir_runs_the_emitted_worker() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[scratch_dir_concurrent_e2e] no esbuild binary available; skipping.");
        return;
    };
    let pnpm_ok = Command::new("pnpm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let node_ok = Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !pnpm_ok || !node_ok {
        eprintln!("[scratch_dir_concurrent_e2e] pnpm or node unavailable; skipping.");
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("project");
    let scratch = temp.path().join("scratch");
    copy_dir(&fixture_dir("wasm-ssr-confirm"), &root).expect("copy wasm fixture");
    fs::create_dir_all(root.join("wasm")).unwrap();
    fs::write(root.join("wasm/ssg-only.wasm"), SSG_WASM).unwrap();
    fs::write(root.join("wasm/ssr-only.wasm"), SSR_WASM).unwrap();

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    let adapter = workspace_root.join("packages/zfb-adapter-cloudflare");
    let (_nm_handle, node_modules) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let scope = node_modules.join("@takazudo");
    fs::create_dir_all(&scope).unwrap();
    std::os::unix::fs::symlink(&adapter, scope.join("zfb-adapter-cloudflare")).unwrap();
    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();
    std::os::unix::fs::symlink(
        "../@takazudo/zfb-adapter-cloudflare/bin/cli.mjs",
        bin_dir.join("zfb-adapter-cloudflare"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&node_modules, root.join("node_modules")).unwrap();

    let scratch_arg = scratch.to_str().unwrap();
    run_zfb_ok(&root, &esbuild, &["build", "--scratch-dir", scratch_arg]);

    let scratch_names = dir_entry_names(&scratch);
    for name in [LOCK_FILE, "bundle.mjs", "bundle-runtime.mjs"] {
        assert!(
            scratch_names.iter().any(|n| n == name),
            "out-of-project scratch dir must hold {name}; found {scratch_names:?}"
        );
    }
    assert!(
        !root.join(".zfb-build/bundle.mjs").exists(),
        "the project's default .zfb-build must stay untouched"
    );
    let scratch_wasm: Vec<_> = scratch_names
        .iter()
        .filter(|n| n.starts_with("ssr-only-") && n.ends_with(".wasm"))
        .collect();
    assert_eq!(
        scratch_wasm.len(),
        1,
        "runtime wasm asset must be emitted beside the scratch bundle: {scratch_names:?}"
    );
    let wasm_name = scratch_wasm[0].clone();

    let dist = root.join("dist");
    assert_eq!(
        read_bytes(&dist.join(&wasm_name)),
        SSR_WASM,
        "the adapter must copy the runtime wasm into dist/ from the out-of-project scratch dir"
    );
    let inner = read_text(&dist.join("_zfb_inner.mjs"));
    assert!(
        inner.contains(&wasm_name),
        "_zfb_inner.mjs imports {wasm_name}"
    );
    assert!(
        dist.join("_worker.js").is_file(),
        "adapter must emit _worker.js"
    );

    let hook = temp.path().join("wasm-hook.mjs");
    let driver = temp.path().join("drive-worker.mjs");
    fs::write(&hook, WASM_LOADER_HOOK).unwrap();
    fs::write(&driver, WORKER_DRIVER).unwrap();
    let output = Command::new("node")
        .arg(&driver)
        .arg(&hook)
        .arg(dist.join("_worker.js"))
        .output()
        .expect("spawn node");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "emitted worker must import and answer in Node\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("\"status\":200"),
        "worker must answer 200: {stdout}"
    );
    assert!(
        stdout.contains(&format!("SSR_WASM_VALUE:{SSR_WASM_VALUE}")),
        "the runtime page must execute the wasm module: {stdout}"
    );
}
