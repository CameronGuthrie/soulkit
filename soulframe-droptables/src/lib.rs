//! Library surface for the Soulframe drop-table pipeline.
//!
//! The CLI subcommand enum and dispatcher live here so other crates (such as
//! the `soulkit-cli` umbrella binary) can compose them as a nested subcommand
//! without reimplementing the logic.

pub mod capture;
pub mod decrypt;
pub mod find_rvas;
pub mod parse;
pub mod resolved_raw;
pub mod resolved_selector;
pub mod scan_rewards;

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Scan the Soulframe binary for Camellia function RVAs.
    FindRvas {
        /// Path to Soulframe.x64.exe (defaults to %LOCALAPPDATA% install).
        #[arg(long)]
        exe: Option<PathBuf>,
    },
    /// Launch the game under a debugger and capture Camellia-256 keys.
    /// Windows-only.
    CaptureKeys {
        /// Path to Soulframe.x64.exe (defaults to %LOCALAPPDATA% install).
        #[arg(long)]
        exe: Option<PathBuf>,
        /// Output directory for `camellia_keys.txt` and `capture_output.txt`.
        #[arg(long, default_value = "output")]
        output_dir: PathBuf,
        /// Override the inline Camellia RVA (otherwise auto-detected).
        #[arg(long, value_parser = parse_hex_u32)]
        inline_rva: Option<u32>,
        /// Override the backup Camellia RVA (otherwise auto-detected).
        #[arg(long, value_parser = parse_hex_u32)]
        backup_rva: Option<u32>,
        /// Hard timeout in seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// Stop early N seconds after last hit.
        #[arg(long, default_value_t = 60)]
        quiet_detach: u64,
    },
    /// Decrypt drop-table entries from the local H.Misc cache.
    Decrypt {
        /// Path to camellia_keys.txt OR capture_output.txt OR a 64-hex-char key.
        /// Pass `auto` (or omit) to rely solely on path-derived keys.
        key_source: Option<String>,
        /// Output directory for decrypted `.bin` files.
        #[arg(long, default_value = "decrypted_raw")]
        output: PathBuf,
        /// Override the Soulframe cache directory.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    /// Parse decrypted `.bin` files into shaped and raw per-table JSON.
    Parse {
        /// Directory containing decrypted `.bin` files.
        decrypted_dir: PathBuf,
        /// Output directory for `drop_tables.json`, `raw_drop_tables.json`, and per-table files.
        #[arg(long, default_value = "drop_tables")]
        output: PathBuf,
        /// Optional `packages_bin_attrs.json` used to add item quantity min/max
        /// metadata to parsed drop-table items.
        #[arg(long)]
        packages_attrs: Option<PathBuf>,
    },
    /// Scan a running Soulframe process for runtime reward-table data.
    ///
    /// Faction Votive item manifests are empty local objects even though the
    /// activity awards fragments. Probe for a hot-loaded or native-built table.
    ///
    /// Windows-only.
    ScanRewards {
        /// Output file for scan results (JSON).
        #[arg(long, default_value = "output/mission_rewards_scan.json")]
        output: PathBuf,
        /// PID of the running Soulframe process (auto-detected if omitted).
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Run the full pipeline: decrypt → parse. Tables decrypt with
    /// path-derived keys; the game is launched (find-rvas → capture-keys →
    /// scan-rewards) only if some table fails that way, or with `--capture`.
    RunAll {
        /// Path to Soulframe.x64.exe.
        #[arg(long)]
        exe: Option<PathBuf>,
        /// Working directory (drop_tables/, mission_rewards_scan.json land here).
        #[arg(long, default_value = "output")]
        output_dir: PathBuf,
        /// Directory for `camellia_keys.txt` and `capture_output.txt` — kept out
        /// of `output_dir` since these are sensitive decryption secrets, not
        /// pipeline data.
        #[arg(long, default_value = "keys")]
        keys_dir: PathBuf,
        /// Directory for decrypted raw `.bin` manifest dumps — kept out of
        /// `output_dir` since these are raw game data, not pipeline output.
        #[arg(long, default_value = "decrypted_raw")]
        decrypted_dir: PathBuf,
        /// Optional `packages_bin_attrs.json` used to add item quantity min/max
        /// metadata to parsed drop-table items.
        #[arg(long)]
        packages_attrs: Option<PathBuf>,
        /// Hard timeout for capture-keys in seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// Launch the game to capture keys and scan runtime rewards even when
        /// path-derived keys decrypt every table.
        #[arg(long)]
        capture: bool,
    },
}

pub fn parse_hex_u32(s: &str) -> Result<u32, String> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).map_err(|e| e.to_string())
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::FindRvas { exe } => {
            let r = find_rvas::find_rvas(exe.as_deref())?;
            println!("\n=== SUMMARY ===");
            println!(
                "Inline RVA:  {}",
                r.inline
                    .map(|v| format!("0x{:08X}", v))
                    .unwrap_or_else(|| "<none>".into())
            );
            println!(
                "Backup RVA:  {}",
                r.backup
                    .map(|v| format!("0x{:08X}", v))
                    .unwrap_or_else(|| "<none>".into())
            );
            if let Some(p) = r.inline_prologue {
                println!("Prologue:    {}", p);
            }
            println!("Clusters: {}", r.clusters.len());
            for (i, c) in r.clusters.iter().enumerate() {
                println!(
                    "  [{}] section={} pairs={}/6 sigma={} func={} span={}B",
                    i + 1,
                    c.section,
                    c.pairs,
                    c.sigma_count,
                    c.func_rva
                        .map(|v| format!("0x{:08X}", v))
                        .unwrap_or_else(|| "?".into()),
                    c.span
                );
            }
        }

        Cmd::CaptureKeys {
            exe,
            output_dir,
            inline_rva,
            backup_rva,
            timeout,
            quiet_detach,
        } => {
            let (inline, backup) = if inline_rva.is_some() && backup_rva.is_some() {
                (inline_rva.unwrap(), backup_rva.unwrap())
            } else {
                let scan = find_rvas::find_rvas(exe.as_deref()).ok();
                let inline = inline_rva
                    .or_else(|| scan.as_ref().and_then(|s| s.inline))
                    .unwrap_or(0x01919D70);
                let backup = backup_rva
                    .or_else(|| scan.as_ref().and_then(|s| s.backup))
                    .unwrap_or(0x01D69540);
                (inline, backup)
            };

            let game_exe = exe
                .or_else(find_rvas::default_game_exe)
                .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA not set"))?;

            let opts = capture::CaptureOptions {
                game_exe,
                output_dir: output_dir.clone(),
                inline_rva: inline,
                backup_rva: backup,
                timeout_secs: timeout,
                quiet_detach_secs: quiet_detach,
            };
            let summary = capture::run(&opts)?;
            println!(
                "\nCapture done: hits={} unique={} elapsed={:.1}s",
                summary.hits, summary.unique_keys, summary.elapsed_secs
            );
        }

        Cmd::Decrypt {
            key_source,
            output,
            cache_dir,
        } => {
            let keys: Vec<[u8; 32]> = match key_source.as_deref() {
                None | Some("auto") | Some("AUTO") => Vec::new(),
                Some(src) => {
                    if src.len() == 64 && src.chars().all(|c| c.is_ascii_hexdigit()) {
                        vec![decrypt::parse_inline_key(src)?]
                    } else {
                        decrypt::load_keys_from_file(std::path::Path::new(src))?
                    }
                }
            };
            println!(
                "Loaded {} captured keys (path-derived fallback always on)",
                keys.len()
            );

            let entries = decrypt::load_encrypted_entries(cache_dir.as_deref())
                .context("loading encrypted H.Misc entries")?;
            println!("Found {} /SF/ drop-table candidate entries", entries.len());

            let (ok, failed) = decrypt::decrypt_all(&keys, &entries);
            let path_derived = ok
                .iter()
                .filter(|r| r.key_source == decrypt::KeySource::PathDerived)
                .count();
            let captured = ok
                .iter()
                .filter(|r| r.key_source == decrypt::KeySource::Captured)
                .count();
            let plaintext = ok
                .iter()
                .filter(|r| r.key_source == decrypt::KeySource::Plaintext)
                .count();

            println!("\n  Extracted: {}", ok.len());
            println!("  Failed:    {}", failed.len());
            println!("  Total:     {}", entries.len());

            let written = decrypt::write_decrypted(&ok, &output)?;
            println!(
                "\n  Written {} .bin files to {} from {} extracted entries ({} path-derived, {} captured-key, {} plaintext)",
                written,
                output.display(),
                ok.len(),
                path_derived,
                captured,
                plaintext
            );
            if !failed.is_empty() {
                println!("\n  Failed entries (no matching key):");
                for p in failed.iter().take(20) {
                    println!("    {}", p);
                }
                if failed.len() > 20 {
                    println!("    ... and {} more", failed.len() - 20);
                }
                anyhow::bail!(
                    "{} drop-table entries could not be decrypted; partial inputs were retained",
                    failed.len()
                );
            }
        }

        Cmd::ScanRewards { output, pid } => {
            scan_rewards::run(pid, &output)?;
        }

        Cmd::Parse {
            decrypted_dir,
            output,
            packages_attrs,
        } => {
            let s = parse::parse_directory_with_packages_attrs(
                &decrypted_dir,
                &output,
                packages_attrs.as_deref(),
            )?;
            println!("\n=== PARSE RESULTS ===");
            println!("Tables: {}", s.table_count);
            println!("Integrated mission reward manifests: {}", s.manifest_count);
            println!("Total pools: {}", s.total_pools);
            println!("Total items: {}", s.total_items);
            println!("Pools with dc=None: {}", s.dc_none_pools);
            println!("Pools with bad probability sums: {}", s.bad_prob_sum_pools);
            println!(
                "Referenced drop-table paths: {} ({} unresolved)",
                s.drop_table_reference_count, s.unresolved_drop_table_reference_count
            );
            if let Some(q) = &s.quantity_summary {
                println!(
                    "Quantity-enriched items: {} (explicit: {}, defaulted: {}, missing attrs: {})",
                    q.enriched_items, q.explicit_items, q.defaulted_items, q.missing_attrs_items
                );
            }
            println!("\nWritten to {}", output.display());
            println!(
                "Raw/default JSON: {}",
                output.join("raw_drop_tables.json").display()
            );
        }

        Cmd::RunAll {
            exe,
            output_dir,
            keys_dir,
            decrypted_dir,
            packages_attrs,
            timeout,
            capture,
        } => {
            std::fs::create_dir_all(&output_dir)?;
            std::fs::create_dir_all(&keys_dir)?;

            println!("\n=== STEP 1/2: decrypt ===");
            let key_file = keys_dir.join("camellia_keys.txt");
            let load_keys = || -> Result<Vec<[u8; 32]>> {
                if key_file.exists() {
                    decrypt::load_keys_from_file(&key_file)
                } else {
                    Ok(Vec::new())
                }
            };
            let entries = decrypt::load_encrypted_entries(None)?;
            let (mut ok, mut failed) = decrypt::decrypt_all(&load_keys()?, &entries);

            if capture || !failed.is_empty() {
                if !failed.is_empty() {
                    println!(
                        "  {} table(s) did not decrypt with path-derived or saved keys; launching the game to capture keys:",
                        failed.len()
                    );
                    for p in &failed {
                        println!("    {p}");
                    }
                }
                capture_keys_and_rewards(exe, &output_dir, &keys_dir, timeout)?;
                (ok, failed) = decrypt::decrypt_all(&load_keys()?, &entries);
            }

            let plaintext = ok
                .iter()
                .filter(|r| r.key_source == decrypt::KeySource::Plaintext)
                .count();
            // Clear the previous build's dumps: parse reads every .bin here, so
            // tables removed from the game would otherwise survive forever.
            if decrypted_dir.is_dir() {
                for f in std::fs::read_dir(&decrypted_dir)? {
                    let p = f?.path();
                    if p.extension().is_some_and(|e| e == "bin") {
                        std::fs::remove_file(&p)?;
                    }
                }
            }
            let written = decrypt::write_decrypted(&ok, &decrypted_dir)?;
            println!(
                "  Extracted {}/{}  (failed: {}, plaintext: {}, files: {})",
                ok.len(),
                entries.len(),
                failed.len(),
                plaintext,
                written
            );

            println!("\n=== STEP 2/2: parse ===");
            let drop_tables_dir = output_dir.join("drop_tables");
            let inferred_packages_attrs = output_dir.join("packages_bin_attrs.json");
            let packages_attrs = packages_attrs.or_else(|| {
                inferred_packages_attrs
                    .exists()
                    .then_some(inferred_packages_attrs)
            });
            let s = parse::parse_directory_with_packages_attrs(
                &decrypted_dir,
                &drop_tables_dir,
                packages_attrs.as_deref(),
            )?;
            println!(
                "  Parsed {} tables, {} pools, {} items",
                s.table_count, s.total_pools, s.total_items
            );
            println!(
                "  Referenced drop-table paths: {} ({} unresolved)",
                s.drop_table_reference_count, s.unresolved_drop_table_reference_count
            );
            if let Some(q) = &s.quantity_summary {
                println!(
                    "  Quantity-enriched items: {} (explicit: {}, defaulted: {}, missing attrs: {})",
                    q.enriched_items, q.explicit_items, q.defaulted_items, q.missing_attrs_items
                );
            }
            println!(
                "  Raw/default JSON: {}",
                drop_tables_dir.join("raw_drop_tables.json").display()
            );

            anyhow::ensure!(
                failed.is_empty(),
                "{} drop-table entries could not be decrypted; partial inputs were retained",
                failed.len()
            );
            println!("\nPipeline complete. Output in: {}", output_dir.display());
        }
    }

    Ok(())
}

