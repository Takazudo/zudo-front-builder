//! Level-4 `zfb dev` smoke for a Wasm-importing SSR route (issue #1511).
//!
//! Bundler and embedded-V8 tests cover their respective seams, but only this
//! process-level test proves the whole dev-server path: esbuild copies the
//! imported Wasm module beside the bundle, the V8 module loader turns it into
//! a `WebAssembly.Module`, and a `prerender=false` route renders that module
//! in an actual HTTP response.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

// `i32.const` uses signed LEB128, so a one-byte positive immediate is <= 63.
const WASM_ANSWER: u8 = 37;
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// Minimal Wasm module exporting `answer(): i32`. Keeping this input in the
// Rust test makes the response marker and exact module contents inseparable.
const WASM: &[u8] = &[
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
    WASM_ANSWER,
    0x0b,
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wasm-ssr-dev-smoke")
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let destination = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
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

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn dump_logs(stdout_path: &Path, stderr_path: &Path) -> String {
    format!(
        "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
        read_log(stdout_path),
        read_log(stderr_path),
    )
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

/// Extract the ephemeral port from the dev ready banner.
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.find(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

/// Copy the source fixture and stage the binary module it imports.
fn spawn_dev(tmp: &tempfile::TempDir, esbuild: &Path) -> DevSession {
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    copy_dir(&fixture_dir(), &root).expect("copy Wasm dev fixture");
    fs::create_dir_all(root.join("wasm")).expect("create fixture Wasm directory");
    fs::write(root.join("wasm/answer.wasm"), WASM).expect("write fixture Wasm module");

    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.process_group(0);

    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// A real `zfb dev` session must execute an imported Wasm module while
/// request-rendering a `prerender=false` route, then serve its value with 200.
#[tokio::test(flavor = "multi_thread")]
async fn dev_serves_wasm_importing_ssr_route() {
    // This binary starts a V8 + esbuild dev session. The advisory flock is
    // acquired before any local state so it serializes against every other
    // flock-adopting dev/build e2e binary (issue #1339).
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[wasm_ssr_dev_smoke_e2e] no esbuild binary available; skipping. \\
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create Wasm dev fixture tempdir");
    let mut session = spawn_dev(&tmp, &esbuild);

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
                    "[wasm_ssr_dev_smoke_e2e] `zfb dev` exited with a known environment \\
                     gate; skipping.\n{}",
                    session.logs(),
                );
                return;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before its ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "ready banner printed port 0 instead of the ephemeral bound port.\n{}",
                session.logs(),
            );
            break port;
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "[wasm_ssr_dev_smoke_e2e] `zfb dev` did not print a parseable ready banner \\
             within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let route_url = format!("http://localhost:{port}/api/og");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");
    let response_start = Instant::now();
    let expected_marker = format!("WASM_DEV_SSR_VALUE:{WASM_ANSWER}");

    loop {
        let observation = match client.get(&route_url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 200 && body.contains(&expected_marker) {
                    break;
                }
                format!("status {status}, body:\n{body}")
            }
            Err(error) => format!("request error: {error}"),
        };
        assert!(
            response_start.elapsed() < RESPONSE_DEADLINE,
            "[wasm_ssr_dev_smoke_e2e] GET {route_url} did not return 200 with \\
             {expected_marker:?} within {}s. Last observation: {observation}\n{}",
            RESPONSE_DEADLINE.as_secs(),
            session.logs(),
        );
        // Condition-keyed polling is needed because a lazy dev session builds
        // this SSR route on its first request; there is no fixed delay.
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
