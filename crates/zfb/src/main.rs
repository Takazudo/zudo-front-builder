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
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
