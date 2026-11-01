//! Thin `soulframe-languages` binary — pipeline lives in
//! [`soulframe_languages::cli`].

use anyhow::Result;
use clap::Parser;
use soulframe_languages::cli::Cmd;

#[derive(Parser, Debug)]
#[command(name = "soulframe-languages", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

fn main() -> Result<()> {
    soulframe_languages::cli::run(Cli::parse().command)
}
