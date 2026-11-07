//! The parent side: deadline, identity, publication permission and cleanup.
//!
//! Each job is owned by a dedicated supervisor thread, not by the caller's
//! future. Dropping a [`RunningJob`] (or the future awaiting it) revokes
//! publication; the supervisor then terminates the worker, reaps it and
//! removes the private directory on its own.
use crate::launcher::WorkerLauncher;
use crate::protocol::{
    Outcome, ReportedFile, TestHook, WorkerRequest, WorkerResponse, FILES_DIR, PROTOCOL_VERSION,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zfb_rolldown_prototype::{BundleRequest, OutputLayout};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// Who a job's worker is. The nonce is checked against the worker's answer
/// before anything it reports is trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerIdentity {
    job_id: String,
    nonce: String,
    pid: u32,
}

impl WorkerIdentity {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

#[derive(Debug)]
pub enum WorkerError {
    /// The host supplied no [`WorkerLauncher`].
    MissingLauncher,
    /// The job could not be prepared or the worker could not be started.
    Setup(String),
    /// The deadline passed. Publication was revoked and the worker reaped.
    Timeout { deadline: Duration },
    /// The job was cancelled. Publication was revoked and the worker reaped.
    Cancelled,
    /// The bundler reported an error; nothing was published.
    Compile { diagnostic: String },
    /// The worker's answer failed validation; nothing was published.
    Rejected { reason: String },
    /// The worker exited without a usable answer.
    WorkerFailed { status: String, stderr: String },
    /// Exit of the worker (or of a process holding its output) could not be
    /// confirmed. The private directory is retained, not deleted.
    ShutdownIncomplete { cause: String, private_dir: PathBuf },
    /// Writing a validated file to its destination failed.
    Publish {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The supervisor thread ended without reporting; the state is unknown.
    SupervisorLost,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLauncher => write!(
                f,
                "this host did not provide a compiler worker launcher; pass a \
                 zfb_compiler_worker::WorkerLauncher for an executable whose main calls \
                 zfb_compiler_worker::run_if_requested(), or run through the zfb CLI"
            ),
            Self::Setup(message) => write!(f, "compiler worker setup failed: {message}"),
            Self::Timeout { deadline } => write!(
                f,
                "compiler worker timed out after {deadline:?}; publication was revoked and the \
                 worker was terminated"
            ),
            Self::Cancelled => write!(
                f,
                "compiler worker job was cancelled; publication was revoked and the worker was \
                 terminated"
            ),
            Self::Compile { diagnostic } => write!(f, "{diagnostic}"),
            Self::Rejected { reason } => write!(
                f,
                "compiler worker result rejected, nothing published: {reason}"
            ),
            Self::WorkerFailed { status, stderr } => {
                write!(f, "compiler worker failed ({status})")?;
                if !stderr.trim().is_empty() {
                    write!(f, ": {}", stderr.trim())?;
                }
                Ok(())
            }
            Self::ShutdownIncomplete { cause, private_dir } => write!(
                f,
                "compiler worker shutdown is incomplete: {cause}; its private directory {} was \
                 retained",
                private_dir.display()
            ),
            Self::Publish { path, error } => {
                write!(f, "publish compiler output {}: {error}", path.display())
            }
            Self::SupervisorLost => write!(
                f,
                "compiler worker supervisor ended without a result; worker state is unknown"
            ),
        }
    }
}

impl std::error::Error for WorkerError {}

/// One compile: the request, the deadline for the whole worker run and an
/// optional test hook.
#[derive(Debug, Clone)]
pub struct CompileJob {
    pub request: BundleRequest,
    pub deadline: Duration,
    test_hook: Option<TestHook>,
}

impl CompileJob {
    pub fn new(request: BundleRequest, deadline: Duration) -> Self {
        Self {
            request,
            deadline,
            test_hook: None,
        }
    }

