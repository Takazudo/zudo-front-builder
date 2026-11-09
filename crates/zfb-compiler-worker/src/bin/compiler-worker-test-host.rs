//! Test-only embedding host: proves a non-zfb executable can host the worker.
use std::process::ExitCode;

fn main() -> ExitCode {
    zfb_compiler_worker::run_if_requested().unwrap_or_else(|| {
        eprintln!("compiler-worker-test-host: started without the private worker argument");
        ExitCode::from(64)
    })
}
