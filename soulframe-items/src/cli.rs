//! CLI subcommands for `soulframe-items` (and `soulkit-cli items`).

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Build the unified Soulframe item catalogue (`all_items.json`).
    AllItems {
        /// Path to `packages_bin_attrs.json` (entity attrs dump from `H.Misc.cache`).
        #[arg(long, default_value = "workdir/packages_bin_attrs.json")]
        packages: PathBuf,
        /// Path to the English `Languages.json` produced by `soulframe-languages`.
        #[arg(long, default_value = "output/Languages/Languages.json")]
        languages: PathBuf,
        /// Path to `crafting_recipes.json` (recipe list).
        #[arg(long, default_value = "output/crafting_recipes.json")]
        recipes: PathBuf,
        /// Output file path.
        #[arg(short, long, default_value = "output/all_items.json")]
        output: PathBuf,
    },
    /// Build syndicate reward distributions (`reward_distributions.json`).
    RewardDistributions {
        /// Path to `manifest_rewards_raw.json` (`{manifest_path: plaintext}`).
        #[arg(long, default_value = "workdir/manifest_rewards_raw.json")]
        manifests: PathBuf,
        /// Output file path.
        #[arg(short, long, default_value = "output/reward_distributions.json")]
        output: PathBuf,
    },
    /// Convert `packages_bin_attrs.json` flat attribute strings into real nested JSON.
    FormatPackages {
        /// Path to `packages_bin_attrs.json`.
        #[arg(long, default_value = "workdir/packages_bin_attrs.json")]
        packages: PathBuf,
        /// Optional path to `drop_tables.json`; when present, parsed drop tables
        /// are embedded into matching package entries.
        #[arg(long)]
        drop_tables: Option<PathBuf>,
        /// Output file path.
        #[arg(short, long, default_value = "output/formatted_packages.json")]
        output: PathBuf,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::AllItems {
            packages,
            languages,
            recipes,
            output,
        } => crate::all_items::run(&packages, &languages, &recipes, &output),
        Cmd::RewardDistributions { manifests, output } => {
            crate::reward_distributions::run(&manifests, &output)
        }
        Cmd::FormatPackages {
            packages,
            drop_tables,
            output,
        } => {
            crate::format_packages::run_with_drop_tables(&packages, &output, drop_tables.as_deref())
        }
    }
}
