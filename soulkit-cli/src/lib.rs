//! `soulkit-cli` — unified Soulframe pipeline binary.
//!
//! Wraps the per-tool crates (`soulframe-languages`, `soulframe-droptables`,
//! `soulframe-images`, `soulframe-audio`) as nested subcommands so the whole pipeline is
//! reachable from a single executable:
//!
//! ```text
//! soulkit-cli languages download
//! soulkit-cli languages extract
//! soulkit-cli droptables run-all
//! soulkit-cli images extract --filter png --path-filter "SF/Interface/"
//! soulkit-cli audio extract
//! soulkit-cli models extract --path-filter "SF/Objects/"
//! soulkit-cli map extract
//! soulkit-cli items all-items
//! soulkit-cli items reward-distributions
//! soulkit-cli zenith-offers
//! soulkit-cli zenith-calendar --from 2026-08-17 --days 30
//! soulkit-cli gods-and-ghosts-calendar --from 2026-08-17 --days 30
//! soulkit-cli weather-calendar --from 2026-08-17 --days 7   # -> weather/{weather-calendar,weather-guide}.json
//! soulkit-cli run-all
//! ```

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod de_rng;
mod epithets;
mod gods_and_ghosts;
mod loot_data;
mod loot_resolved;
mod loot_resource_inputs;
mod loot_script_sources;
mod loot_sources;
mod packages_bin;
mod pipeline_workspace;
mod weather;
mod zenith;

const CALENDAR_DAYS_EITHER_SIDE: u32 = 90;
const DEFAULT_CALENDAR_DAYS: u32 = CALENDAR_DAYS_EITHER_SIDE * 2 + 1;

#[derive(Parser, Debug)]
#[command(
    name = "soulkit-cli",
    version,
    about = "Unified Soulframe pipeline runner."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<TopCmd>,
}

impl Cli {
    fn into_command(self) -> TopCmd {
        self.command.unwrap_or_else(|| {
            println!("(no subcommand given — defaulting to `soulkit-cli run-all` with defaults)");
            // Use the same Clap defaults as an explicit `run-all` invocation.
            Self::parse_from(["soulkit-cli", "run-all"])
                .command
                .expect("run-all is a valid subcommand")
        })
    }
}

