use clap::Parser;
use soulframe_models::cli::Cmd;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "soulframe-models", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

fn main() -> ExitCode {
    match soulframe_models::cli::run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
