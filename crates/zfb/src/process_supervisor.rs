//! Supervision for commands that may leave descendants behind.

use anyhow::{Context, Result};
use std::time::Duration;
use tokio::process::Command;

#[derive(Clone, Copy, Debug)]
pub struct SuperviseOptions {
    pub grace: Duration,
}

impl Default for SuperviseOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitKind {
    Exited(i32),
    Signaled(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisedOutcome {
    pub exit: ExitKind,
    pub forwarded: Option<i32>,
}

#[cfg(unix)]
mod unix {
    use super::*;
    use anyhow::{bail, ensure};
    use std::io;
    use std::os::unix::process::ExitStatusExt as _;
    use tokio::sync::mpsc;
    use tokio::time::{self, Instant};

    pub(super) fn should_handoff_tty(is_tty: bool, is_foreground: bool) -> bool {
        is_tty && is_foreground
    }

    struct ForegroundGuard {
        old_sigttou: libc::sigaction,
    }

    impl ForegroundGuard {
        fn acquire() -> Result<Option<Self>> {
            // SAFETY: isatty and getpgrp inspect process state; tcgetpgrp reads fd 0.
            let eligible = unsafe {
                should_handoff_tty(libc::isatty(0) == 1, libc::tcgetpgrp(0) == libc::getpgrp())
            };
            if !eligible {
                return Ok(None);
            }
            // SAFETY: sigaction is initialized before use and installed for SIGTTOU only.
            unsafe {
                let mut old = std::mem::zeroed();
                let mut ignore: libc::sigaction = std::mem::zeroed();
                ignore.sa_sigaction = libc::SIG_IGN;
                libc::sigemptyset(&mut ignore.sa_mask);
                if libc::sigaction(libc::SIGTTOU, &ignore, &mut old) == -1 {
                    return Err(io::Error::last_os_error()).context("ignore SIGTTOU");
                }
                Ok(Some(Self { old_sigttou: old }))
            }
        }

        fn handoff(&self, pgid: libc::pid_t) -> Result<()> {
            // SAFETY: fd 0 was verified as the foreground controlling tty; pgid is the child group.
            if unsafe { libc::tcsetpgrp(0, pgid) } == -1 {
                return Err(io::Error::last_os_error()).context("give terminal to child group");
            }
            Ok(())
        }
    }

    impl Drop for ForegroundGuard {
        fn drop(&mut self) {
            // SAFETY: restore this process group before restoring SIGTTOU disposition.
            unsafe {
                libc::tcsetpgrp(0, libc::getpgrp());
                libc::sigaction(libc::SIGTTOU, &self.old_sigttou, std::ptr::null_mut());
            }
        }
    }

    fn check_pgid(pgid: libc::pid_t) -> Result<()> {
        // SAFETY: getpgrp only reads the caller's process group.
        let inherited = unsafe { libc::getpgrp() };
        ensure!(
            pgid > 1 && pgid != inherited,
            "unsafe child process group {pgid}"
        );
        Ok(())
    }

    struct ProcessGroupGuard {
        pgid: libc::pid_t,
        active: bool,
    }

    impl Drop for ProcessGroupGuard {
        fn drop(&mut self) {
            if self.active {
                // SAFETY: this pgid was validated against our inherited group at spawn.
                unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            }
        }
    }

    fn group_signal(pgid: libc::pid_t, sig: i32) -> Result<bool> {
        check_pgid(pgid)?;
        // SAFETY: negative, validated pgid targets only the child's process group.
        let result = unsafe { libc::kill(-pgid, sig) };
        if result == 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else if cfg!(target_os = "macos") && sig == 0 && err.raw_os_error() == Some(libc::EPERM) {
            // Darwin reports EPERM when a reaped group has only unsignalable zombies.
            Ok(false)
        } else {
            Err(err).with_context(|| format!("signal child group {pgid} with {sig}"))
        }
    }

    pub(super) async fn supervise_with(
        mut cmd: Command,
        opts: SuperviseOptions,
        mut signal_rx: mpsc::UnboundedReceiver<i32>,
    ) -> Result<SupervisedOutcome> {
        let tty = ForegroundGuard::acquire()?;
        cmd.process_group(0);
        cmd.kill_on_drop(true);
        if tty.is_some() {
            // SAFETY: pre_exec runs in the forked child. Only async-signal-safe libc calls
            // are used: getpid, tcsetpgrp, sigemptyset, and sigaction. No allocation.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::tcsetpgrp(0, libc::getpid()) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    let mut default: libc::sigaction = std::mem::zeroed();
                    default.sa_sigaction = libc::SIG_DFL;
                    libc::sigemptyset(&mut default.sa_mask);
                    if libc::sigaction(libc::SIGTTOU, &default, std::ptr::null_mut()) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().context("spawn supervised process")?;
        let pgid = match child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) {
            Some(pgid) => pgid,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                bail!("supervised process has no valid pid");
            }
        };
        if let Err(err) = check_pgid(pgid) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(err);
        }
        let mut group_guard = ProcessGroupGuard { pgid, active: true };
        if let Some(tty) = &tty {
            // The child also sets this in pre_exec; the parent's set closes the handoff race.
            if let Err(err) = tty.handoff(pgid) {
                let _ = group_signal(pgid, libc::SIGKILL);
                let _ = child.wait().await;
                return Err(err);
            }
        }

        let mut forwarded = None;
        let mut deadline: Option<Instant> = None;
        let mut escalated = false;
        let mut signals_open = true;
        let status = loop {
            tokio::select! {
                biased;
                signal = signal_rx.recv(), if !escalated && signals_open => {
                    if let Some(sig) = signal {
                        if !matches!(sig, libc::SIGTERM | libc::SIGINT | libc::SIGHUP) {
                            continue;
                        }
                        if forwarded.is_none() {
                            forwarded = Some(sig);
                            group_signal(pgid, sig)?;
                            deadline = Some(Instant::now() + opts.grace);
                        } else {
                            group_signal(pgid, libc::SIGKILL)?;
                            escalated = true;
                        }
                    } else {
                        signals_open = false;
                    }
                }
                status = child.wait() => break status.context("wait for supervised process")?,
                _ = async { time::sleep_until(deadline.expect("armed deadline")).await }, if deadline.is_some() && !escalated => {
                    group_signal(pgid, libc::SIGKILL)?;
                    escalated = true;
                }
            }
        };
        // Restore the terminal as soon as the direct child has been reaped.
        drop(tty);
        sweep_group(pgid, opts.grace).await?;
        group_guard.active = false;
        let exit = if let Some(code) = status.code() {
            ExitKind::Exited(code)
        } else if let Some(sig) = status.signal() {
            ExitKind::Signaled(sig)
        } else {
            bail!("supervised process has neither exit code nor signal")
        };
        Ok(SupervisedOutcome { exit, forwarded })
    }