    #[doc(hidden)]
    pub fn with_test_hook(mut self, hook: TestHook) -> Self {
        self.test_hook = Some(hook);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct CompileOutput {
    pub identity: WorkerIdentity,
    /// Files published to the request's destinations, graph file included.
    pub files: Vec<PublishedFile>,
    /// The normalized module graph, as `generate` returned it.
    pub graph: serde_json::Value,
    pub warnings: Vec<String>,
}

/// Runs compile jobs in supervised worker processes.
#[derive(Debug, Clone)]
pub struct CompilerWorker {
    launcher: WorkerLauncher,
    scratch_root: PathBuf,
    shutdown_grace: Duration,
}

impl CompilerWorker {
    pub fn new(launcher: WorkerLauncher) -> Self {
        Self {
            launcher,
            scratch_root: std::env::temp_dir(),
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }

    /// For callers that receive the launcher from their host: a missing one
    /// is an actionable [`WorkerError::MissingLauncher`].
    pub fn require(launcher: Option<WorkerLauncher>) -> Result<Self, WorkerError> {
        launcher.map(Self::new).ok_or(WorkerError::MissingLauncher)
    }

    /// Where private job directories are created. Defaults to the system
    /// temporary directory.
    pub fn with_scratch_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.scratch_root = root.into();
        self
    }

    /// How long to wait for a terminated worker to be confirmed gone before
    /// reporting [`WorkerError::ShutdownIncomplete`].
    pub fn with_shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    pub fn launcher(&self) -> &WorkerLauncher {
        &self.launcher
    }

    pub async fn compile(&self, job: CompileJob) -> Result<CompileOutput, WorkerError> {
        self.start(job)?.wait().await
    }

    /// Must not be called from inside an async runtime.
    pub fn compile_blocking(&self, job: CompileJob) -> Result<CompileOutput, WorkerError> {
        self.start(job)?.wait_blocking()
    }

    /// Spawns the worker and its supervisor; the deadline starts now.
    pub fn start(&self, job: CompileJob) -> Result<RunningJob, WorkerError> {
        let destinations = Destinations::of(&job.request)?;
        let job_id = next_job_id();
        let nonce = random_nonce();
        let private_dir = create_private_dir(&self.scratch_root, &job_id, &nonce)?;
        let payload = serde_json::to_vec(&WorkerRequest {
            protocol: PROTOCOL_VERSION,
            job_id: job_id.clone(),
            nonce: nonce.clone(),
            private_dir: private_dir.clone(),
            request: job.request,
            test_hook: job.test_hook,
        })
        .map_err(|error| WorkerError::Setup(format!("encode request: {error}")))?;

        let mut command = self.launcher.command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&private_dir);
                return Err(WorkerError::Setup(format!(
                    "spawn {}: {error}",
                    self.launcher.program().display()
                )));
            }
        };
        let started = Instant::now();
        let identity = WorkerIdentity {
            job_id,
            nonce,
            pid: child.id(),
        };

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (stdout_tx, stdout_rx) = mpsc::channel();
        let (stderr_tx, stderr_rx) = mpsc::channel();
        let spawned = (|| -> std::io::Result<()> {
            // Detached: a dead worker closes the pipe, which ends the write.
            std::thread::Builder::new()
                .name("zfb-compiler-worker-stdin".into())
                .spawn(move || {
                    let _ = stdin.write_all(&payload);
                })?;
            std::thread::Builder::new()
                .name("zfb-compiler-worker-stdout".into())
                .spawn(move || {
                    let _ = stdout_tx.send(read_capped(stdout, MAX_RESPONSE_BYTES));
                })?;
            std::thread::Builder::new()
                .name("zfb-compiler-worker-stderr".into())
                .spawn(move || {
                    let _ = stderr_tx.send(read_capped(stderr, MAX_STDERR_BYTES));
                })?;
            Ok(())
        })();
        let shared = Arc::new(Shared {
            stage: Mutex::new(Stage::Running),
        });
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let mut supervision = Supervision {
            child,
            identity: identity.clone(),
            shared: shared.clone(),
            deadline: job.deadline,
            deadline_at: started + job.deadline,
            grace: self.shutdown_grace,
            private_dir,
            destinations,
            stdout_rx,
            stderr_rx,
            reaped: false,
        };
        if let Err(error) = spawned {
            let _ = supervision.terminate(Stop::Lost(format!("start I/O threads: {error}")));
            return Err(WorkerError::Setup(format!("start I/O threads: {error}")));
        }
        std::thread::Builder::new()
            .name("zfb-compiler-worker-supervisor".into())
            .spawn(move || {
                let _ = result_tx.send(supervision.run());
            })
            .map_err(|error| WorkerError::Setup(format!("start supervisor thread: {error}")))?;
        Ok(RunningJob {
            shared,
            result: Some(result_rx),
            identity,
        })
    }
}

