//! Cross-platform kill-tree guard for long-running child processes in the
//! `zfb` integration tests (typically `zfb dev`), issues #4483/#4484.
//!
//! Shared by several test binaries through
//! `#[path = "support/process_guard.rs"] mod process_guard;`; its own tests
//! live in `tests/process_guard_support.rs`. It deliberately lives here, not
//! in `zfb-test-utils`, and uses no crate beyond the `libc` dev-dependency
//! `zfb` already has on Unix: a new dependency edge would change
//! `Cargo.lock`, whose digest is pinned by governed contracts (#4481).
//!
//! [`ProcessTreeGuard`] spawns a [`std::process::Command`] so that the child
//! *and every descendant it creates* can be killed together, and kills that
//! whole tree when the guard drops:
//!
//! - **Unix:** the child is spawned in a new process group
//!   (`process_group(0)`, so its pgid equals its pid). Drop sends `SIGKILL`
//!   to the group and reaps the direct child.
//! - **Windows:** Drop runs `taskkill /T /F /PID <pid>` on the spawned
//!   process and reaps it. `taskkill /T` follows parent-pid links, so a
//!   descendant whose parent already exited is not reached.
//!
//! # Reap lifecycle
//!
//! Only the guard waits on the direct child, and every path that can reap
//! it ([`ProcessTreeGuard::try_wait`], [`ProcessTreeGuard::wait_timeout`],
//! [`ProcessTreeGuard::kill_tree`], drop) records the exit status; there is
//! no mutable access to the [`Child`]. After the reap the guard never
//! signals again: every later call returns the recorded status.
//!
//! On Unix an unreaped leader, even a zombie, keeps its pid and therefore
//! the group id from being reused, while after the reap a signal to `-pgid`
//! could reach an unrelated group. So the guard checks for the leader's exit
//! without reaping it (`waitid` with `WNOWAIT`), sends `SIGKILL` to the
//! group, and only then reaps. Observing the leader's exit therefore also
//! ends any descendant still in its group: no grandchild outlives the
//! guard, even one whose parent exited first.
//!
//! # Graceful interrupt
//!
//! [`ProcessTreeGuard::interrupt`] delivers the signal `zfb dev` treats as a
//! graceful stop, and [`ProcessTreeGuard::interrupt_and_wait`] waits with a
//! timeout. A timeout is reported as `None`, never as a graceful stop; the
//! kill-tree Drop (or [`ProcessTreeGuard::kill_tree`]) remains the fallback.
//!
//! - **Unix:** `SIGINT` to the guard's process group.
//!   [`ProcessTreeSpawner::interruptible`] resets an inherited ignored
//!   `SIGINT` in the child.
//! - **Windows:** `zfb dev` awaits `tokio::signal::ctrl_c()`, which only a
//!   console `CTRL_C_EVENT` reaches, and that event cannot target a nonzero
//!   process group. [`ProcessTreeSpawner::interruptible`] therefore ports
//!   `prototypes/direct-v8/console-ctrl-c.py` (the mechanism proven by
//!   `direct-v8.yml`'s windows-x64 lifecycle): a PowerShell helper is
//!   spawned with `CREATE_NEW_CONSOLE`, clears its inherited "ignore Ctrl-C"
//!   attribute, and starts the real command, which inherits the private
//!   console, the cleared attribute and the helper's stdout/stderr. A line on
//!   the helper's stdin (or EOF) makes it ignore Ctrl-C itself and raise
//!   `CTRL_C_EVENT` on its own console, reaching only the command's tree,
//!   never the test runner. The helper exits with the command's exit code,
//!   and as soon as the command does. [`ProcessTreeGuard::id`] is the real
//!   command's pid, not the helper's.
//!
//! Because the Windows command runs under the helper, an interruptible
//! spawn must take its stdout/stderr through [`ProcessTreeSpawner::stdout`]
//! and [`ProcessTreeSpawner::stderr`]: stdio set on the `Command` itself is
//! not carried over (its program, arguments, environment changes and
//! working directory are; `env_clear` is not).

#![allow(dead_code)]

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Builder for [`ProcessTreeGuard`]. `ProcessTreeGuard::spawn` is the
/// shorthand for `ProcessTreeSpawner::new().spawn(cmd)`.
#[derive(Debug, Default)]
pub struct ProcessTreeSpawner {
    interruptible: bool,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

impl ProcessTreeSpawner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the child stoppable with [`ProcessTreeGuard::interrupt`]. On Unix
    /// it resets `SIGINT` to its default disposition in the child: a parent
    /// started as a background job of a non-interactive shell (as
    /// `heavy-guard.sh` does) has `SIGINT` ignored, the child inherits that,
    /// and `sh` cannot even `trap` a signal ignored on entry. On Windows it
    /// runs the child under the private-console helper (see the module docs).
    pub fn interruptible(mut self) -> Self {
        self.interruptible = true;
        self
    }

