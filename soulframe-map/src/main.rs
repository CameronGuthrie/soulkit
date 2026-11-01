use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "soulframe-map",
    version,
    about = "Extract every tiled level map and Midrath map JSON from the Soulframe cache."
)]
struct Cli {
    #[command(subcommand)]
    command: soulframe_map::cli::Cmd,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    soulframe_map::cli::run(cli.command)
}