/// A started job. Dropping it, or a future awaiting [`RunningJob::wait`],
/// revokes publication; the supervisor still terminates, reaps and cleans up.
#[derive(Debug)]
pub struct RunningJob {
    shared: Arc<Shared>,
    result: Option<tokio::sync::oneshot::Receiver<Result<CompileOutput, WorkerError>>>,
    identity: WorkerIdentity,
}

impl RunningJob {
    pub fn identity(&self) -> &WorkerIdentity {
        &self.identity
    }

    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle(self.shared.clone())
    }

    /// Revokes publication unless it already began, and stops the worker.
    pub fn cancel(&self) {
        self.shared.revoke(StopReason::Cancelled);
    }

    /// Resolves once the worker is reaped (or shutdown is reported
    /// incomplete) and, on success, its files are published.
    pub async fn wait(mut self) -> Result<CompileOutput, WorkerError> {
        let receiver = self.result.as_mut().expect("result not yet taken");
        let result = receiver.await;
        self.result = None;
        result.unwrap_or(Err(WorkerError::SupervisorLost))
    }

    /// Must not be called from inside an async runtime.
    pub fn wait_blocking(mut self) -> Result<CompileOutput, WorkerError> {
        let receiver = self.result.take().expect("result not yet taken");
        receiver
            .blocking_recv()
            .unwrap_or(Err(WorkerError::SupervisorLost))
    }
}

impl Drop for RunningJob {
    fn drop(&mut self) {
        if self.result.is_some() {
            self.shared.revoke(StopReason::Abandoned);
        }
    }
}

/// Cancels one job from anywhere.
#[derive(Debug, Clone)]
pub struct CancelHandle(Arc<Shared>);

impl CancelHandle {
    pub fn cancel(&self) {
        self.0.revoke(StopReason::Cancelled);
    }
}

#[derive(Debug)]
struct Shared {
    stage: Mutex<Stage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Running,
    Revoked(StopReason),
    /// Publication began; it is no longer revocable.
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Timeout,
    Cancelled,
    Abandoned,
}

impl Shared {
    fn stage(&self) -> std::sync::MutexGuard<'_, Stage> {
        self.stage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn revoke(&self, reason: StopReason) {
        let mut stage = self.stage();
        if *stage == Stage::Running {
            *stage = Stage::Revoked(reason);
        }
    }
}

enum Stop {
    Revoked(StopReason),
    Lost(String),
}

impl Stop {
    fn describe(&self) -> String {
        match self {
            Self::Revoked(StopReason::Timeout) => "the deadline passed".into(),
            Self::Revoked(StopReason::Cancelled) => "the job was cancelled".into(),
            Self::Revoked(StopReason::Abandoned) => "the caller dropped the job".into(),
            Self::Lost(cause) => cause.clone(),
        }
    }
}

type Captured = std::io::Result<(Vec<u8>, bool)>;

struct Supervision {
    child: Child,
    identity: WorkerIdentity,
    shared: Arc<Shared>,
    deadline: Duration,
    deadline_at: Instant,
    grace: Duration,
    private_dir: PathBuf,
    destinations: Destinations,
    stdout_rx: mpsc::Receiver<Captured>,
    stderr_rx: mpsc::Receiver<Captured>,
    reaped: bool,
}

impl Drop for Supervision {
    /// Backstop for a supervisor that never ran or unwound: the worker must
    /// not outlive its owner unsignalled.
    fn drop(&mut self) {
        if !self.reaped {
            self.kill_tree();
        }
    }
}