    async fn sweep_group(pgid: libc::pid_t, grace: Duration) -> Result<()> {
        // A reaped leader's pgid could theoretically be reused before this probe.
        if !group_signal(pgid, 0)? {
            return Ok(());
        }
        group_signal(pgid, libc::SIGTERM)?;
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if !group_signal(pgid, 0)? {
                return Ok(());
            }
            time::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
        group_signal(pgid, libc::SIGKILL)?;
        Ok(())
    }

    struct SignalPump(tokio::task::JoinHandle<()>);

    impl Drop for SignalPump {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    pub(super) async fn supervise(
        cmd: Command,
        opts: SuperviseOptions,
    ) -> Result<SupervisedOutcome> {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).context("register SIGTERM")?;
        let mut int = signal(SignalKind::interrupt()).context("register SIGINT")?;
        let mut hup = signal(SignalKind::hangup()).context("register SIGHUP")?;
        let (tx, rx) = mpsc::unbounded_channel();
        let _pump = SignalPump(tokio::spawn(async move {
            loop {
                let sig = tokio::select! {
                    _ = term.recv() => libc::SIGTERM,
                    _ = int.recv() => libc::SIGINT,
                    _ = hup.recv() => libc::SIGHUP,
                };
                if tx.send(sig).is_err() {
                    break;
                }
            }
        }));
        supervise_with(cmd, opts, rx).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;
        use std::net::TcpListener;
        use std::path::Path;
        use std::process::{Child, Stdio};
        use tokio::sync::mpsc;

