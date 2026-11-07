use clap::Parser;

use zfb::cli::{Cli, Command};
use zfb::commands;

fn main() -> std::process::ExitCode {
    // Must stay first: the compiler worker mode initializes nothing else.
    if let Some(code) = zfb_compiler_worker::run_if_requested() {
        return code;
    }
    #[cfg(feature = "direct-v8")]
    if let Err(error) = zfb_render::initialize_v8() {
        eprintln!("V8 initialization failed: {error}");
        std::process::exit(1);
    }
    run();
    std::process::ExitCode::SUCCESS
}

#[tokio::main]
async fn run() {
    let cli = Cli::parse();
    if let Err(e) = zfb::reject_removed_bundler_selectors(|name| std::env::var_os(name)) {
        zfb::report_error(&e);
        std::process::exit(1);
    }
    let result = match &cli.command {
        Command::New(args) => commands::new::run(args).await,
        Command::Dev(args) => commands::dev::run(args).await,
        Command::Build(args) => commands::build::run(args).await,
        Command::Css(args) => commands::css::run(args).await,
        Command::Wind(args) => commands::wind::run(args).await,
        Command::Preview(args) => commands::preview::run(args).await,
        Command::Check(args) => commands::check::run(args).await,
    };
    if let Err(e) = result {
        if let Some(exit) = e.downcast_ref::<zfb::SilentExit>() {
            std::process::exit(exit.0);
        }
        // Single, centralized error rendering so individual commands can
        // just `return Err(e)` without pre-printing — avoids the
        // double-print that would otherwise happen when a command both
        // formats the error itself and returns it for main to log again.
        zfb::report_error(&e);
        std::process::exit(1);
    }
}
