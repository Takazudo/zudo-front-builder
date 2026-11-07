//! The worker side: one request in, private files and one response out.
use crate::launcher::PRIVATE_ARG;
use crate::protocol::{
    Outcome, ReportedFile, TestHook, WorkerRequest, WorkerResponse, FILES_DIR, PROTOCOL_VERSION,
    TEST_HOOKS_ENV,
};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;
const EXIT_COMPILE_ERROR: u8 = 1;
const EXIT_PROTOCOL_ERROR: u8 = 2;

/// Serves one worker job when this process was started as
/// `<host> __zfb-compiler-worker`; returns `None` otherwise. Call it first in
/// `main`, before parsing arguments or starting any runtime.
pub fn run_if_requested() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(PRIVATE_ARG)) {
        return None;
    }
    if args.next().is_some() {
        eprintln!("zfb compiler worker: {PRIVATE_ARG} takes no further arguments");
        return Some(ExitCode::from(EXIT_PROTOCOL_ERROR));
    }
    Some(serve_stdio())
}

/// Runs one compiler job from stdin to stdout. Initializes nothing beyond the
/// native bundler: no JS engine, server or plugin host.
pub fn serve_stdio() -> ExitCode {
    match serve() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("zfb compiler worker: {message}");
            ExitCode::from(EXIT_PROTOCOL_ERROR)
        }
    }
}

fn serve() -> Result<ExitCode, String> {
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|error| format!("read request: {error}"))?;
    if input.len() as u64 > MAX_REQUEST_BYTES {
        return Err(format!("request exceeds {MAX_REQUEST_BYTES} bytes"));
    }
    let request: WorkerRequest =
        serde_json::from_slice(&input).map_err(|error| format!("decode request: {error}"))?;
    if request.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "protocol {} requested, this worker speaks {PROTOCOL_VERSION}",
            request.protocol
        ));
    }
    let hook = match request.test_hook.clone() {
        Some(_) if std::env::var_os(TEST_HOOKS_ENV).as_deref() != Some(OsStr::new("1")) => {
            return Err(format!(
                "test hooks are disabled ({TEST_HOOKS_ENV} is not 1)"
            ));
        }
        hook => hook,
    };
    let private_dir = &request.private_dir;
    if !private_dir.is_absolute() || !private_dir.is_dir() {
        return Err(format!(
            "private directory {} is not an existing absolute directory",
            private_dir.display()
        ));
    }
    match &hook {
        Some(TestHook::HangBeforeGenerate { millis }) => sleep(*millis),
        Some(TestHook::SpawnDescendant { pid_file, millis }) => {
            spawn_descendant(pid_file, false)?;
            sleep(*millis);
        }
        Some(TestHook::SpawnEscapedDescendant { pid_file, millis }) => {
            spawn_descendant(pid_file, true)?;
            sleep(*millis);
        }
        _ => {}
    }

    let (outcome, code) = match zfb_bundler::generate(&request.request) {
        Ok(generated) => (
            Outcome::Generated {
                files: write_private_files(private_dir, &generated.files, hook.as_ref())?,
                graph: generated.graph,
                warnings: generated.warnings,
            },
            ExitCode::SUCCESS,
        ),
        Err(error) => (
            Outcome::CompileError {
                diagnostic: format!("{error:#}"),
            },
            ExitCode::from(EXIT_COMPILE_ERROR),
        ),
    };
    let nonce = if hook == Some(TestHook::WrongNonce) {
        format!("{}-forged", request.nonce)
    } else {
        request.nonce
    };
    let response = WorkerResponse {
        protocol: PROTOCOL_VERSION,
        job_id: request.job_id,
        nonce,
        outcome,
    };
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &response)
        .map_err(|error| format!("write response: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush response: {error}"))?;
    drop(stdout);
    if let Some(TestHook::HangAfterResponse { millis }) = hook {
        sleep(millis);
    }
    Ok(code)
}

fn write_private_files(
    private_dir: &Path,
    files: &[(PathBuf, Vec<u8>)],
    hook: Option<&TestHook>,
) -> Result<Vec<ReportedFile>, String> {
    let files_dir = private_dir.join(FILES_DIR);
    std::fs::create_dir_all(&files_dir)
        .map_err(|error| format!("create {}: {error}", files_dir.display()))?;
    let mut reported = Vec::with_capacity(files.len());
    for (index, (destination, bytes)) in files.iter().enumerate() {
        let mut private = Path::new(FILES_DIR).join(index.to_string());
        if index == 0 && hook == Some(&TestHook::EscapePrivateDir) {
            let name = private_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            private = PathBuf::from(format!("../{name}-escaped"));
        }
        let path = private_dir.join(&private);
        std::fs::write(&path, bytes)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        let mut sha256 = hex::encode(Sha256::digest(bytes));
        let mut destination = destination.clone();
        if index == 0 {
            match hook {
                Some(TestHook::MisreportDigest) => sha256 = hex::encode(Sha256::digest(b"other")),
                Some(TestHook::ForeignDestination {
                    destination: foreign,
                }) => {
                    destination = foreign.clone();
                }
                _ => {}
            }
        }
        reported.push(ReportedFile {
            destination,
            private,
            size: bytes.len() as u64,
            sha256,
        });
    }
    if hook == Some(&TestHook::UnreportedFile) {
        let path = files_dir.join("unreported");
        std::fs::write(&path, b"unreported")
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(reported)
}

fn spawn_descendant(pid_file: &Path, own_group: bool) -> Result<(), String> {
    let mut command = std::process::Command::new("sleep");
    command.arg("30").stdin(std::process::Stdio::null());
    #[cfg(unix)]
    if own_group {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = own_group;
    let child = command
        .spawn()
        .map_err(|error| format!("spawn descendant: {error}"))?;
    std::fs::write(pid_file, child.id().to_string())
        .map_err(|error| format!("write {}: {error}", pid_file.display()))?;
    drop(child);
    Ok(())
}

fn sleep(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}
