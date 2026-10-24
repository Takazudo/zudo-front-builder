//! Cross-platform kill-tree guard for long-running child processes in tests
//! (typically `zfb dev`), issue #4483.
//!
//! [`ProcessTreeGuard`] spawns a [`std::process::Command`] so that the child
//! *and every descendant it creates* can be killed together, and kills that
//! whole tree when the guard drops:
//!
//! - **Unix:** the child is spawned in a new process group
//!   (`process_group(0)`, so its pgid equals its pid). Drop sends `SIGKILL`
//!   to the group and reaps the direct child.
//! - **Windows:** the child is spawned `CREATE_SUSPENDED`, assigned to a
//!   fresh job object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
//!   and only then resumed — so it cannot spawn a descendant outside the job.
//!   The job handle is not inheritable. A failed assignment terminates the
//!   suspended child and returns an error. Drop terminates the job, closes
//!   it, and reaps the direct child.
//!
//! # Graceful interrupt (#4484)
//!
//! [`ProcessTreeGuard::interrupt`] delivers the signal `zfb dev` treats as a
//! graceful stop, and [`ProcessTreeGuard::interrupt_and_wait`] waits for the
//! direct child with a timeout. A timeout is reported as `None`, never as a
//! graceful stop; the guard's kill-tree Drop (or [`ProcessTreeGuard::kill_tree`])
//! remains the fallback. Spawning still goes through the single path,
//! [`ProcessTreeSpawner::spawn`]:
//!
//! - **Unix:** `SIGINT` to the guard's process group (`-pgid`).
//!   [`ProcessTreeSpawner::interruptible`] resets an inherited ignored
//!   `SIGINT` in the child.
//! - **Windows:** `zfb dev` awaits `tokio::signal::ctrl_c()`, which only a
//!   console `CTRL_C_EVENT` reaches, and that event cannot target a nonzero
//!   process group (Ctrl-C is disabled in groups created with
//!   `CREATE_NEW_PROCESS_GROUP`). The port of
//!   `prototypes/direct-v8/console-ctrl-c.py` (the mechanism proven by
//!   `direct-v8.yml`'s windows-x64 lifecycle) is:
//!   1. [`ProcessTreeSpawner::interruptible`] spawns the child with
//!      `CREATE_NEW_CONSOLE`, so its console holds only the child's tree,
//!      never the test runner. Before spawning it clears this process's
//!      inherited "ignore Ctrl-C" attribute (`SetConsoleCtrlHandler(NULL,
//!      FALSE)`), because the child inherits that attribute at creation and a
//!      CI runner's process chain may carry it.
//!   2. `interrupt` runs a short-lived, windowless PowerShell helper that
//!      leaves its own console, attaches to the child's console, ignores
//!      Ctrl-C itself, and broadcasts `CTRL_C_EVENT` to process group 0 of
//!      that console only. The test runner's console is never signalled.
//!
//! [`ProcessTreeSpawner::windows_creation_flags`] remains the generic
//! extension point; its flags are ORed with the guard's own
//! `CREATE_SUSPENDED`. Callers must not call `CommandExt::creation_flags` on
//! the `Command` themselves — the spawner overwrites it.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// Builder for [`ProcessTreeGuard`]. `ProcessTreeGuard::spawn` is the
/// shorthand for `ProcessTreeSpawner::new().spawn(cmd)`.
#[derive(Debug, Default, Clone)]
pub struct ProcessTreeSpawner {
    #[cfg(windows)]
    extra_creation_flags: u32,
    interruptible: bool,
}

