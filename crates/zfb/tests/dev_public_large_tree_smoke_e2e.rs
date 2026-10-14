//! Startup-independence smoke for a large `public/` tree (issue #1167 — C:
//! confirm gate, acceptance item 1).
//!
//! ## Why this test exists
//!
//! After the A+B fixes (`public/` excluded from watch roots in wave A,
//! listener binds before the deferred walk in wave B), cold-start
//! reachability must not scale with the size of the `public/` directory.
//! B (#1166) owns the *authoritative* guard (inject a slow deferred-step;
//! assert the port answers before it completes). This test provides the
//! complementary *realistic full-stack* confirmation: a real `zfb dev`
//! process booting over a real large `public/` tree (1 000 + files) must
//! still serve within the existing `BOOT_DEADLINE` generous bound.
//!
//! ## What it does NOT do
//!
//! It does NOT assert a wall-clock ratio (large vs small fixture) — that
//! would be a flaky wall-clock race. The assertion is simply
//! "boots and serves within `BOOT_DEADLINE`" on a 1 000+ file tree, which
//! would have been painful (or impossible) before the A+B fixes.
//!
//! ## Relation to B's test
//!
//! B's `dev_bind_before_walk_e2e::dev_binds_and_serves_before_slow_deferred_step`
//! is the deterministic guard. This test is additive: it exercises the
//! real public-tree scan path (not an injected slow step), covering any
//! interaction between file-count and the renamed watch roots from fix A.
//!
//! ## `public/assets/` through the `/assets` mount (issue #4233)
//!
//! The same no-`dist/` boot also probes the reserved `/assets` mount: a
//! `public/assets/example.png` staged before boot must answer GET + HEAD
//! with its bytes and MIME, a file added after boot must serve without a
//! restart, and a `public/assets/styles.css` (a framework-reserved name)
//! must draw a boot warning and never be served. Before #4233 every
//! `/assets/*` URL in dev 404'd until a `zfb build` copied `public/` into
//! `dist/assets/`.
//!
//! ## Spawn / teardown discipline
//!
//! Identical to `dev_serve_e2e.rs`: real process group, stdout/stderr
//! to files (never pipes — pipe buffers deadlock on long-lived processes).
//! `DevServerGuard` group-kills on Drop for all exit paths.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Generous wall-clock boot deadline reused from `dev_serve_e2e.rs`. Boot
/// involves V8 + esbuild bundling and an initial render — generous cap so
/// slow CI machines and cold V8 compiles do not produce flaky failures.
const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Number of files to stage in `public/`. The pre-fix code walked this
/// directory synchronously before binding; 1 000+ files made the port
/// unavailable for tens of seconds on a warm build.
const LARGE_PUBLIC_FILE_COUNT: usize = 1_100;

const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dev-loop-basic")
}

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

/// Extract the port from the dev ready banner (dev_serve_e2e.rs pattern).
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

/// Stage `count` small files under `<root>/public/static/` so the public
/// tree is large enough to have been painful pre-fix. Each file is a
/// one-line SVG fragment named `asset-N.svg`. The files are created at test
/// runtime so they are never committed.
///
/// The large tree lives under `public/static/` so it exercises the page
/// waterfall's `public/` leg; `public/assets/` is served by the reserved
/// `/assets` mount instead (issue #4233) and is probed separately by
/// [`stage_public_assets`].
fn stage_large_public_tree(root: &Path, count: usize) {
    let public_static = root.join("public").join("static");
    fs::create_dir_all(&public_static).expect("create public/static dir");
    for i in 0..count {
        let path = public_static.join(format!("asset-{i}.svg"));
        fs::write(&path, format!("<svg id=\"{i}\"/>")).expect("write public static file");
    }
}

/// Bytes of the `public/assets/example.png` staged before boot.
const PUBLIC_ASSET_PNG: &[u8] = b"\x89PNG\r\n\x1a\nzfb-4233-public-asset";

/// Marker inside the framework-reserved `public/assets/styles.css`, which
/// must never be served.
const RESERVED_PUBLIC_CSS_MARKER: &str = "zfb-4233-reserved-public-css";