#[derive(Subcommand, Debug)]
enum TopCmd {
    /// Language pack downloader / extractor.
    Languages {
        #[command(subcommand)]
        cmd: soulframe_languages::cli::Cmd,
    },
    /// Drop-table extraction pipeline.
    Droptables {
        #[command(subcommand)]
        cmd: soulframe_droptables::Cmd,
    },
    /// Image / texture extractor (H+B DDS reconstruction, optional PNG conversion).
    Images {
        #[command(subcommand)]
        cmd: soulframe_images::cli::Cmd,
    },
    /// Voice, music, ambience and sound-effect extractor.
    Audio {
        #[command(subcommand)]
        cmd: soulframe_audio::cli::Cmd,
    },
    /// Static mesh exporter (glTF binary).
    Models {
        #[command(subcommand)]
        cmd: soulframe_models::cli::Cmd,
    },
    /// Tiled level map + Midrath map JSON extractor.
    Map {
        #[command(subcommand)]
        cmd: soulframe_map::cli::Cmd,
    },
    /// Item catalogue & syndicate reward pipelines.
    Items {
        #[command(subcommand)]
        cmd: soulframe_items::cli::Cmd,
    },
    /// Export level-aware drop-table data from existing inputs.
    ResolveLoot(loot_resolved::Args),
    /// Reconstruct Zenith's deterministic daily weapon offers.
    ZenithOffers {
        /// `formatted_packages.json` produced by `soulkit-cli items format-packages`.
        #[arg(long, default_value = "output/formatted_packages.json")]
        packages: std::path::PathBuf,
        /// Optional `market_items.json` used for localized display names.
        /// Defaults to the file beside `formatted_packages.json` when present.
        #[arg(long)]
        market_items: Option<std::path::PathBuf>,
        /// A worldstate/snapshot JSON containing `server_time` or `Time`.
        #[arg(long, conflicts_with = "unix_time")]
        worldstate: Option<std::path::PathBuf>,
        /// Unix timestamp to reconstruct; defaults to the current clock.
        #[arg(long)]
        unix_time: Option<u64>,
        /// Emit machine-readable JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Generate a calendar of deterministic future Zenith weapon offers.
    ZenithCalendar {
        /// `formatted_packages.json` produced by `soulkit-cli items format-packages`.
        #[arg(long, default_value = "output/formatted_packages.json")]
        packages: std::path::PathBuf,
        /// Optional `market_items.json` used for localized display names.
        /// Defaults to the file beside `formatted_packages.json` when present.
        #[arg(long)]
        market_items: Option<std::path::PathBuf>,
        /// First UTC rotation date, formatted as YYYY-MM-DD.
        #[arg(long)]
        from: String,
        /// Number of consecutive UTC days to generate.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=10_000))]
        days: u32,
        /// Destination JSON file.
        #[arg(long, default_value = "zenith-calendar.json")]
        output: std::path::PathBuf,
    },
    /// Generate the date-seeded Gods and Ghosts ancestor rotation.
    #[command(name = "gods-and-ghosts-calendar", alias = "gods-ghosts-calendar")]
    GodsAndGhostsCalendar {
        /// `ancestors.json` produced by `soulkit-cli map extract`.
        #[arg(long, default_value = "output/maps/map-json/ancestors.json")]
        ancestors: std::path::PathBuf,
        /// First UTC rotation date, formatted as YYYY-MM-DD.
        #[arg(long)]
        from: String,
        /// Number of consecutive UTC days to generate.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=10_000))]
        days: u32,
        /// Generate every unlock pool, assuming Zenith and Moose are always unlocked.
        #[arg(long, default_value_t = false)]
        all_unlock_variations: bool,
        /// Destination JSON file, or destination directory with --all-unlock-variations.
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Generate a calendar of Midrath's deterministic dynamic weather.
    WeatherCalendar {
        /// `formatted_packages.json` produced by `soulkit-cli items format-packages`.
        #[arg(long, default_value = "output/formatted_packages.json")]
        packages: std::path::PathBuf,
        /// First UTC date, formatted as YYYY-MM-DD.
        #[arg(long)]
        from: String,
        /// Number of consecutive UTC days to generate.
        #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=10_000))]
        days: u32,
        /// Destination calendar JSON; `weather-guide.json` is written beside it.
        #[arg(long, default_value = "weather/weather-calendar.json")]
        output: std::path::PathBuf,
    },
    /// Run the complete extraction pipeline and generate both daily calendars.
    RunAll {
        /// Comma-separated locales for the language pipeline.
        #[arg(long, default_value = soulframe_languages::cli::DEFAULT_LOCALES)]
        locales: String,
        /// Working directory for droptables outputs.
        #[arg(long, default_value = "output")]
        droptables_output: std::path::PathBuf,
        /// Directory for captured Camellia decryption keys (sensitive, kept
        /// out of droptables_output).
        #[arg(long)]
        keys_output: Option<std::path::PathBuf>,
        /// Directory for decrypted raw `.bin` manifest dumps (kept out of
        /// droptables_output).
        #[arg(long)]
        decrypted_output: Option<std::path::PathBuf>,
        /// Output directory for the image/SWF stage (`SF/Interface/`, `Temp/SWF/`, and Interface SWFs).
        #[arg(long, default_value = "output/images")]
        images_output: std::path::PathBuf,
        /// Output directory for voice, music, ambience and sound effects.
        #[arg(long, default_value = "output/audio")]
        audio_output: std::path::PathBuf,
        /// Output directory for the map stage (<levelKey>/ tiles and map-json/).
        #[arg(long, default_value = "output/maps")]
        map_output: std::path::PathBuf,
        /// `packages_bin_attrs.json` for the items stage (also published to the
        /// data repo so consumers can build an entity-icon index from it).
        #[arg(long, default_value = "output/packages_bin_attrs.json")]
        items_packages: std::path::PathBuf,
        /// `Languages.json` for the items stage.
        #[arg(long, default_value = "output/Languages/Languages.json")]
        items_languages: std::path::PathBuf,
        /// `crafting_recipes.json` for the items stage.
        #[arg(long, default_value = "output/crafting_recipes.json")]
        items_recipes: std::path::PathBuf,
        /// `manifest_rewards_raw.json` for the reward-distributions stage.
        #[arg(long)]
        items_manifests: Option<std::path::PathBuf>,
        /// Output directory for items JSON (all_items.json, reward_distributions.json).
        #[arg(long, default_value = "output")]
        items_output: std::path::PathBuf,
        /// Output directory for the generated daily calendars.
        #[arg(long, default_value = "output")]
        calendars_output: std::path::PathBuf,
        /// Files packed as-is into the Avakot API zip: epithets.json,
        /// ava_api_items.json and drop_tables.json.
        #[arg(long, default_value = "output/Artifacts")]
        artifacts_output: std::path::PathBuf,
        /// First UTC calendar date (YYYY-MM-DD); defaults to 90 days before the run date in UTC.
        #[arg(long)]
        calendar_from: Option<String>,
        /// Number of consecutive UTC days; defaults to 90 days either side of the run date, inclusive.
        #[arg(long, default_value_t = DEFAULT_CALENDAR_DAYS, value_parser = clap::value_parser!(u32).range(1..=10_000))]
        calendar_days: u32,
        /// Skip the images stage.
        #[arg(long, default_value_t = false)]
        skip_images: bool,
        /// Skip audio extraction.
        #[arg(long, default_value_t = false)]
        skip_audio: bool,
        /// Skip the map stage.
        #[arg(long, default_value_t = false)]
        skip_map: bool,
        /// Skip the items stage (all-items + reward-distributions).
        #[arg(long, default_value_t = false)]
        skip_items: bool,
        /// Skip Zenith and Gods and Ghosts calendar generation.
        #[arg(long, default_value_t = false)]
        skip_calendars: bool,
        /// Skip the drop-table stage (never launches the game).
        #[arg(long, default_value_t = false)]
        skip_droptables: bool,
        /// Skip downloading and extracting language packs.
        #[arg(long)]
        skip_languages: bool,
        /// Skip resolved-loot exports and their private binary inputs.
        #[arg(long)]
        skip_resolved_loot: bool,
        /// Keep the generated scratch directory after a successful run.
        #[arg(long)]
        keep_intermediates: bool,
    },
}

/// Execute the shared extraction tools with command-line style arguments.
///
/// The first argument is the program name, just as with `std::env::args_os()`.
/// Help and argument errors use Clap's normal exit behavior. The desktop app
/// calls this only in its isolated worker process, before starting the GUI.
pub fn run_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let command = Cli::parse_from(args).into_command();
    match command {
        TopCmd::Languages { cmd } => soulframe_languages::cli::run(cmd),
        TopCmd::Droptables { cmd } => soulframe_droptables::run(cmd),
        TopCmd::Images { cmd } => soulframe_images::cli::run(cmd),
        TopCmd::Audio { cmd } => soulframe_audio::cli::run(cmd),
        TopCmd::Models { cmd } => soulframe_models::cli::run(cmd),
        TopCmd::Map { cmd } => soulframe_map::cli::run(cmd),
        TopCmd::Items { cmd } => soulframe_items::cli::run(cmd),
        TopCmd::ResolveLoot(args) => loot_resolved::run(args),
        TopCmd::ZenithOffers {
            packages,
            market_items,
            worldstate,
            unix_time,
            json,
        } => zenith::run(
            &packages,
            market_items.as_deref(),
            worldstate.as_deref(),
            unix_time,
            json,
        ),
        TopCmd::ZenithCalendar {
            packages,
            market_items,
            from,
            days,
            output,
        } => zenith::run_calendar(&packages, market_items.as_deref(), &from, days, &output),
        TopCmd::WeatherCalendar {
            packages,
            from,
            days,
            output,
        } => weather::run_calendar(&packages, &from, days, &output),
        TopCmd::GodsAndGhostsCalendar {
            ancestors,
            from,
            days,
            all_unlock_variations,
            output,
        } => {
            let output = output.unwrap_or_else(|| {
                if all_unlock_variations {
                    std::path::PathBuf::from("gods-and-ghosts-calendars")
                } else {
                    std::path::PathBuf::from("gods-and-ghosts-calendar.json")
                }
            });
            if all_unlock_variations {
                gods_and_ghosts::run_all_unlock_variations(&ancestors, &from, days, &output)
            } else {
                gods_and_ghosts::run_calendar(&ancestors, &from, days, &output)
            }
        }
        TopCmd::RunAll {
            locales,
            droptables_output,
            keys_output,
            decrypted_output,
            images_output,
            audio_output,
            map_output,
            items_packages,
            items_languages,
            items_recipes,
            items_manifests,
            items_output,
            calendars_output,
            artifacts_output,
            calendar_from,
            calendar_days,
            skip_images,
            skip_audio,
            skip_map,
            skip_items,
            skip_calendars,
            skip_droptables,
            skip_languages,
            skip_resolved_loot,
            keep_intermediates,
        } => run_all(RunAllArgs {
            locales,
            droptables_output,
            keys_output,
            decrypted_output,
            images_output,
            audio_output,
            map_output,
            items_packages,
            items_languages,
            items_recipes,
            items_manifests,
            items_output,
            calendars_output,
            artifacts_output,
            calendar_from,
            calendar_days,
            skip_images,
            skip_audio,
            skip_map,
            skip_items,
            skip_calendars,
            skip_droptables,
            skip_languages,
            skip_resolved_loot,
            keep_intermediates,
        }),
    }
}

struct RunAllArgs {
    locales: String,
    droptables_output: std::path::PathBuf,
    keys_output: Option<std::path::PathBuf>,
    decrypted_output: Option<std::path::PathBuf>,
    images_output: std::path::PathBuf,
    audio_output: std::path::PathBuf,
    map_output: std::path::PathBuf,
    items_packages: std::path::PathBuf,
    items_languages: std::path::PathBuf,
    items_recipes: std::path::PathBuf,
    items_manifests: Option<std::path::PathBuf>,
    items_output: std::path::PathBuf,
    calendars_output: std::path::PathBuf,
    artifacts_output: std::path::PathBuf,
    calendar_from: Option<String>,
    calendar_days: u32,
    skip_images: bool,
    skip_audio: bool,
    skip_map: bool,
    skip_items: bool,
    skip_calendars: bool,
    skip_droptables: bool,
    skip_languages: bool,
    skip_resolved_loot: bool,
    keep_intermediates: bool,
}

fn pipeline_calendar_from(from: Option<String>, run_date: chrono::NaiveDate) -> String {
    from.unwrap_or_else(|| {
        (run_date - chrono::Days::new(u64::from(CALENDAR_DAYS_EITHER_SIDE)))
            .format("%Y-%m-%d")
            .to_string()
    })
}

fn run_all(args: RunAllArgs) -> Result<()> {
    let workspace = pipeline_workspace::PipelineWorkspace::new(&std::env::current_dir()?)?;
    let keep = args.keep_intermediates;
    let result = run_all_in(args, &workspace);
    let cleanup = workspace.finish(result.is_ok(), keep);
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("Scratch cleanup also failed: {cleanup:#}")))
        }
        (Err(error), _) => Err(error),
        (Ok(()), result) => result,
    }
}