/// Launch the game under the debugger, save captured keys to `keys_dir`, and
/// scan its heap for runtime reward tables before closing it.
#[cfg(windows)]
fn capture_keys_and_rewards(
    exe: Option<PathBuf>,
    output_dir: &std::path::Path,
    keys_dir: &std::path::Path,
    timeout: u64,
) -> Result<()> {
    println!("\n=== find-rvas ===");
    let rvas = find_rvas::find_rvas(exe.as_deref())?;
    let inline = rvas.inline.unwrap_or(0x01919D70);
    let backup = rvas.backup.unwrap_or(0x01D69540);
    println!("INLINE=0x{:08X}  BACKUP=0x{:08X}", inline, backup);

    println!("\n=== capture-keys ===");
    let game_exe = exe
        .or_else(find_rvas::default_game_exe)
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA not set"))?;
    let opts = capture::CaptureOptions {
        game_exe,
        output_dir: keys_dir.to_path_buf(),
        inline_rva: inline,
        backup_rva: backup,
        timeout_secs: timeout,
        // Keys arrive in one continuous burst while the client
        // loads its encrypted assets (~25s, 112 keys, no gap), then
        // stop for good — nothing drives further loads on an
        // unattended title screen. 20s of silence is the detach
        // evidence; raise it if a future build streams keys later
        // in the session.
        quiet_detach_secs: 20,
    };
    let launched_pid = capture::run(&opts).ok().map(|s| s.launched_pid);

    // The game is still alive after the debugger detaches
    // (DebugSetProcessKillOnExit was set to false). Scan its heap
    // for any runtime reward tables that are not represented by the
    // decrypted cache files.
    println!("\n=== scan-rewards ===");
    let rewards_out = output_dir.join("mission_rewards_scan.json");
    if let Err(e) = scan_rewards::run(None, &rewards_out) {
        eprintln!("[!] scan-rewards failed (game may have already exited): {e:#}");
    }

    // The rest only reads files, so the game has no reader left
    // after this point and would otherwise sit idle holding several
    // GB until the user noticed. Only the instance we launched is
    // touched; one the user started themselves keeps running.
    if let Some(pid) = launched_pid {
        match capture::terminate_pid(pid) {
            Ok(()) => println!("[*] Closed the Soulframe instance we launched (PID {pid})."),
            Err(e) => eprintln!("[!] Could not close PID {pid}: {e:#}"),
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn capture_keys_and_rewards(
    _exe: Option<PathBuf>,
    _output_dir: &std::path::Path,
    _keys_dir: &std::path::Path,
    _timeout: u64,
) -> Result<()> {
    anyhow::bail!("capture-keys + scan-rewards are Windows-only")
}