impl ProcessTreeSpawner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Extra Win32 process-creation flags (e.g. `CREATE_NEW_CONSOLE`), ORed
    /// with the `CREATE_SUSPENDED` the guard always needs.
    #[cfg(windows)]
    pub fn windows_creation_flags(mut self, flags: u32) -> Self {
        self.extra_creation_flags |= flags;
        self
    }

    /// Make the child stoppable with [`ProcessTreeGuard::interrupt`]. On Unix
    /// it resets `SIGINT` to its default disposition in the child: a parent
    /// started as a background job of a non-interactive shell (as
    /// `heavy-guard.sh` does) has `SIGINT` ignored, the child inherits that,
    /// and `sh` cannot even `trap` a signal ignored on entry. On Windows it
    /// gives the child a private console (see the module docs).
    pub fn interruptible(mut self) -> Self {
        self.interruptible = true;
        self
    }

    /// Spawn `cmd` inside a killable process tree.
    pub fn spawn(&self, cmd: &mut Command) -> io::Result<ProcessTreeGuard> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
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
                reaped: false,
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
            let job = windows::Job::new()?;
            let mut flags = self.extra_creation_flags | CREATE_SUSPENDED;
            if self.interruptible {
                windows::accept_ctrl_c();
                flags |= windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;
            }
            cmd.creation_flags(flags);
            let mut child = cmd.spawn()?;
            if let Err(err) = job.adopt_suspended(&child) {
                // The child is still suspended and outside any job we own:
                // kill it rather than leak it, then fail loudly.
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
            Ok(ProcessTreeGuard {
                child,
                job: Some(job),
                interruptible: self.interruptible,
                reaped: false,
            })
        }
    }
}

/// A spawned child whose whole process tree is killed on drop. See the
/// module docs for the per-platform mechanism.
#[derive(Debug)]
pub struct ProcessTreeGuard {
    child: Child,
    #[cfg(unix)]
    pgid: libc::pid_t,
    #[cfg(windows)]
    job: Option<windows::Job>,
    #[cfg(windows)]
    interruptible: bool,
    reaped: bool,
}

impl ProcessTreeGuard {
    /// Spawn `cmd` with the default [`ProcessTreeSpawner`].
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        ProcessTreeSpawner::new().spawn(cmd)
    }

    /// OS process id of the direct child.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Process group id of the tree (equal to the direct child's pid).
    #[cfg(unix)]
    pub fn process_group_id(&self) -> libc::pid_t {
        self.pgid
    }

    /// Raw handle of the job object holding the tree, while it is alive.
    #[cfg(windows)]
    pub fn job_handle(&self) -> Option<windows_sys::Win32::Foundation::HANDLE> {
        self.job.as_ref().map(|job| job.handle)
    }

    pub fn child(&self) -> &Child {
        &self.child
    }

    /// Mutable access to the direct child (stdio pipes, `wait`, ...). Waiting
    /// on it directly does not kill the rest of the tree; drop still does.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Ask the tree to stop gracefully: `SIGINT` to the process group on
    /// Unix, a console `CTRL_C_EVENT` on Windows (which requires
    /// [`ProcessTreeSpawner::interruptible`]). Returns once the signal is
    /// delivered; it does not wait for the child to exit.
    pub fn interrupt(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            // SAFETY: plain syscall on the group this guard created.
            if unsafe { libc::kill(-self.pgid, libc::SIGINT) } != 0 {
                let err = io::Error::last_os_error();
                return Err(io::Error::new(
                    err.kind(),
                    format!("SIGINT to process group {} failed: {err}", self.pgid),
                ));
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            if !self.interruptible {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "interrupt needs a child spawned with ProcessTreeSpawner::interruptible()",
                ));
            }
            windows::send_ctrl_c_to_console_of(self.child.id())
        }
    }

    /// Wait up to `timeout` for the direct child to exit. `Ok(None)` means it
    /// is still running. The rest of the tree is still killed on drop.
    pub fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
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
    /// that as graceful.
    pub fn interrupt_and_wait(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        self.interrupt()?;
        self.wait_timeout(timeout)
    }

    /// Kill the whole tree now and reap the direct child. Idempotent; drop
    /// calls it too.
    pub fn kill_tree(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        {
            // SAFETY: plain syscall; a stale or empty group yields ESRCH,
            // which is the "already dead" case and needs no handling.
            unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        }
        #[cfg(windows)]
        {
            // Dropping the job terminates and closes it (KILL_ON_JOB_CLOSE).
            drop(self.job.take());
        }
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.kill_tree();
        }
    }
}