/// Stage the issue #4233 probes under `public/assets/` before boot.
fn stage_public_assets(root: &Path) {
    let public_assets = root.join("public").join("assets");
    fs::create_dir_all(&public_assets).expect("create public/assets dir");
    fs::write(public_assets.join("example.png"), PUBLIC_ASSET_PNG)
        .expect("write public/assets/example.png");
    fs::write(
        public_assets.join("styles.css"),
        format!("/* {RESERVED_PUBLIC_CSS_MARKER} */"),
    )
    .expect("write public/assets/styles.css");
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

fn fixture_root(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path()
        .canonicalize()
        .expect("canonicalize fixture root")
}

/// Spawn `zfb dev --port 0` over a copy of the fixture that already has
/// the large public tree staged, in its own process group.
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path) -> DevSession {
    let root = fixture_root(tmp);
    copy_dir(&fixture_dir(), &root).expect("copy fixture into tempdir");
    // Stage the large public tree AFTER copying the fixture so we don't
    // pollute the source fixture directory.
    stage_large_public_tree(&root, LARGE_PUBLIC_FILE_COUNT);
    stage_public_assets(&root);
    assert!(
        !root.join("dist").exists(),
        "the fixture must boot with no prebuilt dist/ (issue #4233 repro)"
    );

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
        .env_remove("ZFB_DEV_DEFER_BUNDLE");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

// ---------------------------------------------------------------------------
// The startup-independence smoke test.
// ---------------------------------------------------------------------------

/// Boot `zfb dev` over a fixture with 1 100 public-directory files and
/// assert that it serves within the same `BOOT_DEADLINE` used by the
/// dev-loop e2e harness — confirming startup is not O(public files).
///
/// This is the realistic full-stack complement to B's deterministic
/// injected-slow guard (`dev_bind_before_walk_e2e.rs`) — it exercises the
/// real watch-root exclusion path from fix A and the real deferred-boot
/// path from fix B, together.
#[tokio::test(flavor = "multi_thread")]
async fn dev_boots_and_serves_under_large_public_tree() {
    // Only one test in this binary boots a dev server, so no in-binary
    // SERIAL mutex is needed — but sibling e2e binaries (dev_serve_e2e,
    // dev_bind_before_walk_e2e, etc.) also boot V8/esbuild, so the
    // cross-binary lock still applies (issue #1339). See
    // zfb-test-utils/src/cross_binary_lock.rs.
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[dev_public_large_tree_smoke_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create tempdir for large-public-tree fixture");
    let mut session = spawn_dev(&tmp, &esbuild);
    let pgid = session.guard.pgid;

    eprintln!(
        "[large_public_tree_smoke] staged {LARGE_PUBLIC_FILE_COUNT} files under public/static; \
         booting `zfb dev` …"
    );

    // Phase A: port discovery from the ready banner.
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
                    "[large_public_tree_smoke] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild unavailable); skipping.\n{}",
                    session.logs(),
                );
                return;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the ready \
                 banner.\n{}",
                session.logs(),
            );
        }
        if let Some(p) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                p,
                0,
                "ready banner printed port 0 — the bind-to-0 actual-port fix regressed.\n{}",
                session.logs(),
            );
            break p;
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "[large_public_tree_smoke] `zfb dev` did not print a parseable ready banner within \
             {}s with {LARGE_PUBLIC_FILE_COUNT} public/ files.\n\
             This confirms the pre-fix O(public-files) startup regression: the watched-tree \
             walk over public/ (which fix A excluded) was blocking the bind.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let banner_elapsed = boot_start.elapsed();
    eprintln!(
        "[large_public_tree_smoke] banner appeared in {}ms with {LARGE_PUBLIC_FILE_COUNT} \
         public/ files.",
        banner_elapsed.as_millis()
    );

    // Phase B: HTTP readiness — poll GET / until 200 within the same deadline.
    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("build reqwest client");

    loop {
        match client.get(format!("{base}/")).send().await {
            Ok(resp) if resp.status().as_u16() == 200 => break,
            _ => {}
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "[large_public_tree_smoke] GET / never answered 200 within {}s after the ready \
             banner (total elapsed: {}ms). Process group {pgid} will be killed.\n{}",
            BOOT_DEADLINE.as_secs(),
            boot_start.elapsed().as_millis(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let total_elapsed = boot_start.elapsed();
    eprintln!(
        "[large_public_tree_smoke] first 200 on GET / received in {}ms with \
         {LARGE_PUBLIC_FILE_COUNT} public/ files.",
        total_elapsed.as_millis()
    );

    // Phase C: confirm that a public/ file is actually reachable — the
    // large tree is not just staged for noise, it is served. It lives under
    // public/static/ so this probes the page waterfall's public/ leg; the
    // public/assets/ probes in Phase D go through the /assets mount.
    let asset_url = format!("{base}/static/asset-0.svg");
    let resp = client
        .get(&asset_url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {asset_url} failed: {e}\n{}", session.logs()));
    assert_eq!(
        resp.status().as_u16(),
        200,
        "[large_public_tree_smoke] a staged public/ file at /static/asset-0.svg must be \
         accessible once the dev server is up. This confirms the public/ waterfall is wired \
         correctly.\n{}",
        session.logs(),
    );

    // Phase D (issue #4233): public/assets/ is served through the
    // contained /assets mount with no dist/ and no build.
    let png_url = format!("{base}/assets/example.png");
    let resp = client
        .get(&png_url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {png_url} failed: {e}\n{}", session.logs()));
    assert_eq!(
        resp.status().as_u16(),
        200,
        "[#4233] public/assets/example.png must answer /assets/example.png before any \
         build.\n{}",
        session.logs(),
    );
    assert_eq!(
        content_type(&resp),
        "image/png",
        "[#4233] GET content-type\n{}",
        session.logs()
    );
    let body = resp.bytes().await.expect("read /assets/example.png body");
    assert_eq!(
        &body[..],
        PUBLIC_ASSET_PNG,
        "[#4233] GET must serve the file's bytes"
    );

    let resp = client
        .head(&png_url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("HEAD {png_url} failed: {e}\n{}", session.logs()));
    assert_eq!(
        resp.status().as_u16(),
        200,
        "[#4233] HEAD /assets/example.png\n{}",
        session.logs()
    );
    assert_eq!(
        content_type(&resp),
        "image/png",
        "[#4233] HEAD content-type"
    );
    let body = resp.bytes().await.expect("read HEAD body");
    assert!(body.is_empty(), "[#4233] HEAD must carry no body");

    // A file added after boot serves without a restart (read per request).
    let late = b"zfb-4233-added-after-boot";
    fs::write(fixture_root(&tmp).join("public/assets/late.svg"), late)
        .expect("write public/assets/late.svg after boot");
    let late_url = format!("{base}/assets/late.svg");
    let resp = client
        .get(&late_url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {late_url} failed: {e}\n{}", session.logs()));
    assert_eq!(
        resp.status().as_u16(),
        200,
        "[#4233] a public/assets/ file added after boot must serve immediately.\n{}",
        session.logs(),
    );
    assert_eq!(content_type(&resp), "image/svg+xml");
    assert_eq!(&resp.bytes().await.expect("read late body")[..], late);

    // The reserved name is never served from public/assets/, and boot
    // warned about it exactly once.
    let css_url = format!("{base}/assets/styles.css");
    let resp = client
        .get(&css_url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {css_url} failed: {e}\n{}", session.logs()));
    let css = resp.text().await.unwrap_or_default();
    assert!(
        !css.contains(RESERVED_PUBLIC_CSS_MARKER),
        "[#4233] public/assets/styles.css must not stand in for zfb's stylesheet.\n{}",
        session.logs(),
    );
    let stderr = read_log(&session.stderr_path);
    assert_eq!(
        stderr
            .matches("public/assets/styles.css is not served at /assets/styles.css")
            .count(),
        1,
        "[#4233] expected exactly one reserved-name boot warning.\n{}",
        session.logs(),
    );

    // The DevServerGuard Drop SIGKILL's the process group here.
    drop(session);
}

fn content_type(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}