    /// Stdout of the guarded command (see the module docs for why an
    /// interruptible spawn must set it here rather than on the `Command`).
    pub fn stdout(mut self, stdout: impl Into<Stdio>) -> Self {
        self.stdout = Some(stdout.into());
        self
    }

    /// Stderr of the guarded command; see [`stdout`](Self::stdout).
    pub fn stderr(mut self, stderr: impl Into<Stdio>) -> Self {
        self.stderr = Some(stderr.into());
        self
    }

    /// Spawn `cmd` inside a killable process tree.
    pub fn spawn(mut self, cmd: &mut Command) -> io::Result<ProcessTreeGuard> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            self.apply_stdio(cmd);
            cmd.process_group(0);
            if self.interruptible {
                // SAFETY: `signal` is async-signal-safe, as `pre_exec` requires.
                unsafe {
                    cmd.pre_exec(|| {
                        if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                            return Err(io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let child = cmd.spawn()?;
            let pgid = child.id() as libc::pid_t;
            Ok(ProcessTreeGuard {
                child,
                pgid,
                status: None,
            })
        }
        #[cfg(windows)]
        {
            if self.interruptible {
                let (mut helper, inner_pid) = windows::spawn_under_console_helper(
                    cmd,
                    self.stdout.take(),
                    self.stderr.take(),
                )?;
                let control = helper.stdin.take();
                return Ok(ProcessTreeGuard {
                    child: helper,
                    inner_pid,
                    control,
                    status: None,
                });
            }
            self.apply_stdio(cmd);
            let child = cmd.spawn()?;
            let inner_pid = child.id();
            Ok(ProcessTreeGuard {
                child,
                inner_pid,
                control: None,
                status: None,
            })
        }
    }

    fn apply_stdio(&mut self, cmd: &mut Command) {
        if let Some(stdout) = self.stdout.take() {
            cmd.stdout(stdout);
        }
        if let Some(stderr) = self.stderr.take() {
            cmd.stderr(stderr);
        }
    }
}

/// A spawned child whose whole process tree is killed on drop. See the
/// module docs for the per-platform mechanism.
#[derive(Debug)]
pub struct ProcessTreeGuard {
    /// The spawned process: the command itself, or on an interruptible
    /// Windows spawn the console helper that runs it.
    child: Child,
    #[cfg(unix)]
    pgid: libc::pid_t,
    #[cfg(windows)]
    inner_pid: u32,
    /// The console helper's stdin; a line on it requests the Ctrl-C.
    #[cfg(windows)]
    control: Option<std::process::ChildStdin>,
    /// The exit status, recorded whenever `child` is reaped. `Some` means
    /// its pid (and on Unix its group id) may already be reused.
    status: Option<ExitStatus>,
}

impl ProcessTreeGuard {
    /// Spawn `cmd` with the default [`ProcessTreeSpawner`].
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        ProcessTreeSpawner::new().spawn(cmd)
    }

    /// OS process id of the guarded command (never the Windows helper's).
    pub fn id(&self) -> u32 {
        #[cfg(unix)]
        {
            self.child.id()
        }
        #[cfg(windows)]
        {
            self.inner_pid
        }
    }

    /// Process group id of the tree (equal to the direct child's pid).
    #[cfg(unix)]
    pub fn process_group_id(&self) -> libc::pid_t {
        self.pgid
    }

    /// The spawned process (the console helper on an interruptible Windows
    /// spawn, whose exit status mirrors the command's). Deliberately shared
    /// only: waiting on it directly would bypass the reap lifecycle (see the
    /// module docs).
    pub fn child(&self) -> &Child {
        &self.child
    }

    /// The recorded exit status, or `Ok(None)` while the spawned process is
    /// running. On Unix, observing its exit kills the rest of its process
    /// group before reaping it (see the module docs).
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        #[cfg(unix)]
        {
            if !self.leader_exited()? {
                return Ok(None);
            }
            self.reap().map(Some)
        }
        #[cfg(windows)]
        {
            match self.child.try_wait()? {
                Some(status) => {
                    self.status = Some(status);
                    Ok(Some(status))
                }
                None => Ok(None),
            }
        }
    }

    /// Whether the leader has exited, without reaping it: the unreaped
    /// zombie keeps the pgid from being reused.
    #[cfg(unix)]
    fn leader_exited(&self) -> io::Result<bool> {
        loop {
            // SAFETY: an all-zero `siginfo_t` is valid; `waitid` writes it.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: plain syscall on our own unreaped child; `WNOWAIT`
            // leaves it waitable for `reap`.
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pgid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if rc == 0 {
                // With `WNOHANG`, a child that has not exited leaves
                // `si_pid` zero.
                // SAFETY: `waitid` filled (or left zeroed) the SIGCHLD fields.
                return Ok(unsafe { info.si_pid() } != 0);
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }

    /// Send `signal` to the process group. Only called while the leader is
    /// unreaped, so the group id cannot belong to anyone else.
    #[cfg(unix)]
    fn signal_group(&self, signal: libc::c_int) -> io::Result<()> {
        debug_assert!(self.status.is_none(), "signal after reap");
        // SAFETY: plain syscall on the group this guard created.
        if unsafe { libc::kill(-self.pgid, signal) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        // ESRCH: every member is gone (the leader at most a zombie).
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(err)
    }

    /// The single reap path: kill whatever remains of the tree while the
    /// leader still pins its pid, then wait for the leader and record the
    /// status.
    fn reap(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        #[cfg(unix)]
        {
            // Best effort: a failure here must not skip the reap.
            let _ = self.signal_group(libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            // The unreaped `Child` keeps its pid from being reused.
            windows::taskkill_tree(self.child.id());
        }
        let status = self.child.wait()?;
        self.status = Some(status);
        Ok(status)
    }

    /// Ask the tree to stop gracefully: `SIGINT` to the process group on
    /// Unix, a console `CTRL_C_EVENT` on Windows (which requires
    /// [`ProcessTreeSpawner::interruptible`]). Returns once the request is
    /// sent; it does not wait for the child to exit. Once the spawned
    /// process has been reaped there is nothing left to interrupt, and this
    /// is a no-op.
    pub fn interrupt(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            self.signal_group(libc::SIGINT).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("SIGINT to process group {} failed: {err}", self.pgid),
                )
            })
        }
        #[cfg(windows)]
        {
            use std::io::Write;
            let Some(control) = self.control.as_mut() else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "interrupt needs a child spawned with ProcessTreeSpawner::interruptible()",
                ));
            };
            control
                .write_all(b"interrupt\n")
                .and_then(|()| control.flush())
                .map_err(|err| {
                    io::Error::new(
                        err.kind(),
                        format!("request Ctrl-C from the console helper: {err}"),
                    )
                })
        }
    }

    /// Wait up to `timeout` for the spawned process to exit, through
    /// [`try_wait`](Self::try_wait). `Ok(None)` means it is still running.
    /// The rest of the tree is still killed on drop.
    pub fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(20)));
        }
    }

    /// [`interrupt`](Self::interrupt), then [`wait_timeout`](Self::wait_timeout).
    /// `Ok(Some(status))` is a graceful stop; `Ok(None)` means the child
    /// ignored the interrupt for `timeout` and is NOT stopped — call
    /// [`kill_tree`](Self::kill_tree) or drop the guard, and do not report
    /// that as graceful. On an already-reaped process it returns the
    /// recorded status without signalling anything.
    pub fn interrupt_and_wait(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        self.interrupt()?;
        self.wait_timeout(timeout)
    }

    /// Kill the whole tree now and reap the spawned process. Idempotent:
    /// once reaped it signals nothing and returns the recorded status. Drop
    /// calls it too.
    pub fn kill_tree(&mut self) -> io::Result<ExitStatus> {
        self.reap()
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        let _ = self.reap();
    }
}

