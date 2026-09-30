//! Process-level adapter preview supervision (#3428).
//!
//! The harness owns each zfb process group and records the child group for
//! emergency cleanup in Drop if an assertion fails.
//! The stop under test targets the recorded zfb PID alone. The supervisor owns
//! the separate pnpm child group and its listener grandchild. A pnpm wrapper
//! that discards signals instead of waiting is outside this scenario.
#![cfg(unix)]

use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zfb::commands::preview::EXPECTED_WRANGLER_VERSION;

const DEADLINE: Duration = Duration::from_secs(30);
const OVERALL_DEADLINE: Duration = Duration::from_secs(120);

thread_local! {
    static STARTED: Instant = Instant::now();
}

#[test]
fn preview_grandchild_helper() {
    if std::env::var_os("PREVIEW_GRANDCHILD").is_none() {
        return;
    }
    let port: u16 = std::env::var("PREVIEW_PORT").unwrap().parse().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    fs::write(
        std::env::var("PREVIEW_PID_FILE").unwrap(),
        std::process::id().to_string(),
    )
    .unwrap();
    loop {
        let _ = listener.accept();
    }
}

fn until(mut ready: impl FnMut() -> bool, description: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if ready() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        STARTED.with(|started| {
            assert!(
                started.elapsed() < OVERALL_DEADLINE,
                "preview case exceeded overall watchdog"
            );
        });
        thread::sleep(Duration::from_millis(10));
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn connected(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(50),
    )
    .is_ok()
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: read-only probe of a pid written by our own helper process.
    unsafe { libc::kill(pid, 0) == 0 }
}

struct Preview {
    _fixture: TempDir,
    child: Child,
    port: u16,
    pid_file: PathBuf,
    pnpm_pid_file: PathBuf,
    stderr: PathBuf,
}

