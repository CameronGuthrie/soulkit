use anyhow::Result;
use clap::Parser;
use soulframe_items::cli::Cmd;

#[derive(Parser, Debug)]
#[command(
    name = "soulframe-items",
    about = "Soulframe item catalogue & syndicate reward pipelines"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    soulframe_items::cli::run(cli.command)
}