        #[test]
        fn foreground_decision() {
            for tty in [false, true] {
                for fg in [false, true] {
                    assert_eq!(should_handoff_tty(tty, fg), tty && fg);
                }
            }
        }

        #[test]
        fn grandchild_helper() {
            let Ok(info) = std::env::var("ZFB_SUPERVISOR_HELPER_INFO") else {
                return;
            };
            let socket = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            fs::write(info, format!("{} {port}", std::process::id())).unwrap();
            loop {
                std::thread::park();
            }
        }

        async fn until(mut predicate: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if predicate() {
                    return;
                }
                assert!(Instant::now() < deadline, "condition did not become true");
                time::sleep(Duration::from_millis(10)).await;
            }
        }

        fn process_gone(pid: i32) -> bool {
            // SAFETY: kill(pid, 0) only probes existence, and the pid comes from our helper.
            let result = unsafe { libc::kill(pid, 0) };
            result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }

        struct Unrelated(Child);
        impl Unrelated {
            fn new() -> Self {
                use std::os::unix::process::CommandExt;
                let mut cmd = std::process::Command::new("sh");
                cmd.args(["-c", "exec sleep 60"])
                    .process_group(0)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                Self(cmd.spawn().unwrap())
            }
            fn alive(&mut self) -> bool {
                self.0.try_wait().unwrap().is_none()
            }
        }
        impl Drop for Unrelated {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        fn helper_command(info: &Path, mode: &str) -> Command {
            let exe = std::env::current_exe().unwrap();
            let mut cmd = Command::new("sh");
            let script = match mode {
                "wait" => {
                    r#""$1" --exact process_supervisor::unix::tests::grandchild_helper & wait"#
                }
                "ignore" => {
                    r#"trap '' TERM; "$1" --exact process_supervisor::unix::tests::grandchild_helper & wait"#
                }
                "leave" => {
                    r#""$1" --exact process_supervisor::unix::tests::grandchild_helper & while [ ! -s "$ZFB_SUPERVISOR_HELPER_INFO" ]; do sleep .01; done; exit 0"#
                }
                _ => unreachable!(),
            };
            cmd.args(["-c", script, "sh"])
                .arg(exe)
                .env("ZFB_SUPERVISOR_HELPER_INFO", info)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            cmd
        }

        async fn tree_case(
            sig: Option<i32>,
            mode: &str,
            second: bool,
            queued: bool,
        ) -> SupervisedOutcome {
            let dir = tempfile::tempdir().unwrap();
            let info = dir.path().join("helper.info");
            let mut unrelated = Unrelated::new();
            let (tx, rx) = mpsc::unbounded_channel();
            if queued {
                tx.send(sig.unwrap()).unwrap();
            }
            let grace = if second {
                Duration::from_secs(30)
            } else {
                Duration::from_millis(200)
            };
            let task = tokio::spawn(supervise_with(
                helper_command(&info, mode),
                SuperviseOptions { grace },
                rx,
            ));
            until(|| info.exists()).await;
            let data = fs::read_to_string(&info).unwrap();
            let mut fields = data.split_whitespace();
            let pid: i32 = fields.next().unwrap().parse().unwrap();
            let port: u16 = fields.next().unwrap().parse().unwrap();
            if let Some(sig) = sig {
                if !queued {
                    tx.send(sig).unwrap();
                }
                if second {
                    tx.send(sig).unwrap();
                }
            }
            let outcome = time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            until(|| process_gone(pid)).await;
            assert!(
                TcpListener::bind(("127.0.0.1", port)).is_ok(),
                "port still held"
            );
            assert!(unrelated.alive(), "unrelated process group was signaled");
            outcome
        }