impl Preview {
    fn start(mode: &str) -> Self {
        let fixture = tempfile::tempdir().unwrap();
        fs::create_dir(fixture.path().join("dist")).unwrap();
        fs::write(
            fixture.path().join("zfb.config.json"),
            r#"{"adapter":"@takazudo/zfb-adapter-cloudflare"}"#,
        )
        .unwrap();
        fs::write(
            fixture.path().join("wrangler.toml"),
            "name = 'preview-test'\nmain = 'dist/_worker.js'\n",
        )
        .unwrap();
        let bin_dir = fixture.path().join("bin");
        fs::create_dir(&bin_dir).unwrap();
        let pnpm = bin_dir.join("pnpm");
        fs::write(
            &pnpm,
            r#"#!/bin/sh
if [ "$1" = exec ] && [ "$2" = wrangler ] && [ "$3" = --version ]; then
  echo "$PREVIEW_EXPECTED_VERSION"
  exit 0
fi
if [ "$1" = exec ] && [ "$2" = wrangler ] && [ "$3" = dev ]; then
  echo $$ > "$PREVIEW_PNPM_PID_FILE"
  case "$PREVIEW_MODE" in
    exit0) exit 0 ;;
    exit7) exit 7 ;;
  esac
  "$PREVIEW_HELPER_EXE" --exact preview_grandchild_helper --nocapture &
  wait $!
  exit $?
fi
exit 86
"#,
        )
        .unwrap();
        fs::set_permissions(&pnpm, fs::Permissions::from_mode(0o755)).unwrap();
        let port = free_port();
        let pid_file = fixture.path().join("grandchild.pid");
        let pnpm_pid_file = fixture.path().join("pnpm.pid");
        let stderr = fixture.path().join("stderr.log");
        let stdout = fixture.path().join("stdout.log");
        let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
        let child = Command::new(env!("CARGO_BIN_EXE_zfb"))
            .args(["preview", "--port", &port.to_string()])
            .current_dir(fixture.path())
            .env("PATH", path)
            .env("PREVIEW_MODE", mode)
            .env("PREVIEW_HELPER_EXE", std::env::current_exe().unwrap())
            .env("PREVIEW_GRANDCHILD", "1")
            .env("PREVIEW_PORT", port.to_string())
            .env("PREVIEW_PID_FILE", &pid_file)
            .env("PREVIEW_PNPM_PID_FILE", &pnpm_pid_file)
            .env("PREVIEW_EXPECTED_VERSION", EXPECTED_WRANGLER_VERSION)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(stdout).unwrap()))
            .stderr(Stdio::from(File::create(&stderr).unwrap()))
            .spawn()
            .unwrap();
        Self {
            _fixture: fixture,
            child,
            port,
            pid_file,
            pnpm_pid_file,
            stderr,
        }
    }

    fn listening() -> Self {
        for attempt in 0..=1 {
            let mut preview = Self::start("listen");
            let deadline = Instant::now() + DEADLINE;
            loop {
                if preview.pid_file.exists() && connected(preview.port) {
                    return preview;
                }
                if preview.child.try_wait().unwrap().is_some() {
                    let stderr = fs::read_to_string(&preview.stderr).unwrap();
                    if attempt == 0 && stderr.contains("Address already in use") {
                        break;
                    }
                    panic!("zfb exited before listener ready: {stderr}");
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for grandchild listener"
                );
                STARTED.with(|started| assert!(started.elapsed() < OVERALL_DEADLINE));
                thread::sleep(Duration::from_millis(10));
            }
        }
        unreachable!("the second port attempt returns or panics")
    }

    fn ready(&mut self) -> i32 {
        until(
            || {
                assert!(
                    self.child.try_wait().unwrap().is_none(),
                    "zfb exited before listener ready: {}",
                    fs::read_to_string(&self.stderr).unwrap()
                );
                self.pid_file.exists() && connected(self.port)
            },
            "grandchild listener",
        );
        fs::read_to_string(&self.pid_file).unwrap().parse().unwrap()
    }

    fn signal(&self, signal: i32) {
        // SAFETY: target is the direct zfb child PID, never its process group.
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
    }

    fn exit(&mut self) -> ExitStatus {
        let mut status = None;
        until(
            || {
                status = self.child.try_wait().unwrap();
                status.is_some()
            },
            "zfb exit",
        );
        status.unwrap()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(&self.pnpm_pid_file) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                // SAFETY: the supervisor created this separate pnpm process group.
                if pid > 1 && pid != unsafe { libc::getpgrp() } {
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                }
            }
        }
        // SAFETY: this group was made exclusively for the zfb process we spawned.
        unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

fn assert_stopped(preview: &mut Preview, signal: i32) {
    let pid = preview.ready();
    preview.signal(signal);
    assert_eq!(
        preview.exit().code(),
        Some(128 + signal),
        "{}",
        fs::read_to_string(&preview.stderr).unwrap()
    );
    until(|| !pid_alive(pid), "grandchild exit");
    until(
        || TcpListener::bind(("127.0.0.1", preview.port)).is_ok(),
        "port release",
    );
    assert!(!fs::read_to_string(&preview.stderr).unwrap().contains("✗"));
}

#[test]
fn signal_stops_tree_and_frees_port() {
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let mut preview = Preview::listening();
        assert_stopped(&mut preview, signal);
    }
}

#[test]
fn child_exit_status_is_preserved() {
    let mut ok = Preview::start("exit0");
    assert_eq!(ok.exit().code(), Some(0));
    let mut failed = Preview::start("exit7");
    assert_eq!(failed.exit().code(), Some(1));
    assert!(fs::read_to_string(&failed.stderr)
        .unwrap()
        .contains("wrangler dev exited with status"));
}

#[test]
fn concurrent_previews_are_isolated() {
    let mut first = Preview::listening();
    let mut second = Preview::listening();
    let _first_pid = first.ready();
    let second_pid = second.ready();
    assert_stopped(&mut first, libc::SIGTERM);
    assert!(pid_alive(second_pid));
    assert!(connected(second.port));
    assert_stopped(&mut second, libc::SIGTERM);
}