/// Standard padded base64 (for PowerShell `-EncodedCommand`).
#[cfg(any(windows, test))]
fn base64(bytes: &[u8]) -> String {
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

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    /// Owned job object handle. Created with null security attributes, so it
    /// is not inheritable by the child.
    #[derive(Debug)]
    pub(super) struct Job {
        pub(super) handle: HANDLE,
    }

    // SAFETY: a job object handle is a kernel handle usable from any thread;
    // the raw pointer type is the only reason the auto traits are missing.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub(super) fn new() -> io::Result<Self> {
            // SAFETY: null attributes and name are valid arguments.
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(last_error("CreateJobObjectW"));
            }
            let job = Job { handle };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `info` is a valid, correctly sized struct for this class.
            let ok = unsafe {
                SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(last_error("SetInformationJobObject"));
            }
            Ok(job)
        }

        /// Assign a child spawned with `CREATE_SUSPENDED` to this job, then
        /// resume it. Because the child has not run a single instruction
        /// before assignment, every descendant it creates inherits the job.
        pub(super) fn adopt_suspended(&self, child: &Child) -> io::Result<()> {
            let process = child.as_raw_handle() as HANDLE;
            // SAFETY: both handles are valid and owned for this call.
            if unsafe { AssignProcessToJobObject(self.handle, process) } == 0 {
                return Err(last_error("AssignProcessToJobObject"));
            }
            resume_process_threads(child.id())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: `handle` is a job handle we own and close exactly once.
            unsafe {
                TerminateJobObject(self.handle, 1);
                CloseHandle(self.handle);
            }
        }
    }

    // `std::process::Command` closes the primary-thread handle CreateProcessW
    // returns, so the suspended thread is found through a ToolHelp snapshot.
    fn resume_process_threads(pid: u32) -> io::Result<()> {
        // SAFETY: plain snapshot call; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateToolhelp32Snapshot"));
        }
        let result = resume_threads_in_snapshot(snapshot, pid);
        // SAFETY: valid snapshot handle, closed once.
        unsafe { CloseHandle(snapshot) };
        result
    }

    fn resume_threads_in_snapshot(snapshot: HANDLE, pid: u32) -> io::Result<()> {
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = 0usize;
        // SAFETY: `entry` is a valid THREADENTRY32 with dwSize set.
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: plain handle-returning call; closed below.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(last_error("OpenThread"));
                }
                // SAFETY: valid thread handle opened with SUSPEND_RESUME.
                let previous = unsafe { ResumeThread(thread) };
                // SAFETY: closed exactly once.
                unsafe { CloseHandle(thread) };
                if previous == u32::MAX {
                    return Err(last_error("ResumeThread"));
                }
                resumed += 1;
            }
            // SAFETY: as above.
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        if resumed == 0 {
            return Err(io::Error::other(format!(
                "no thread of suspended child pid {pid} found to resume"
            )));
        }
        Ok(())
    }

    /// Clear this process's "ignore Ctrl-C" attribute so a child created
    /// next inherits a console on which Ctrl-C reaches its handler. Mirrors
    /// `accept_ctrl_c` in `prototypes/direct-v8/console-ctrl-c.py`. The
    /// attribute is process-wide; restoring normal Ctrl-C processing is the
    /// default state, and this process's own console is never signalled by
    /// `send_ctrl_c_to_console_of`.
    pub(super) fn accept_ctrl_c() {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        // SAFETY: a null handler with FALSE only clears the ignore attribute.
        // A failure (e.g. no console) leaves the child with the attribute it
        // would have inherited anyway, which `interrupt` then cannot clear;
        // the caller's timeout surfaces that as a non-graceful stop.
        unsafe { SetConsoleCtrlHandler(None, 0) };
    }

    /// Run a short-lived windowless PowerShell helper that detaches from its
    /// own console, attaches to `pid`'s console, ignores Ctrl-C itself, and
    /// broadcasts `CTRL_C_EVENT` within that console only (the
    /// `deliver_ctrl_c` step of `console-ctrl-c.py`). A helper process is
    /// required because console attachment is process-wide and must not
    /// disturb the test process.
    pub(super) fn send_ctrl_c_to_console_of(pid: u32) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        let script = format!(
            r#"$ErrorActionPreference = 'Stop'
Add-Type -Namespace ZfbTestUtils -Name ConsoleCtrl -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool FreeConsole();
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool AttachConsole(uint pid);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetConsoleCtrlHandler(System.IntPtr handler, bool add);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool GenerateConsoleCtrlEvent(uint ctrlEvent, uint processGroupId);
'@
function Fail($step, $code) {{
  [Console]::Error.WriteLine("console-ctrl-c: $step failed: " + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
  exit $code
}}
[void][ZfbTestUtils.ConsoleCtrl]::FreeConsole()
if (-not [ZfbTestUtils.ConsoleCtrl]::SetConsoleCtrlHandler([IntPtr]::Zero, $true)) {{ Fail 'SetConsoleCtrlHandler' 3 }}
if (-not [ZfbTestUtils.ConsoleCtrl]::AttachConsole({pid})) {{ Fail 'AttachConsole({pid})' 4 }}
if (-not [ZfbTestUtils.ConsoleCtrl]::GenerateConsoleCtrlEvent(0, 0)) {{ Fail 'GenerateConsoleCtrlEvent' 5 }}
exit 0
"#
        );
        let mut helper = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(encode_powershell_command(&script))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // Its own hidden console, never the test runner's.
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|err| {
                io::Error::new(err.kind(), format!("spawn console-ctrl-c helper: {err}"))
            })?;
        // Drain stderr on a thread so a chatty failure cannot fill the pipe
        // and stall the helper until the deadline.
        let stderr_reader = helper.stderr.take().map(|mut pipe| {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                text
            })
        });
        // Add-Type compiles C#, which takes a few seconds on a cold runner.
        let deadline = Instant::now() + Duration::from_secs(120);
        let status = loop {
            if let Some(status) = helper.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = helper.kill();
                let _ = helper.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "console-ctrl-c helper did not finish within 120s",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if status.success() {
            return Ok(());
        }
        let stderr = stderr_reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        Err(io::Error::other(format!(
            "console-ctrl-c helper for pid {pid} exited {status}: {}",
            stderr.trim()
        )))
    }

    /// `-EncodedCommand` takes base64 of the UTF-16LE script, which avoids
    /// every command-line quoting rule.
    pub(super) fn encode_powershell_command(script: &str) -> String {
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        super::base64(&bytes)
    }

    fn last_error(call: &str) -> io::Error {
        let err = io::Error::last_os_error();
        io::Error::new(err.kind(), format!("{call} failed: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn wait_for_pid_file(path: &Path, guard: &mut ProcessTreeGuard) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return pid;
                }
            }
            if let Some(status) = guard.try_wait().expect("try_wait") {
                panic!("child exited ({status}) before writing the grandchild pid");
            }
            assert!(
                Instant::now() < deadline,
                "grandchild pid file never appeared"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    fn pid_alive(pid: u32) -> bool {
        // SAFETY: signal 0 only probes for existence.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(unix)]
    #[test]
    fn drop_kills_child_and_grandchild() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 60 & echo $! > \"$1\"; sleep 60", "sh"])
            .arg(&pid_file);
        let mut guard = ProcessTreeGuard::spawn(&mut cmd).expect("spawn guarded sh");
        let child_pid = guard.id();
        assert_eq!(guard.process_group_id() as u32, child_pid);
        let grandchild_pid = wait_for_pid_file(&pid_file, &mut guard);
        assert!(pid_alive(grandchild_pid), "grandchild should be running");

        drop(guard);

        // The killed grandchild is reparented and reaped asynchronously by
        // init; until then signal 0 still succeeds on the zombie.
        let deadline = Instant::now() + Duration::from_secs(10);
        while pid_alive(grandchild_pid) {
            assert!(
                Instant::now() < deadline,
                "grandchild {grandchild_pid} survived the guard drop"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!pid_alive(child_pid), "direct child should be reaped");
    }

    #[cfg(windows)]
    #[test]
    fn drop_kills_child_and_grandchild() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow -FilePath ping.exe \
             -ArgumentList '-n','120','127.0.0.1'; \
             Set-Content -Encoding ascii -LiteralPath '{}' -Value $p.Id; Start-Sleep -Seconds 120",
            pid_file.display()
        );
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        let mut guard = ProcessTreeGuard::spawn(&mut cmd).expect("spawn guarded powershell");
        assert!(guard.job_handle().is_some());
        let grandchild_pid = wait_for_pid_file(&pid_file, &mut guard);

        // Hold a handle before the drop so pid reuse cannot fool the check.
        // SAFETY: plain handle-returning call; closed below.
        let grandchild = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, grandchild_pid) };
        assert!(
            !grandchild.is_null(),
            "open grandchild {grandchild_pid}: {}",
            std::io::Error::last_os_error()
        );

        drop(guard);

        // SAFETY: valid process handle; closed exactly once.
        let waited = unsafe { WaitForSingleObject(grandchild, 10_000) };
        unsafe { CloseHandle(grandchild) };
        assert_eq!(
            waited, WAIT_OBJECT_0,
            "grandchild {grandchild_pid} survived the guard drop"
        );
    }

    fn wait_for_file(path: &Path, guard: &mut ProcessTreeGuard, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !path.exists() {
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

    #[cfg(unix)]
    #[test]
    fn interrupt_stops_child_gracefully() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("graceful");
        let ready = dir.path().join("ready");
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "trap 'echo ok > \"$1\"; exit 0' INT; echo ready > \"$2\"; while :; do sleep 0.1; done",
            "sh",
        ])
        .arg(&marker)
        .arg(&ready);
        let mut guard = ProcessTreeSpawner::new()
            .interruptible()
            .spawn(&mut cmd)
            .expect("spawn guarded sh");
        wait_for_file(&ready, &mut guard, "the SIGINT trap was installed");

        let status = guard
            .interrupt_and_wait(Duration::from_secs(30))
            .expect("interrupt")
            .expect("child should stop gracefully on SIGINT");

        assert!(status.success(), "graceful exit status: {status}");
        let text = std::fs::read_to_string(&marker).expect("graceful marker");
        assert_eq!(text.trim(), "ok");
    }

    #[cfg(unix)]
    #[test]
    fn interrupt_timeout_is_not_reported_as_graceful() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ready = dir.path().join("ready");
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "trap '' INT; echo ready > \"$1\"; while :; do sleep 0.1; done",
            "sh",
        ])
        .arg(&ready);
        let mut guard = ProcessTreeGuard::spawn(&mut cmd).expect("spawn guarded sh");
        wait_for_file(&ready, &mut guard, "SIGINT was ignored");

        let outcome = guard
            .interrupt_and_wait(Duration::from_millis(500))
            .expect("interrupt");

        assert!(outcome.is_none(), "an ignored SIGINT must time out");
        let status = guard.kill_tree().expect("kill_tree fallback");
        assert!(
            !status.success(),
            "hard kill must not look like a clean exit"
        );
    }

    #[cfg(windows)]
    #[test]
    fn interrupt_stops_child_gracefully() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("graceful");
        let ready = dir.path().join("ready");
        // A .NET Console.CancelKeyPress handler stands in for zfb dev's
        // `tokio::signal::ctrl_c()`: both are console control handlers.
        let script = r#"
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Threading;
public static class ZfbCtrlCChild {
    public static void Run(string marker, string ready) {
        Console.CancelKeyPress += delegate(object sender, ConsoleCancelEventArgs e) {
            e.Cancel = true;
            File.WriteAllText(marker, "ok");
            Environment.Exit(0);
        };
        File.WriteAllText(ready, "ready");
        while (true) { Thread.Sleep(100); }
    }
}
'@
[ZfbCtrlCChild]::Run($env:ZFB_GRACEFUL_MARKER, $env:ZFB_GRACEFUL_READY)
"#;
        let mut cmd = Command::new("powershell.exe");
        // -EncodedCommand sidesteps Windows PowerShell's handling of the
        // double quotes in the C# source.
        cmd.args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(windows::encode_powershell_command(script))
            .env("ZFB_GRACEFUL_MARKER", &marker)
            .env("ZFB_GRACEFUL_READY", &ready);
        let mut guard = ProcessTreeSpawner::new()
            .interruptible()
            .spawn(&mut cmd)
            .expect("spawn guarded powershell");
        wait_for_file(&ready, &mut guard, "the Ctrl-C handler was installed");

        let status = guard
            .interrupt_and_wait(Duration::from_secs(60))
            .expect("interrupt")
            .expect("child should stop gracefully on console Ctrl-C");

        assert!(status.success(), "graceful exit status: {status}");
        let text = std::fs::read_to_string(&marker).expect("graceful marker");
        assert_eq!(text.trim(), "ok");
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
            .expect_err("plain spawn has no private console");
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }
}