        #[tokio::test]
        async fn forwarded_term() {
            let result = tree_case(Some(libc::SIGTERM), "wait", false, false).await;
            assert_eq!(result.forwarded, Some(libc::SIGTERM));
        }
        #[tokio::test]
        async fn forwarded_int() {
            let result = tree_case(Some(libc::SIGINT), "wait", false, false).await;
            assert_eq!(result.forwarded, Some(libc::SIGINT));
        }
        #[tokio::test]
        async fn forwarded_hup() {
            let result = tree_case(Some(libc::SIGHUP), "wait", false, false).await;
            assert_eq!(result.forwarded, Some(libc::SIGHUP));
        }
        #[tokio::test]
        async fn escalates_after_grace() {
            let result = tree_case(Some(libc::SIGTERM), "ignore", false, false).await;
            assert_eq!(
                result,
                SupervisedOutcome {
                    exit: ExitKind::Signaled(libc::SIGKILL),
                    forwarded: Some(libc::SIGTERM)
                }
            );
        }
        #[tokio::test]
        async fn repeated_signal_escalates_immediately() {
            let start = Instant::now();
            let result = tree_case(Some(libc::SIGTERM), "ignore", true, false).await;
            assert_eq!(result.exit, ExitKind::Signaled(libc::SIGKILL));
            assert!(start.elapsed() < Duration::from_secs(5));
        }
        #[tokio::test]
        async fn queued_signal_is_forwarded() {
            let mut unrelated = Unrelated::new();
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "exec sleep 60"]);
            let (tx, rx) = mpsc::unbounded_channel();
            tx.send(libc::SIGTERM).unwrap();
            let result = time::timeout(
                Duration::from_secs(5),
                supervise_with(
                    cmd,
                    SuperviseOptions {
                        grace: Duration::from_millis(200),
                    },
                    rx,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result.forwarded, Some(libc::SIGTERM));
            assert!(unrelated.alive());
        }
        #[tokio::test]
        async fn sweeps_leftover_grandchild() {
            let result = tree_case(None, "leave", false, false).await;
            assert_eq!(
                result,
                SupervisedOutcome {
                    exit: ExitKind::Exited(0),
                    forwarded: None
                }
            );
        }
        #[tokio::test]
        async fn exits_without_signal() {
            let mut unrelated = Unrelated::new();
            for code in [0, 3] {
                let mut cmd = Command::new("sh");
                cmd.args(["-c", &format!("exit {code}")]);
                let (_tx, rx) = mpsc::unbounded_channel();
                let result = supervise_with(cmd, SuperviseOptions::default(), rx)
                    .await
                    .unwrap();
                assert_eq!(
                    result,
                    SupervisedOutcome {
                        exit: ExitKind::Exited(code),
                        forwarded: None
                    }
                );
                assert!(unrelated.alive());
            }
        }
    }
}

#[cfg(unix)]
pub async fn supervise(cmd: Command, opts: SuperviseOptions) -> Result<SupervisedOutcome> {
    unix::supervise(cmd, opts).await
}

#[cfg(not(unix))]
pub async fn supervise(mut cmd: Command, _opts: SuperviseOptions) -> Result<SupervisedOutcome> {
    // Windows process-tree cleanup needs a separate Job Object follow-up.
    let status = cmd
        .spawn()
        .context("spawn supervised process")?
        .wait()
        .await
        .context("wait for supervised process")?;
    Ok(SupervisedOutcome {
        exit: ExitKind::Exited(status.code().unwrap_or(1)),
        forwarded: None,
    })
}
