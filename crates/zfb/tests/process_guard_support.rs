//! Tests for the shared `support/process_guard.rs` kill-tree guard (#4483,
//! #4484, #4481). They spawn only `sh` / `powershell.exe`, never `zfb`, so
//! this binary is not in nextest's e2e-heavy groups.

#[path = "support/process_guard.rs"]
mod process_guard;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use process_guard::{base64, windows_quote_arg, ProcessTreeGuard, ProcessTreeSpawner};

fn wait_for_file_text(path: &Path, guard: &mut ProcessTreeGuard, what: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.trim().is_empty() {
                return text.trim().to_owned();
            }
        }
        if let Some(status) = guard.try_wait().expect("try_wait") {
            panic!("child exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn base64_matches_rfc4648_vectors() {
    for (input, expected) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(input.as_bytes()), expected, "base64({input:?})");
    }
}

#[test]
fn windows_quote_arg_follows_msvc_argv_rules() {
    for (input, expected) in [
        ("plain", "plain"),
        ("", "\"\""),
        ("has space", "\"has space\""),
        (r"C:\dir\file", r"C:\dir\file"),
        (r"C:\with space\", r#""C:\with space\\""#),
        (r#"say "hi""#, r#""say \"hi\"""#),
        (r#"a\"b"#, r#""a\\\"b""#),
        ("__ORIGIN__=\"A\"", r#""__ORIGIN__=\"A\"""#),
    ] {
        assert_eq!(windows_quote_arg(input), expected, "quote({input:?})");
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    let output = Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("run tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

fn assert_dies(pid: u32, what: &str) {
    // A killed Unix grandchild is reparented and reaped asynchronously by
    // init; until then signal 0 still succeeds on the zombie.
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "{what} {pid} survived the guard drop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn grandchild_command(pid_file: &Path) -> Command {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "sleep 60 & echo $! > \"$1\"; sleep 60", "sh"])
        .arg(pid_file);
    cmd
}

#[cfg(windows)]
fn grandchild_command(pid_file: &Path) -> Command {
    let script = format!(
        "$p = Start-Process -PassThru -NoNewWindow -FilePath ping.exe \
         -ArgumentList '-n','120','127.0.0.1'; \
         Set-Content -Encoding ascii -LiteralPath '{}' -Value $p.Id; Start-Sleep -Seconds 120",
        pid_file.display()
    );
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    cmd
}

#[test]
fn drop_kills_child_and_grandchild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("grandchild.pid");
    let mut guard =
        ProcessTreeGuard::spawn(&mut grandchild_command(&pid_file)).expect("spawn guarded child");
    let child_pid = guard.id();
    #[cfg(unix)]
    assert_eq!(guard.process_group_id() as u32, child_pid);
    let grandchild_pid: u32 = wait_for_file_text(&pid_file, &mut guard, "the grandchild started")
        .parse()
        .expect("grandchild pid");
    assert!(pid_alive(grandchild_pid), "grandchild should be running");

    drop(guard);

    assert_dies(grandchild_pid, "grandchild");
    assert!(!pid_alive(child_pid), "direct child should be gone");
}

/// A child that writes its own pid to `ready` once its interrupt handler is
/// installed, and on interrupt writes `ok` to `marker` and exits 0 — or,
/// with `ignore`, swallows the interrupt and keeps running.
#[cfg(unix)]
fn interrupt_child(marker: &Path, ready: &Path, ignore: bool) -> Command {
    let trap = if ignore {
        "trap '' INT"
    } else {
        "trap 'echo ok > \"$1\"; exit 0' INT"
    };
    let mut cmd = Command::new("sh");
    cmd.args([
        "-c",
        &format!("{trap}; echo $$ > \"$2\"; while :; do sleep 0.1; done"),
        "sh",
    ])
    .arg(marker)
    .arg(ready);
    cmd
}

#[cfg(windows)]
fn interrupt_child(marker: &Path, ready: &Path, ignore: bool) -> Command {
    // A .NET Console.CancelKeyPress handler stands in for zfb dev's
    // `tokio::signal::ctrl_c()`: both are console control handlers.
    let script = r#"
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
public static class ZfbCtrlCChild {
    public static void Run(string marker, string ready, bool ignore) {
        Console.CancelKeyPress += delegate(object sender, ConsoleCancelEventArgs e) {
            e.Cancel = true;
            if (ignore) { return; }
            File.WriteAllText(marker, "ok");
            Environment.Exit(0);
        };
        File.WriteAllText(ready, Process.GetCurrentProcess().Id.ToString());
        while (true) { Thread.Sleep(100); }
    }
}
'@
[ZfbCtrlCChild]::Run($env:ZFB_GRACEFUL_MARKER, $env:ZFB_GRACEFUL_READY, $env:ZFB_GRACEFUL_IGNORE -eq '1')
"#;
    let mut cmd = Command::new("powershell.exe");
    // -EncodedCommand sidesteps Windows PowerShell's handling of the double
    // quotes in the C# source.
    cmd.args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(process_guard::encode_powershell_command(script))
        .env("ZFB_GRACEFUL_MARKER", marker)
        .env("ZFB_GRACEFUL_READY", ready)
        .env("ZFB_GRACEFUL_IGNORE", if ignore { "1" } else { "0" });
    cmd
}

#[test]
fn interrupt_stops_child_gracefully() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("graceful");
    let ready = dir.path().join("ready");
    let mut guard = ProcessTreeSpawner::new()
        .interruptible()
        .spawn(&mut interrupt_child(&marker, &ready, false))
        .expect("spawn interruptible child");
    let ready_pid = wait_for_file_text(&ready, &mut guard, "the interrupt handler was installed");
    assert_eq!(
        ready_pid,
        guard.id().to_string(),
        "id() must be the guarded command's pid"
    );

    let status = guard
        .interrupt_and_wait(Duration::from_secs(60))
        .expect("interrupt")
        .expect("child should stop gracefully on interrupt");

    assert!(status.success(), "graceful exit status: {status}");
    let text = std::fs::read_to_string(&marker).expect("graceful marker");
    assert_eq!(text.trim(), "ok");
}

#[test]
fn interrupt_timeout_is_not_reported_as_graceful() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("graceful");
    let ready = dir.path().join("ready");
    let mut guard = ProcessTreeSpawner::new()
        .interruptible()
        .spawn(&mut interrupt_child(&marker, &ready, true))
        .expect("spawn interruptible child");
    let child_pid: u32 = wait_for_file_text(&ready, &mut guard, "the interrupt was ignored")
        .parse()
        .expect("child pid");

    let outcome = guard
        .interrupt_and_wait(Duration::from_secs(2))
        .expect("interrupt");

    assert!(outcome.is_none(), "an ignored interrupt must time out");
    let status = guard.kill_tree().expect("kill_tree fallback");
    assert!(
        !status.success(),
        "hard kill must not look like a clean exit"
    );
    assert!(!marker.exists(), "the ignoring child never ran its handler");
    assert_dies(child_pid, "guarded command");
}

#[cfg(windows)]
#[test]
fn interrupt_requires_interruptible_spawn() {
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Start-Sleep -Seconds 60",
    ]);
    let mut guard = ProcessTreeGuard::spawn(&mut cmd).expect("spawn guarded powershell");
    let err = guard
        .interrupt()
        .expect_err("a plain spawn has no console helper");
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}