/// Standard padded base64 (RFC 4648), for PowerShell `-EncodedCommand` and
/// the helper's embedded literals.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `-EncodedCommand` takes base64 of the UTF-16LE script, which avoids every
/// command-line quoting rule.
pub fn encode_powershell_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

/// Quote one argument by the MSVC `CommandLineToArgvW` rules, so the helper
/// can rebuild the command line the Windows child parses back into `argv`.
pub fn windows_quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    // Win32 `CREATE_NEW_CONSOLE` process-creation flag.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    /// The helper. Mirrors `prototypes/direct-v8/console-ctrl-c.py`;
    /// `__PROGRAM__`, `__ARGS__` and `__PID_FILE__` are base64 UTF-8 literals.
    /// `RedirectStandardInput` makes .NET pass the helper's own (made
    /// inheritable) stdout/stderr handles to the command, the same handle
    /// plumbing `subprocess.Popen` does in the prototype.
    const HELPER_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
Add-Type -Namespace ZfbProcessGuard -Name Native -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetConsoleCtrlHandler(System.IntPtr handler, bool add);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool GenerateConsoleCtrlEvent(uint ctrlEvent, uint processGroupId);
[DllImport("kernel32.dll", SetLastError = true)] public static extern System.IntPtr GetStdHandle(int stdHandle);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetHandleInformation(System.IntPtr handle, uint mask, uint flags);
'@
function Decode($b64) { [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($b64)) }
$program = Decode '__PROGRAM__'
$arguments = Decode '__ARGS__'
$pidFile = Decode '__PID_FILE__'
# The "ignore Ctrl-C" attribute is inherited at process creation and a CI
# runner's process chain may carry it; clear it so the command inherits a
# console on which Ctrl-C reaches its handler.
[void][ZfbProcessGuard.Native]::SetConsoleCtrlHandler([IntPtr]::Zero, $false)
foreach ($std in -11, -12) {
  $handle = [ZfbProcessGuard.Native]::GetStdHandle($std)
  if ($handle -ne [IntPtr]::Zero) { [void][ZfbProcessGuard.Native]::SetHandleInformation($handle, 1, 1) }
}
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $program
$psi.Arguments = $arguments
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$child = [System.Diagnostics.Process]::Start($psi)
$child.StandardInput.Close()
[System.IO.File]::WriteAllText($pidFile, [string]$child.Id)
$control = New-Object System.IO.StreamReader([Console]::OpenStandardInput())
$request = $control.ReadLineAsync()
$requested = $false
while (-not $child.HasExited -and -not $requested) {
  # A broken control pipe counts as a request, like EOF does.
  try { $requested = $request.Wait(50) } catch { $requested = $true }
}
if (-not $child.HasExited) {
  # The event reaches every process on this console; the helper ignores it.
  [void][ZfbProcessGuard.Native]::SetConsoleCtrlHandler([IntPtr]::Zero, $true)
  if ([ZfbProcessGuard.Native]::GenerateConsoleCtrlEvent(0, 0)) {
    [Console]::Error.WriteLine('process-guard: CTRL_C_EVENT delivered')
  } else {
    [Console]::Error.WriteLine('process-guard: GenerateConsoleCtrlEvent failed: ' + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
    $child.Kill()
  }
}
$child.WaitForExit()
[Environment]::Exit($child.ExitCode)
"#;

    /// Spawn `cmd` under the console helper. Returns the helper and the
    /// pid of the real command it started.
    pub(super) fn spawn_under_console_helper(
        cmd: &Command,
        stdout: Option<Stdio>,
        stderr: Option<Stdio>,
    ) -> io::Result<(Child, u32)> {
        let program = cmd.get_program().to_string_lossy().into_owned();
        let arguments = cmd
            .get_args()
            .map(|arg| super::windows_quote_arg(&arg.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        let pid_dir = tempfile::tempdir()?;
        let pid_file = pid_dir.path().join("child.pid");
        let literal = |text: &str| super::base64(text.as_bytes());
        let script = HELPER_SCRIPT
            .replace("__PROGRAM__", &literal(&program))
            .replace("__ARGS__", &literal(&arguments))
            .replace("__PID_FILE__", &literal(&pid_file.to_string_lossy()));

        let mut helper = Command::new("powershell.exe");
        helper
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(super::encode_powershell_command(&script))
            .stdin(Stdio::piped())
            .stdout(stdout.unwrap_or_else(Stdio::inherit))
            .stderr(stderr.unwrap_or_else(Stdio::inherit))
            .creation_flags(CREATE_NEW_CONSOLE);
        if let Some(dir) = cmd.get_current_dir() {
            helper.current_dir(dir);
        }
        for (key, value) in cmd.get_envs() {
            match value {
                Some(value) => helper.env(key, value),
                None => helper.env_remove(key),
            };
        }
        let mut helper = helper.spawn().map_err(|err| {
            io::Error::new(err.kind(), format!("spawn the console helper: {err}"))
        })?;

        // Add-Type compiles C#, which takes a few seconds on a cold runner.
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return Ok((helper, pid));
                }
            }
            if let Some(status) = helper.try_wait()? {
                return Err(io::Error::other(format!(
                    "console helper exited ({status}) before starting `{program}`"
                )));
            }
            if Instant::now() >= deadline {
                taskkill_tree(helper.id());
                let _ = helper.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("console helper did not start `{program}` within 120s"),
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `taskkill /T /F` on `pid` and its descendants. Best effort and
    /// idempotent: an already-exited tree makes taskkill fail, which is fine.
    pub(super) fn taskkill_tree(pid: u32) {
        let _ = Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
