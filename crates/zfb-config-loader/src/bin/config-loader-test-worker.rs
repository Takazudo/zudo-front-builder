//! Test-only compiler worker host for the config-loader tests: an embedding
//! host whose executable is not the zfb CLI.
use std::process::ExitCode;

fn main() -> ExitCode {
    zfb_compiler_worker::run_if_requested().unwrap_or_else(|| {
        eprintln!("config-loader-test-worker: started without the private worker argument");
        ExitCode::from(2)
    })
}