fn run_all_in(args: RunAllArgs, workspace: &pipeline_workspace::PipelineWorkspace) -> Result<()> {
    let RunAllArgs {
        locales,
        droptables_output,
        keys_output,
        decrypted_output,
        images_output,
        audio_output,
        map_output,
        items_packages,
        items_languages,
        items_recipes,
        items_manifests,
        items_output,
        calendars_output,
        artifacts_output,
        calendar_from,
        calendar_days,
        skip_images,
        skip_audio,
        skip_map,
        skip_items,
        skip_calendars,
        skip_droptables,
        skip_languages,
        skip_resolved_loot,
        keep_intermediates: _,
    } = args;

    let mut retained = vec![
        &droptables_output,
        &images_output,
        &audio_output,
        &map_output,
        &items_packages,
        &items_languages,
        &items_recipes,
        &items_output,
        &calendars_output,
        &artifacts_output,
    ];
    retained.extend(keys_output.iter());
    retained.extend(decrypted_output.iter());
    retained.extend(items_manifests.iter());
    workspace.ensure_retained_paths(&retained.iter().map(|p| p.as_path()).collect::<Vec<_>>())?;
    let reuse_decrypted = decrypted_output.is_some();
    let seed_keys = keys_output.is_none();
    let keys_output = keys_output.unwrap_or_else(|| workspace.root().join("keys"));
    let decrypted_output =
        decrypted_output.unwrap_or_else(|| workspace.root().join("decrypted_raw"));
    let items_manifests =
        items_manifests.unwrap_or_else(|| workspace.root().join("manifest_rewards_raw.json"));
    if seed_keys {
        let cached_keys = std::env::current_dir()?.join("keys/camellia_keys.txt");
        if cached_keys.is_file() {
            std::fs::create_dir_all(&keys_output)?;
            std::fs::copy(cached_keys, keys_output.join("camellia_keys.txt"))?;
        }
    }
    let downloads = workspace.root().join("downloaded-data");
    let extracted = workspace.root().join("extracted-data");
    let language_output = items_languages
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut failures = Vec::new();
    let parsed_locales = soulframe_languages::cli::parse_locales(&locales);
    // Resolve the shared window before extraction, even if the run crosses UTC midnight.
    let calendar_from = pipeline_calendar_from(calendar_from, chrono::Utc::now().date_naive());
    let calendar_stages = if skip_calendars {
        0
    } else {
        (!skip_map as usize) + 2 * (!skip_items as usize)
    };
    let total_stages = 2 * (!skip_languages as usize)
        + (!skip_resolved_loot as usize)
        + (!skip_droptables as usize)
        + (!skip_images as usize)
        + (!skip_audio as usize)
        + (!skip_map as usize)
        + (if skip_items { 0 } else { 6 })
        + calendar_stages;
    let mut stage = 0;
    let mut header = |title: &str| {
        stage += 1;
        println!("\n############################################");
        println!("# soulkit-cli run-all — stage {stage}/{total_stages}: {title}");
        println!("############################################");
    };

    if !skip_languages {
        header("languages download");
        if let Err(e) = soulframe_languages::cli::run_download_to(&parsed_locales, &downloads) {
            eprintln!("[!] languages download failed: {e:#}");
            failures.push("languages download");
        }

        header("languages extract");
        if let Err(e) = soulframe_languages::cli::run_extract_to(
            &parsed_locales,
            &downloads,
            &extracted,
            language_output,
        ) {
            eprintln!("[!] languages extract failed: {e:#}");
            failures.push("languages extract");
        }
    }

    let mut packages_attrs_for_droptables =
        items_packages.exists().then_some(items_packages.clone());

    if !skip_items {
        // Materialise the three intermediate JSONs the items pipeline reads
        // (packages_bin_attrs.json, crafting_recipes.json, manifest_rewards_raw.json)
        // before parsing drop tables so the droptable output can include
        // per-pickup quantity ranges.
        header("items inputs (packages.bin, recipes, manifests)");
        packages_bin::generate_items_inputs(&items_packages, &items_recipes, &items_manifests)
            .context("items input extraction failed; refusing to reuse stale package data")?;
        packages_attrs_for_droptables = Some(items_packages.clone());

        header("items format-packages");
        let fmt_cmd = soulframe_items::cli::Cmd::FormatPackages {
            packages: items_packages.clone(),
            drop_tables: None,
            output: items_output.join("formatted_packages.json"),
        };
        soulframe_items::cli::run(fmt_cmd).context(
            "formatting current packages failed; stopping to avoid reusing stale formatted data",
        )?;
    }

    if !skip_items || !skip_droptables {
        std::fs::create_dir_all(&artifacts_output)?;
    }
    let raw_drop_tables_out = droptables_output
        .join("drop_tables")
        .join("raw_drop_tables.json");
    let dt_cmd = soulframe_droptables::Cmd::RunAll {
        exe: None,
        output_dir: droptables_output,
        keys_dir: keys_output.clone(),
        decrypted_dir: decrypted_output.clone(),
        packages_attrs: packages_attrs_for_droptables,
        timeout: 600,
        capture: false,
    };
    let mut drop_extraction_succeeded = false;
    if !skip_droptables {
        header("droptables run-all");
        match soulframe_droptables::run(dt_cmd) {
            Err(e) => {
                eprintln!("[!] droptables run-all failed: {e:#}");
                failures.push("droptables");
            }
            Ok(()) => {
                drop_extraction_succeeded = true;
                println!(
                    "  [droptables] raw/default JSON: {}",
                    raw_drop_tables_out.display()
                );
            }
        }
    }
    if !skip_resolved_loot && skip_droptables && !reuse_decrypted {
        // Resolved loot still needs binary inputs. Regenerate these privately
        // without capture, so skip mode also works after successful cleanup.
        let key_file = keys_output.join("camellia_keys.txt");
        soulframe_droptables::run(soulframe_droptables::Cmd::Decrypt {
            key_source: key_file.is_file().then(|| key_file.to_string_lossy().into_owned()),
            output: decrypted_output.clone(),
            cache_dir: None,
        }).context("regenerating resolved-loot inputs without launching the game; use --decrypted-output to supply an existing snapshot")?;
    }

    if !skip_items {
        header("items format-packages + droptables");
        // Skipped drop tables: reuse the last run's copy, already moved into artifacts.
        let drop_tables_json = if skip_droptables {
            artifacts_output.join("drop_tables.json")
        } else {
            raw_drop_tables_out.with_file_name("drop_tables.json")
        };
        let fmt_cmd = soulframe_items::cli::Cmd::FormatPackages {
            packages: items_packages.clone(),
            drop_tables: Some(drop_tables_json),
            output: items_output.join("formatted_packages.json"),
        };
        if let Err(e) = soulframe_items::cli::run(fmt_cmd) {
            eprintln!("[!] items format-packages + droptables failed: {e:#}");
            failures.push("items format-packages + droptables");
        }
    }

    if !skip_resolved_loot {
        header("resolved loot");
        loot_resolved::run(loot_resolved::Args {
            packages: items_output.join("formatted_packages.json"),
            decrypted_dir: decrypted_output,
            output: items_output.clone(),
            exe: None,
            levels: loot_resolved::DEFAULT_LEVELS.to_vec(),
            languages: items_languages.exists().then_some(items_languages.clone()),
            invocations: None,
            input_context: if drop_extraction_succeeded {
                "pipeline_extraction_succeeded"
            } else if skip_droptables && !reuse_decrypted {
                "pipeline_binary_inputs_regenerated_without_capture"
            } else if skip_droptables {
                "retained_snapshot_with_drop_tables_skipped"
            } else {
                "existing_snapshot_after_extraction_failure"
            }
            .into(),
        })
        .context("resolved loot exports failed")?;
    }

    if !skip_images {
        header("images extract (SF/Interface/ + Temp/SWF PNGs + Interface SWFs)");
        for path_filter in ["SF/Interface/", "Temp/SWF/"] {
            let img_cmd = soulframe_images::cli::Cmd::Extract {
                cache_path: None,
                output: images_output.clone(),
                filter: Some("png".to_string()),
                path_filter: Some(path_filter.to_string()),
                limit: 0,
                list_caches: false,
                list_paths: false,
                list_all: false,
                path_filter_substring: false,
                flat: false,
                deswizzle_textures: false,
                fix_dims: false,
                output_format: "png".to_string(),
            };
            if let Err(e) = soulframe_images::cli::run(img_cmd) {
                eprintln!("[!] images extract failed for {path_filter}: {e:#}");
                failures.push("images");
            }
        }

        let swf_cmd = soulframe_images::cli::Cmd::Extract {
            cache_path: None,
            output: images_output,
            filter: Some("swf".to_string()),
            path_filter: Some("SF/Interface/".to_string()),
            limit: 0,
            list_caches: false,
            list_paths: false,
            list_all: true,
            path_filter_substring: false,
            flat: false,
            deswizzle_textures: false,
            fix_dims: false,
            output_format: "dds".to_string(),
        };
        if let Err(e) = soulframe_images::cli::run(swf_cmd) {
            eprintln!("[!] Interface SWF extraction failed: {e:#}");
            failures.push("Interface SWFs");
        }
    }

    if !skip_audio {
        header("audio extract (voice, music, ambience and sound effects)");
        if let Err(e) = soulframe_audio::cli::run_extract(soulframe_audio::cli::ExtractArgs {
            output: audio_output,
            ..Default::default()
        }) {
            eprintln!("[!] audio extract failed: {e:#}");
            failures.push("audio");
        }
    }

    if !skip_map {
        header("map extract (Midrath Redux tiles + JSON)");
        let map_cmd = soulframe_map::cli::Cmd::Extract {
            cache_path: None,
            output: map_output.clone(),
            packages: Some(items_output.join("formatted_packages.json")),
            skip_tiles: false,
            skip_json: false,
            list_tiles: false,
            skip_dungeons: false,
        };
        if let Err(e) = soulframe_map::cli::run(map_cmd) {
            eprintln!("[!] map extract failed: {e:#}");
            failures.push("map");
        }

        if !skip_calendars {
            header(&format!(
                "Gods and Ghosts {calendar_days}-day calendars (all unlock variations)"
            ));
            let ancestors = map_output.join("map-json").join("ancestors.json");
            let output = calendars_output.join("gods-and-ghosts-calendar.json");
            if let Err(e) = gods_and_ghosts::run_all_unlock_variations(
                &ancestors,
                &calendar_from,
                calendar_days,
                &output,
            ) {
                eprintln!("[!] Gods and Ghosts calendar failed: {e:#}");
                failures.push("Gods and Ghosts calendar");
            }
        }
    }

    let mut items_generated = false;
    if !skip_items {
        let all_items_out = items_output.join("all_items.json");
        let market_items_out = items_output.join("market_items.json");
        let ava_api_items_out = items_output.join("ava_api_items.json");
        let rewards_out = items_output.join("reward_distributions.json");

        // Materialise Languages.json (the items stage expects a flat {key: en_string}
        // dict; the languages-extract stage already produced this under
        // extracted-data/0/Languages/). Copy/alias it into the items stage's
        // expected input path so the items stage can find it without manual setup.
        if let Err(e) = prepare_languages_json(&items_languages) {
            eprintln!("[!] could not prepare Languages.json for items stage: {e:#}");
            failures.push("items language input");
        }

        header("items all-items");
        let cmd = soulframe_items::cli::Cmd::AllItems {
            packages: items_packages.clone(),
            languages: items_languages.clone(),
            recipes: items_recipes,
            output: all_items_out.clone(),
        };
        if let Err(e) = soulframe_items::cli::run(cmd) {
            eprintln!("[!] items all-items failed: {e:#}");
            failures.push("items all-items");
        } else {
            items_generated = true;
            println!("  [items] also wrote: {}", market_items_out.display());
            println!("  [items] also wrote: {}", ava_api_items_out.display());
        }

        header("epithets");
        if let Err(e) = epithets::run(
            &items_output.join("formatted_packages.json"),
            &items_packages,
            &items_languages,
            &artifacts_output.join("epithets.json"),
        ) {
            eprintln!("[!] epithets failed: {e:#}");
            failures.push("epithets");
        }

        header("items reward-distributions");
        let cmd = soulframe_items::cli::Cmd::RewardDistributions {
            manifests: items_manifests,
            output: rewards_out,
        };
        if let Err(e) = soulframe_items::cli::run(cmd) {
            eprintln!("[!] items reward-distributions failed: {e:#}");
            failures.push("reward-distributions");
        }

        if !skip_calendars {
            header(&format!("Zenith {calendar_days}-day weapon calendar"));
            let packages = items_output.join("formatted_packages.json");
            let output = calendars_output.join("zenith-calendar.json");
            if let Err(e) = zenith::run_calendar(
                &packages,
                Some(&market_items_out),
                &calendar_from,
                calendar_days,
                &output,
            ) {
                eprintln!("[!] Zenith calendar failed: {e:#}");
                failures.push("Zenith calendar");
            }

            header(&format!("Midrath {calendar_days}-day weather calendar"));
            let output = calendars_output
                .join("weather")
                .join("weather-calendar.json");
            if let Err(e) = weather::run_calendar(&packages, &calendar_from, calendar_days, &output)
            {
                eprintln!("[!] weather calendar failed: {e:#}");
                failures.push("weather calendar");
            }
        }
    }

    // Move only artifacts generated in this run. A skipped or failed stage
    // must not replace the retained final export with an older sibling file.
    let mut artifact_sources = Vec::new();
    if drop_extraction_succeeded {
        artifact_sources.push(raw_drop_tables_out.with_file_name("drop_tables.json"));
    }
    if items_generated {
        artifact_sources.push(items_output.join("ava_api_items.json"));
    }
    for src in artifact_sources {
        if src.exists() {
            let dest = artifacts_output.join(src.file_name().unwrap());
            if let Err(e) = std::fs::rename(&src, &dest) {
                eprintln!(
                    "[!] moving {} -> {} failed: {e}",
                    src.display(),
                    dest.display()
                );
                failures.push("final artifact move");
            }
        }
    }

    anyhow::ensure!(
        failures.is_empty(),
        "Pipeline stages failed: {}. Intermediates have been retained.",
        failures.join(", ")
    );
    println!("\nsoulkit-cli run-all complete.");
    Ok(())
}

