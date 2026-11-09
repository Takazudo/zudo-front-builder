//! The stdio wire format between parent and worker. One JSON request on
//! stdin, one JSON response on stdout.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zfb_bundler::BundleRequest;

pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// Test hooks run only when the worker's own environment sets this to `1`
/// and the request carries a hook; production requests never carry one.
pub const TEST_HOOKS_ENV: &str = "ZFB_COMPILER_WORKER_TEST_HOOKS";

/// Subdirectory of the private directory that holds reported files.
pub(crate) const FILES_DIR: &str = "files";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WorkerRequest {
    pub protocol: u32,
    pub job_id: String,
    pub nonce: String,
    pub private_dir: PathBuf,
    pub request: BundleRequest,
    pub test_hook: Option<TestHook>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WorkerResponse {
    pub protocol: u32,
    pub job_id: String,
    pub nonce: String,
    pub outcome: Outcome,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Outcome {
    Generated {
        files: Vec<ReportedFile>,
        graph: serde_json::Value,
        warnings: Vec<String>,
    },
    CompileError {
        diagnostic: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReportedFile {
    /// Where the parent should publish the file: the path `generate` produced
    /// for the original request.
    pub destination: PathBuf,
    /// Location inside the private directory, relative to it.
    pub private: PathBuf,
    pub size: u64,
    pub sha256: String,
}

/// Misbehaviors the lifecycle tests inject into a real worker process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestHook {
    /// Sleep before generating, like a generation that never returns.
    HangBeforeGenerate { millis: u64 },
    /// Write files and the response, then sleep instead of exiting.
    HangAfterResponse { millis: u64 },
    /// Spawn a `sleep` descendant in the worker's process group, record its
    /// pid, then hang.
    SpawnDescendant { pid_file: PathBuf, millis: u64 },
    /// Spawn a `sleep` descendant in its own process group that keeps the
    /// worker's stdout open, record its pid, then hang.
    SpawnEscapedDescendant { pid_file: PathBuf, millis: u64 },
    /// Write the first file outside the private directory and report it there.
    EscapePrivateDir,
    /// Report a wrong digest for the first file.
    MisreportDigest,
    /// Report the first file for a destination the request does not own.
    ForeignDestination { destination: PathBuf },
    /// Leave an extra file in the private directory.
    UnreportedFile,
    /// Answer with a different identity nonce.
    WrongNonce,
}
