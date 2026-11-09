//! Test-only compiler worker host for the embed tests: an embedding host
//! whose executable is not the zfb CLI.
use std::process::ExitCode;

fn main() -> ExitCode {
    zfb_server::run_compiler_worker_if_requested().unwrap_or_else(|| {
        eprintln!("zfb-server-test-worker: started without the private worker argument");
        ExitCode::from(2)
    })
}