impl Supervision {
    fn run(&mut self) -> Result<CompileOutput, WorkerError> {
        loop {
            match self.has_exited() {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    self.shared.revoke(StopReason::Abandoned);
                    return Err(self.terminate(Stop::Lost(format!("wait for worker: {error}"))));
                }
            }
            let stop = {
                let mut stage = self.shared.stage();
                match *stage {
                    Stage::Revoked(reason) => Some(reason),
                    Stage::Running if Instant::now() >= self.deadline_at => {
                        *stage = Stage::Revoked(StopReason::Timeout);
                        Some(StopReason::Timeout)
                    }
                    _ => None,
                }
            };
            if let Some(reason) = stop {
                return Err(self.terminate(Stop::Revoked(reason)));
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        // The leader has exited but is not reaped, so its group id still
        // names only this worker's processes: stop any descendants first.
        self.kill_tree();
        let status = match self.child.wait() {
            Ok(status) => {
                self.reaped = true;
                status
            }
            Err(error) => {
                return Err(WorkerError::ShutdownIncomplete {
                    cause: format!("reap worker pid {}: {error}", self.identity.pid),
                    private_dir: self.private_dir.clone(),
                })
            }
        };
        let grace_end = Instant::now() + self.grace;
        let stdout = match self.stdout_rx.recv_timeout(self.grace) {
            Ok(captured) => captured,
            Err(_) => return Err(self.output_held_open("exited")),
        };
        let stderr = self
            .stderr_rx
            .recv_timeout(grace_end.saturating_duration_since(Instant::now()))
            .ok()
            .and_then(Result::ok)
            .map(|(bytes, _)| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let result = self.finish(status, stdout, stderr);
        let _ = std::fs::remove_dir_all(&self.private_dir);
        result
    }

    fn finish(
        &self,
        status: ExitStatus,
        stdout: Captured,
        stderr: String,
    ) -> Result<CompileOutput, WorkerError> {
        let failed = |detail: &str| WorkerError::WorkerFailed {
            status: format!("{status}{detail}"),
            stderr: stderr.clone(),
        };
        let (bytes, overflowed) = stdout.map_err(|error| failed(&format!("; stdout: {error}")))?;
        if overflowed {
            return Err(failed("; response exceeded the size limit"));
        }
        let Ok(response) = serde_json::from_slice::<WorkerResponse>(&bytes) else {
            return Err(failed("; no valid response"));
        };
        if response.protocol != PROTOCOL_VERSION
            || response.job_id != self.identity.job_id
            || response.nonce != self.identity.nonce
        {
            return Err(WorkerError::Rejected {
                reason: format!(
                    "identity mismatch: expected job {} with this job's nonce, got job {} \
                     (protocol {})",
                    self.identity.job_id, response.job_id, response.protocol
                ),
            });
        }
        match response.outcome {
            Outcome::Generated {
                files,
                graph,
                warnings,
            } if status.success() => {
                self.validate(&files)
                    .map_err(|reason| WorkerError::Rejected { reason })?;
                self.commit()?;
                let files = self.publish(&files)?;
                Ok(CompileOutput {
                    identity: self.identity.clone(),
                    files,
                    graph,
                    warnings,
                })
            }
            Outcome::CompileError { diagnostic } if status.code() == Some(1) => {
                Err(WorkerError::Compile { diagnostic })
            }
            _ => Err(failed("; exit status does not match the response")),
        }
    }

    fn validate(&self, files: &[ReportedFile]) -> Result<(), String> {
        let mut names = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        for file in files {
            let mut components = file.private.components();
            let name = match (components.next(), components.next(), components.next()) {
                (Some(Component::Normal(dir)), Some(Component::Normal(name)), None)
                    if dir == FILES_DIR =>
                {
                    name.to_owned()
                }
                _ => {
                    return Err(format!(
                        "{} is not inside the private directory",
                        file.private.display()
                    ))
                }
            };
            if !names.insert(name) {
                return Err(format!("{} reported twice", file.private.display()));
            }
            let destination = normalize(&file.destination);
            if !self.destinations.permits(&destination) {
                return Err(format!(
                    "{} is not a destination of this request",
                    file.destination.display()
                ));
            }
            if !destinations.insert(destination) {
                return Err(format!("{} reported twice", file.destination.display()));
            }
            let path = self.private_dir.join(&file.private);
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if !metadata.file_type().is_file() {
                return Err(format!("{} is not a regular file", path.display()));
            }
            if metadata.len() != file.size {
                return Err(format!(
                    "{} is {} bytes, reported {}",
                    path.display(),
                    metadata.len(),
                    file.size
                ));
            }
            let digest = hex::encode(Sha256::digest(
                std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
            ));
            if digest != file.sha256 {
                return Err(format!(
                    "{} digest does not match its report",
                    path.display()
                ));
            }
        }
        for entry in read_dir_names(&self.private_dir)? {
            if entry != FILES_DIR {
                return Err(format!(
                    "unreported entry {entry:?} in the private directory"
                ));
            }
        }
        let files_dir = self.private_dir.join(FILES_DIR);
        if std::fs::symlink_metadata(&files_dir).is_ok_and(|meta| meta.file_type().is_dir()) {
            for entry in read_dir_names(&files_dir)? {
                if !names.contains(std::ffi::OsStr::new(&entry)) {
                    return Err(format!(
                        "unreported file {entry:?} in the private directory"
                    ));
                }
            }
        } else if files_dir.symlink_metadata().is_ok() {
            return Err("the private files entry is not a directory".into());
        }
        Ok(())
    }

    fn commit(&self) -> Result<(), WorkerError> {
        let mut stage = self.shared.stage();
        match *stage {
            Stage::Running if Instant::now() >= self.deadline_at => {
                *stage = Stage::Revoked(StopReason::Timeout);
                Err(WorkerError::Timeout {
                    deadline: self.deadline,
                })
            }
            Stage::Running => {
                *stage = Stage::Committed;
                Ok(())
            }
            Stage::Revoked(reason) => Err(self.revoked_error(reason)),
            Stage::Committed => Ok(()),
        }
    }

    fn publish(&self, files: &[ReportedFile]) -> Result<Vec<PublishedFile>, WorkerError> {
        let mut published = Vec::with_capacity(files.len());
        for file in files {
            let destination = normalize(&file.destination);
            let error = |error| WorkerError::Publish {
                path: destination.clone(),
                error,
            };
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(error)?;
            }
            let staged = destination.with_file_name(format!(
                ".{}.{}.zfb-worker-tmp",
                destination
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                &self.identity.nonce[..16]
            ));
            let copied = std::fs::copy(self.private_dir.join(&file.private), &staged)
                .and_then(|_| std::fs::rename(&staged, &destination));
            if let Err(failure) = copied {
                let _ = std::fs::remove_file(&staged);
                return Err(error(failure));
            }
            published.push(PublishedFile {
                path: destination,
                size: file.size,
                sha256: file.sha256.clone(),
            });
        }
        Ok(published)
    }

    /// Revocation already happened; terminate the worker's process tree,
    /// confirm exit, then delete the private directory.
    fn terminate(&mut self, stop: Stop) -> WorkerError {
        self.kill_tree();
        let grace_end = Instant::now() + self.grace;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped = true;
                    break;
                }
                Ok(None) if Instant::now() < grace_end => std::thread::sleep(POLL_INTERVAL),
                Ok(None) => {
                    return WorkerError::ShutdownIncomplete {
                        cause: format!(
                            "{}, but worker pid {} did not exit within {:?} of being killed",
                            stop.describe(),
                            self.identity.pid,
                            self.grace
                        ),
                        private_dir: self.private_dir.clone(),
                    }
                }
                Err(error) => {
                    return WorkerError::ShutdownIncomplete {
                        cause: format!(
                            "{}, but reaping worker pid {} failed: {error}",
                            stop.describe(),
                            self.identity.pid
                        ),
                        private_dir: self.private_dir.clone(),
                    }
                }
            }
        }
        if let Err(RecvTimeoutError::Timeout) = self
            .stdout_rx
            .recv_timeout(grace_end.saturating_duration_since(Instant::now()))
        {
            return self.output_held_open(&stop.describe());
        }
        let _ = std::fs::remove_dir_all(&self.private_dir);
        match stop {
            Stop::Revoked(reason) => self.revoked_error(reason),
            Stop::Lost(cause) => WorkerError::WorkerFailed {
                status: cause,
                stderr: String::new(),
            },
        }
    }

    fn output_held_open(&self, context: &str) -> WorkerError {
        WorkerError::ShutdownIncomplete {
            cause: format!(
                "{context}, worker pid {} is reaped, but its output stream stayed open for {:?}; \
                 a descendant outside its process group may still be running",
                self.identity.pid, self.grace
            ),
            private_dir: self.private_dir.clone(),
        }
    }

    fn revoked_error(&self, reason: StopReason) -> WorkerError {
        match reason {
            StopReason::Timeout => WorkerError::Timeout {
                deadline: self.deadline,
            },
            StopReason::Cancelled | StopReason::Abandoned => WorkerError::Cancelled,
        }
    }

    #[cfg(unix)]
    fn has_exited(&mut self) -> std::io::Result<bool> {
        // WNOWAIT leaves the leader unreaped, so the process group id cannot
        // be reused before `kill_tree` signals it.
        loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.identity.pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if rc == 0 {
                return Ok(unsafe { info.si_pid() } != 0);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    #[cfg(not(unix))]
    fn has_exited(&mut self) -> std::io::Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    #[cfg(unix)]
    fn kill_tree(&mut self) {
        unsafe {
            libc::kill(-(self.identity.pid as libc::pid_t), libc::SIGKILL);
        }
    }

    /// Windows: only the worker itself is killed; descendants it started
    /// outside a job object can outlive it.
    #[cfg(not(unix))]
    fn kill_tree(&mut self) {
        let _ = self.child.kill();
    }
}

/// The caller-visible paths a request may produce, resolved against its cwd.
#[derive(Debug)]
struct Destinations {
    output_root: PathBuf,
    graph: Option<PathBuf>,
}

impl Destinations {
    fn of(request: &BundleRequest) -> Result<Self, WorkerError> {
        if !request.cwd.is_absolute() {
            return Err(WorkerError::Setup(format!(
                "request cwd {} must be absolute",
                request.cwd.display()
            )));
        }
        let resolve = |path: &Path| normalize(&request.cwd.join(path));
        let output_root = match &request.output {
            OutputLayout::File(file) => resolve(file)
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| WorkerError::Setup("output file has no parent".into()))?,
            OutputLayout::Dir { dir, .. } => resolve(dir),
        };
        Ok(Self {
            output_root,
            graph: request.graph.as_deref().map(resolve),
        })
    }

    fn permits(&self, destination: &Path) -> bool {
        destination.is_absolute()
            && (self.graph.as_deref() == Some(destination)
                || (destination.starts_with(&self.output_root) && destination != self.output_root))
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn read_dir_names(dir: &Path) -> Result<Vec<String>, String> {
    std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect()
        })
        .map_err(|error| format!("list {}: {error}", dir.display()))
}

fn read_capped(mut reader: impl Read, cap: usize) -> Captured {
    let mut kept = Vec::new();
    let mut overflowed = false;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => return Ok((kept, overflowed)),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let room = cap.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..read.min(room)]);
        overflowed |= read > room;
    }
}

fn create_private_dir(root: &Path, job_id: &str, nonce: &str) -> Result<PathBuf, WorkerError> {
    let setup = |what: &str, path: &Path, error: std::io::Error| {
        WorkerError::Setup(format!("{what} {}: {error}", path.display()))
    };
    std::fs::create_dir_all(root).map_err(|error| setup("create scratch root", root, error))?;
    let root = std::fs::canonicalize(root).map_err(|error| setup("resolve", root, error))?;
    let dir = root.join(format!("zfb-compiler-worker-{job_id}-{}", &nonce[..12]));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|error| setup("create private directory", &dir, error))?;
    Ok(dir)
}

fn next_job_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn random_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    (0..2u64)
        .map(|lane| {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u64(lane);
            hasher.write_u128(nanos);
            hasher.write_u32(std::process::id());
            format!("{:016x}", hasher.finish())
        })
        .collect()
}