/// Copy the English Languages JSON produced by the languages-extract stage
/// to the items stage's expected input path. Tries `output/Languages/Languages.json`
/// first (written directly by the updated extract stage), then falls back to
/// the extracted-data tree.
fn prepare_languages_json(dest: &std::path::Path) -> Result<()> {
    let published = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("Languages.json");
    if published.is_file() {
        let published_path = std::fs::canonicalize(&published)?;
        if !std::fs::canonicalize(dest).is_ok_and(|path| path == published_path) {
            std::fs::copy(&published, dest)?;
            println!(
                "  [items] copied {} -> {}",
                published.display(),
                dest.display()
            );
        }
        return Ok(());
    }
    if dest.exists() {
        return Ok(());
    }
    let candidates = [
        dest.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("Languages.json"),
        std::env::current_dir()?.join("output/Languages/Languages.json"),
        std::env::current_dir()?.join("extracted-data/0/Languages/Languages.json"),
        std::env::current_dir()?.join("extracted-data/0/Languages/en.json"),
    ];
    let src = candidates.iter().find(|p| p.exists()).ok_or_else(|| {
        anyhow::anyhow!("no extracted Languages JSON found under extracted-data/0/Languages/")
    })?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dest)?;
    println!("  [items] copied {} -> {}", src.display(), dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Days, NaiveDate};

    #[test]
    fn public_cli_rejects_removed_extractors_and_pipeline_option() {
        for argv in [
            vec!["soulkit-cli", "scripts", "extract"],
            vec!["soulkit-cli", "version", "--remote"],
            vec!["soulkit-cli", "api", "capture"],
        ] {
            let error = Cli::try_parse_from(argv).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }
        let error = Cli::try_parse_from(["soulkit-cli", "run-all", "--skip-version"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn public_cli_preserves_extraction_commands_and_its_own_version_flag() {
        for (command, action) in [
            ("images", "extract"),
            ("audio", "extract"),
            ("models", "extract"),
            ("map", "extract"),
            ("languages", "extract-from-cache"),
            ("items", "all-items"),
            ("droptables", "run-all"),
            ("droptables", "capture-keys"),
        ] {
            assert!(Cli::try_parse_from(["soulkit-cli", command, action]).is_ok());
        }
        let error = Cli::try_parse_from(["soulkit-cli", "--version"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn an_explicitly_empty_pipeline_does_not_access_game_data_or_create_output() {
        let workspace = pipeline_workspace::PipelineWorkspace::new(&std::env::temp_dir()).unwrap();
        let output = workspace.root().with_extension("final-output");
        let args = RunAllArgs {
            locales: "not/a/valid/locale".into(),
            droptables_output: output.clone(),
            keys_output: None,
            decrypted_output: None,
            images_output: output.join("images"),
            audio_output: output.join("audio"),
            map_output: output.join("maps"),
            items_packages: output.join("packages.json"),
            items_languages: output.join("Languages/Languages.json"),
            items_recipes: output.join("recipes.json"),
            items_manifests: None,
            items_output: output.clone(),
            calendars_output: output.clone(),
            artifacts_output: output.join("Artifacts"),
            calendar_from: None,
            calendar_days: 181,
            skip_images: true,
            skip_audio: true,
            skip_map: true,
            skip_items: true,
            skip_calendars: true,
            skip_droptables: true,
            skip_languages: true,
            skip_resolved_loot: true,
            keep_intermediates: false,
        };
        run_all_in(args, &workspace).unwrap();
        assert!(!output.exists());
        workspace.finish(true, false).unwrap();
    }

    #[test]
    fn fresh_language_alias_updates_custom_destinations_without_copying_over_itself() {
        let workspace = pipeline_workspace::PipelineWorkspace::new(&std::env::temp_dir()).unwrap();
        let alias = workspace.root().join("Languages.json");
        let custom = workspace.root().join("custom-language.json");
        std::fs::write(&alias, b"{\"current\":true}").unwrap();
        std::fs::write(&custom, b"{\"stale\":true}").unwrap();
        prepare_languages_json(&custom).unwrap();
        assert_eq!(
            std::fs::read(&custom).unwrap(),
            std::fs::read(&alias).unwrap()
        );
        prepare_languages_json(&alias).unwrap();
        assert_eq!(std::fs::read(&alias).unwrap(), b"{\"current\":true}");
        let new_custom = workspace.root().join("new-language.json");
        prepare_languages_json(&new_custom).unwrap();
        assert_eq!(
            std::fs::read(new_custom).unwrap(),
            std::fs::read(&alias).unwrap()
        );
        workspace.finish(true, false).unwrap();
    }

    #[test]
    fn default_pipeline_extracts_audio_for_both_entry_points() {
        for argv in [vec!["soulkit-cli"], vec!["soulkit-cli", "run-all"]] {
            let TopCmd::RunAll {
                audio_output,
                skip_audio,
                ..
            } = Cli::try_parse_from(argv).unwrap().into_command()
            else {
                panic!("expected the full pipeline");
            };
            assert_eq!(audio_output, std::path::Path::new("output/audio"));
            assert!(!skip_audio);
        }
    }

    #[test]
    fn audio_pipeline_output_and_skip_can_be_overridden() {
        let TopCmd::RunAll {
            audio_output,
            skip_audio,
            ..
        } = Cli::try_parse_from([
            "soulkit-cli",
            "run-all",
            "--audio-output",
            "local-audio",
            "--skip-audio",
        ])
        .unwrap()
        .into_command()
        else {
            panic!("expected the full pipeline");
        };
        assert_eq!(audio_output, std::path::Path::new("local-audio"));
        assert!(skip_audio);
    }

    #[test]
    fn audio_extractor_is_available_as_a_nested_command() {
        assert!(matches!(
            Cli::try_parse_from(["soulkit-cli", "audio", "extract"])
                .unwrap()
                .into_command(),
            TopCmd::Audio { .. }
        ));
    }

    #[test]
    fn model_exporter_is_available_as_a_nested_command() {
        assert!(matches!(
            Cli::try_parse_from(["soulkit-cli", "models", "extract", "--output", "out"])
                .unwrap()
                .into_command(),
            TopCmd::Models { .. }
        ));
    }

    #[test]
    fn successful_pipeline_cleans_intermediates_by_default_at_both_entry_points() {
        for argv in [vec!["soulkit-cli"], vec!["soulkit-cli", "run-all"]] {
            let TopCmd::RunAll {
                keys_output,
                decrypted_output,
                items_manifests,
                keep_intermediates,
                ..
            } = Cli::try_parse_from(argv).unwrap().into_command()
            else {
                panic!("expected the full pipeline");
            };
            assert!(
                keys_output.is_none() && decrypted_output.is_none() && items_manifests.is_none()
            );
            assert!(!keep_intermediates);
        }
        let TopCmd::RunAll {
            keep_intermediates, ..
        } = Cli::try_parse_from(["soulkit-cli", "run-all", "--keep-intermediates"])
            .unwrap()
            .into_command()
        else {
            panic!("expected the full pipeline");
        };
        assert!(keep_intermediates);
    }

    #[test]
    fn explicit_loot_snapshot_can_be_shared_by_pipeline_and_standalone_commands() {
        let TopCmd::RunAll {
            decrypted_output, ..
        } = Cli::try_parse_from([
            "soulkit-cli",
            "run-all",
            "--decrypted-output",
            "saved/decrypted",
        ])
        .unwrap()
        .into_command()
        else {
            panic!("expected the full pipeline");
        };
        let TopCmd::Droptables {
            cmd: soulframe_droptables::Cmd::RunAll { decrypted_dir, .. },
        } = Cli::try_parse_from([
            "soulkit-cli",
            "droptables",
            "run-all",
            "--decrypted-dir",
            "saved/decrypted",
        ])
        .unwrap()
        .into_command()
        else {
            panic!("expected drop-table extraction");
        };
        let TopCmd::ResolveLoot(args) = Cli::try_parse_from([
            "soulkit-cli",
            "resolve-loot",
            "--decrypted-dir",
            "saved/decrypted",
        ])
        .unwrap()
        .into_command() else {
            panic!("expected loot resolution");
        };
        assert_eq!(decrypted_output.as_ref(), Some(&decrypted_dir));
        assert_eq!(args.decrypted_dir, decrypted_dir);
    }

    #[test]
    fn default_pipeline_calendars_cover_90_days_either_side() {
        for argv in [vec!["soulkit-cli"], vec!["soulkit-cli", "run-all"]] {
            let TopCmd::RunAll {
                calendar_from,
                calendar_days,
                calendars_output,
                artifacts_output,
                skip_map,
                skip_items,
                skip_calendars,
                ..
            } = Cli::try_parse_from(argv).unwrap().into_command()
            else {
                panic!("expected the full pipeline");
            };
            assert_eq!(calendar_days, 181);
            assert_eq!(calendars_output, std::path::Path::new("output"));
            assert_eq!(artifacts_output, std::path::Path::new("output/Artifacts"));
            assert!(!skip_map && !skip_items && !skip_calendars);
            for (today, expected_first, expected_last) in [
                ("2026-09-12", "2026-06-14", "2026-12-11"),
                ("2024-03-01", "2023-12-02", "2024-05-30"),
                ("2026-01-01", "2025-10-03", "2026-04-01"),
            ] {
                let today = NaiveDate::parse_from_str(today, "%Y-%m-%d").unwrap();
                let from = pipeline_calendar_from(calendar_from.clone(), today);
                assert_eq!(from, expected_first);
                let first = NaiveDate::parse_from_str(&from, "%Y-%m-%d").unwrap();
                let last = first + Days::new(u64::from(calendar_days - 1));
                assert_eq!(last.format("%Y-%m-%d").to_string(), expected_last);
            }
        }
    }

    #[test]
    fn explicit_pipeline_calendar_window_is_preserved() {
        let TopCmd::RunAll {
            calendar_from,
            calendar_days,
            ..
        } = Cli::try_parse_from([
            "soulkit-cli",
            "run-all",
            "--calendar-from",
            "2026-08-17",
            "--calendar-days",
            "30",
        ])
        .unwrap()
        .into_command()
        else {
            panic!("expected the full pipeline");
        };
        let today = NaiveDate::from_ymd_opt(2026, 9, 12).unwrap();
        assert_eq!(pipeline_calendar_from(calendar_from, today), "2026-08-17");
        assert_eq!(calendar_days, 30);
    }
}
