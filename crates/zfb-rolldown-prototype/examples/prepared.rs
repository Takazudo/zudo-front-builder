//! Minimal native capability/reproduction runner: arguments are a prepared ZFB job.
#[cfg(feature = "native")]
fn main() -> anyhow::Result<()> {
    let mut job = std::process::Command::new("unused-esbuild");
    job.current_dir(std::env::current_dir()?)
        .args(std::env::args_os().skip(1));
    zfb_rolldown_prototype::run_prepared(&job)
}
#[cfg(not(feature = "native"))]
fn main() {
    panic!("enable native feature");
}
