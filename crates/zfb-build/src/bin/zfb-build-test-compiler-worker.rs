//! Test-only compiler worker host for zfb-build's plugin bundling tests.
use std::process::ExitCode;

fn main() -> ExitCode {
    zfb_compiler_worker::run_if_requested().unwrap_or_else(|| {
        eprintln!("zfb-build-test-compiler-worker: started without the private worker argument");
        ExitCode::from(64)
    })
}
