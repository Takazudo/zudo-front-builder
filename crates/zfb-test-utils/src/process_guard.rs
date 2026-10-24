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
//! # Extension point (graceful interrupt, #4484)
//!
//! Spawning goes through exactly one path, [`ProcessTreeSpawner::spawn`]. A
//! graceful-interrupt feature extends that builder rather than adding a second
//! spawn path: on Windows it can request a private console through
//! [`ProcessTreeSpawner::windows_creation_flags`] (e.g. `CREATE_NEW_CONSOLE`),
//! which is ORed with the guard's own `CREATE_SUSPENDED`. The interrupt then
//! targets [`ProcessTreeGuard::id`] / [`ProcessTreeGuard::process_group_id`]
//! (Unix `SIGINT` to `-pgid`) or the child's console, and still relies on the
//! guard's Drop as the kill-tree backstop. Callers must not call
//! `CommandExt::creation_flags` on the `Command` themselves — the spawner
//! overwrites it.

use std::io;
use std::process::{Child, Command, ExitStatus};

/// Builder for [`ProcessTreeGuard`]. `ProcessTreeGuard::spawn` is the
/// shorthand for `ProcessTreeSpawner::new().spawn(cmd)`.
#[derive(Debug, Default, Clone)]
pub struct ProcessTreeSpawner {
    #[cfg(windows)]
    extra_creation_flags: u32,
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

    /// Spawn `cmd` inside a killable process tree.
    pub fn spawn(&self, cmd: &mut Command) -> io::Result<ProcessTreeGuard> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
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
            cmd.creation_flags(self.extra_creation_flags | CREATE_SUSPENDED);
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
}
