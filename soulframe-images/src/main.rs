use anyhow::Result;
use clap::Parser;
use soulframe_images::cli::Cmd;

#[derive(Parser, Debug)]
#[command(
    name = "soulframe-images",
    about = "Soulframe image / texture extractor"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    soulframe_images::cli::run(cli.command)
}
