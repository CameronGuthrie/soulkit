use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

pub use crate::{run_extract, ExtractArgs};

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Extract all locally installed music, dialogue, ambience and sound effects.
    Extract {
        /// Override the installed Cache.Windows directory.
        #[arg(long, alias = "cache-dir")]
        cache_path: Option<PathBuf>,
        /// Matching Soulframe.x64.exe for Vorbis headers (defaults to the cache's parent).
        #[arg(long)]
        game_exe: Option<PathBuf>,
        /// Destination for playable files and index.json.
        #[arg(short, long, default_value = "output/audio")]
        output: PathBuf,
        /// Only paths containing this text (case insensitive).
        #[arg(long)]
        path_filter: Option<String>,
        /// Maximum number of audio assets to export (0 = unlimited).
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// List matching paths without loading Oodle or extracting files.
        #[arg(long)]
        list_paths: bool,
        /// Rebuild exports even when their cache metadata has not changed.
        #[arg(long)]
        force: bool,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Extract {
            cache_path,
            game_exe,
            output,
            path_filter,
            limit,
            list_paths,
            force,
        } => run_extract(ExtractArgs {
            cache_path,
            game_exe,
            output,
            path_filter,
            limit,
            list_paths,
            force,
        }),
    }
}
