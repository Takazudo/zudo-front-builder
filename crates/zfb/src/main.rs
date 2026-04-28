use clap::Parser;

use zfb::cli::{Cli, Command};
use zfb::commands;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::New(args) => commands::new::run(args).await,
        Command::Dev(args) => commands::dev::run(args).await,
        Command::Build(args) => commands::build::run(args).await,
        Command::Preview(args) => commands::preview::run(args).await,
        Command::Check(args) => commands::check::run(args).await,
    };
    if let Err(e) = result {
        // Single, centralized error rendering so individual commands can
        // just `return Err(e)` without pre-printing — avoids the
        // double-print that would otherwise happen when a command both
        // formats the error itself and returns it for main to log again.
        zfb::report_error(&e);
        std::process::exit(1);
    }
}
