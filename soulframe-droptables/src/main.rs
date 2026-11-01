//! Thin `soulframe-droptables` binary — see [`soulframe_droptables`] for the
//! pipeline implementation.

use anyhow::Result;
use clap::Parser;
use soulframe_droptables::Cmd;

#[derive(Parser, Debug)]
#[command(name = "soulframe-droptables", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

fn main() -> Result<()> {
    soulframe_droptables::run(Cli::parse().command)
}
