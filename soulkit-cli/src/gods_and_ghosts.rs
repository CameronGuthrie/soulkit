use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use anyhow::{bail, Context, Result};
use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::de_rng::{daily_seed, DeRandom};

const METHOD: &str = "de_daily_seed_weighted_asset_order";
const ALWAYS_UNLOCKED: [&str; 2] = ["ZenithBoonNpc", "MooseBoonNpc"];

#[derive(Debug, Deserialize)]
struct AncestorManifest {
    event: String,
    npc_candidates: Vec<AncestorCandidate>,
}

#[derive(Clone, Debug, Deserialize)]
struct AncestorCandidate {
    resource: String,
    weight: f64,
}

#[derive(Debug, Serialize)]
struct SelectedAncestor {
    internal_name: String,
    resource: String,
    weight: f64,
    candidate_index: usize,
}

#[derive(Debug, Serialize)]
struct Rotation {
    date_utc: String,
    day_start_unix: u64,
    rng_seed: u64,
    rng_sample_30: u32,
    weighted_roll: f64,
    total_weight: f64,
    selected: SelectedAncestor,
}

#[derive(Debug, Serialize)]
struct Calendar {
    method: &'static str,
    event: String,
    candidate_manifest: String,
    from_utc: String,
    days: u32,
    candidates: Vec<OutputCandidate>,
    rotations: Vec<Rotation>,
}

#[derive(Debug, Serialize)]
struct OutputCandidate {
    internal_name: String,
    resource: String,
    weight: f64,
}

#[derive(Debug, Serialize)]
struct VariationIndex {
    method: &'static str,
    assumption: &'static str,
    candidate_manifest: String,
    from_utc: String,
    days: u32,
    always_unlocked: Vec<String>,
    optional_ancestors: Vec<String>,
    variation_count: usize,
    variations: Vec<VariationEntry>,
}

#[derive(Debug, Serialize)]
struct VariationEntry {
    id: usize,
    file: String,
    unlocked_ancestors: Vec<String>,
}

pub fn run_calendar(ancestors_path: &Path, from: &str, days: u32, output: &Path) -> Result<()> {
    let first_date = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .with_context(|| format!("invalid --from date {from:?}; expected YYYY-MM-DD"))?;
    let manifest = read_manifest(ancestors_path)?;
    validate_candidates(&manifest.npc_candidates)?;

    let calendar = build_calendar(
        &manifest,
        &manifest.npc_candidates,
        ancestors_path,
        from,
        first_date,
        days,
    )?;
    write_json(output, &calendar)?;

    let last_date = calendar
        .rotations
        .last()
        .map(|rotation| rotation.date_utc.as_str())
        .unwrap_or(from);
    println!(
        "Wrote {} Gods and Ghosts rotations ({} through {}) to {}",
        days,
        from,
        last_date,
        output.display()
    );
    Ok(())
}

pub fn run_all_unlock_variations(
    ancestors_path: &Path,
    from: &str,
    days: u32,
    output_dir: &Path,
) -> Result<()> {
    if days == 0 {
        bail!("calendar days must be at least 1");
    }
    let first_date = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .with_context(|| format!("invalid --from date {from:?}; expected YYYY-MM-DD"))?;
    let last_date = first_date
        .checked_add_days(Days::new(u64::from(days - 1)))
        .context("calendar date overflow")?;
    let manifest = read_manifest(ancestors_path)?;
    validate_candidates(&manifest.npc_candidates)?;
    let pools = unlock_variation_pools(&manifest.npc_candidates)?;

    let optional_ancestors = manifest
        .npc_candidates
        .iter()
        .filter(|candidate| !is_always_unlocked(candidate))
        .map(|candidate| display_name(&candidate.resource))
        .collect();
    let mut variations = Vec::with_capacity(pools.len());

    for (id, pool) in pools.iter().enumerate() {
        let file_name = format!("{}.json", pool.slug);
        let output = output_dir.join(&file_name);
        let calendar = build_calendar(
            &manifest,
            &pool.candidates,
            ancestors_path,
            from,
            first_date,
            days,
        )?;
        if id == 0 {
            prepare_variation_directory(output_dir)?;
        }
        write_json(&output, &calendar)?;
        variations.push(VariationEntry {
            id,
            file: file_name,
            unlocked_ancestors: pool
                .candidates
                .iter()
                .map(|candidate| display_name(&candidate.resource))
                .collect(),
        });
    }

    let index = VariationIndex {
        method: METHOD,
        assumption: "the weighted candidate pool contains only ancestors unlocked on the account",
        candidate_manifest: ancestors_path.display().to_string(),
        from_utc: from.to_string(),
        days,
        always_unlocked: ALWAYS_UNLOCKED
            .iter()
            .map(|name| display_name(name))
            .collect(),
        optional_ancestors,
        variation_count: variations.len(),
        variations,
    };
    write_json(&output_dir.join("index.json"), &index)?;

    println!(
        "Wrote {} Gods and Ghosts unlock variations ({} days, {} through {}) to {}",
        index.variation_count,
        days,
        from,
        last_date.format("%Y-%m-%d"),
        output_dir.display()
    );
    Ok(())
}

fn prepare_variation_directory(output_dir: &Path) -> Result<()> {
    if output_dir.is_file() {
        let mut backup_name = output_dir
            .file_name()
            .context("calendar output has no file name")?
            .to_os_string();
        backup_name.push(".bak");
        let backup_path = output_dir.with_file_name(backup_name);
        let mut original = File::open(output_dir)
            .with_context(|| format!("opening legacy calendar {}", output_dir.display()))?;
        let mut backup = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
            .with_context(|| {
                format!(
                    "preserving legacy calendar as {}; move any existing backup before retrying; {} has been retained",
                    backup_path.display(),
                    output_dir.display()
                )
            })?;
        std::io::copy(&mut original, &mut backup)
            .with_context(|| format!("backing up {}", output_dir.display()))?;
        backup
            .sync_all()
            .with_context(|| format!("flushing {}", backup_path.display()))?;
        drop(backup);
        drop(original);
        std::fs::remove_file(output_dir).with_context(|| {
            format!(
                "replacing legacy calendar {} with a directory",
                output_dir.display()
            )
        })?;
        println!("Preserved previous calendar at {}", backup_path.display());
    }
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;
    Ok(())
}

fn build_calendar(
    manifest: &AncestorManifest,
    selected_candidates: &[AncestorCandidate],
    ancestors_path: &Path,
    from: &str,
    first_date: NaiveDate,
    days: u32,
) -> Result<Calendar> {
    validate_candidates(selected_candidates)?;

    let mut rotations = Vec::with_capacity(days as usize);
    for offset in 0..days {
        let date = first_date
            .checked_add_days(Days::new(u64::from(offset)))
            .context("calendar date overflow")?;
        let unix_time = date
            .and_hms_opt(0, 0, 0)
            .context("invalid UTC calendar date")?
            .and_utc()
            .timestamp();
        let unix_time = u64::try_from(unix_time).context("dates before 1970 are unsupported")?;
        rotations.push(calculate_rotation(selected_candidates, date, unix_time)?);
    }

    let candidates = selected_candidates
        .iter()
        .map(|candidate| OutputCandidate {
            internal_name: internal_name(&candidate.resource).to_string(),
            resource: candidate.resource.clone(),
            weight: candidate.weight,
        })
        .collect();
    let calendar = Calendar {
        method: METHOD,
        event: manifest.event.clone(),
        candidate_manifest: ancestors_path.display().to_string(),
        from_utc: from.to_string(),
        days,
        candidates,
        rotations,
    };

    Ok(calendar)
}

fn write_json<T: Serialize>(output: &Path, value: &T) -> Result<()> {
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let file = File::create(output).with_context(|| format!("creating {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)
        .with_context(|| format!("writing {}", output.display()))?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}

struct UnlockPool {
    slug: String,
    candidates: Vec<AncestorCandidate>,
}

fn unlock_variation_pools(candidates: &[AncestorCandidate]) -> Result<Vec<UnlockPool>> {
    for required in ALWAYS_UNLOCKED {
        if !candidates
            .iter()
            .any(|candidate| internal_name(&candidate.resource) == required)
        {
            bail!("ancestor manifest is missing always-unlocked candidate {required}");
        }
    }

    let optional = candidates
        .iter()
        .filter(|candidate| !is_always_unlocked(candidate))
        .collect::<Vec<_>>();
    if optional.len() >= usize::BITS as usize {
        bail!("too many optional ancestors to enumerate unlock variations");
    }

    let variation_count = 1usize << optional.len();
    let mut pools = Vec::with_capacity(variation_count);
    for mask in 0..variation_count {
        let selected = candidates
            .iter()
            .filter(|candidate| {
                if is_always_unlocked(candidate) {
                    return true;
                }
                let optional_index = optional
                    .iter()
                    .position(|optional| optional.resource == candidate.resource)
                    .expect("candidate came from optional list");
                mask & (1usize << optional_index) != 0
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut slug_parts = ALWAYS_UNLOCKED
            .iter()
            .map(|name| ancestor_slug(name))
            .collect::<Vec<_>>();
        slug_parts.extend(
            optional
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1usize << index) != 0)
                .map(|(_, candidate)| ancestor_slug(&candidate.resource)),
        );
        pools.push(UnlockPool {
            slug: slug_parts.join("-"),
            candidates: selected,
        });
    }
    Ok(pools)
}

fn is_always_unlocked(candidate: &AncestorCandidate) -> bool {
    ALWAYS_UNLOCKED.contains(&internal_name(&candidate.resource))
}

fn display_name(resource: &str) -> String {
    let name = internal_name(resource)
        .strip_suffix("BoonNpc")
        .unwrap_or_else(|| internal_name(resource));
    let mut output = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if index > 0 && ch.is_ascii_uppercase() {
            output.push(' ');
        }
        output.push(ch);
    }
    output
}

fn ancestor_slug(resource: &str) -> String {
    display_name(resource)
        .to_ascii_lowercase()
        .replace(' ', "-")
}

fn read_manifest(path: &Path) -> Result<AncestorManifest> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parsing {}", path.display()))
}

fn validate_candidates(candidates: &[AncestorCandidate]) -> Result<()> {
    if candidates.is_empty() {
        bail!("ancestor manifest contains no npc_candidates");
    }
    for candidate in candidates {
        if candidate.resource.is_empty() {
            bail!("ancestor manifest contains a candidate with an empty resource path");
        }
        if !candidate.weight.is_finite() || candidate.weight <= 0.0 {
            bail!(
                "ancestor candidate {} has invalid weight {}",
                candidate.resource,
                candidate.weight
            );
        }
    }
    Ok(())
}

fn calculate_rotation(
    candidates: &[AncestorCandidate],
    date: NaiveDate,
    unix_time: u64,
) -> Result<Rotation> {
    let (day_start_unix, rng_seed) =
        daily_seed(unix_time).context("Gods and Ghosts dates before 2004-01-14 are unsupported")?;
    let total_weight: f64 = candidates.iter().map(|candidate| candidate.weight).sum();
    let mut rng = DeRandom::seeded(rng_seed);
    let rng_sample_30 = rng.sample_30();
    let weighted_roll = f64::from(rng_sample_30) / 1_073_741_824.0 * total_weight;

    let mut remainder = weighted_roll;
    let mut selected_index = candidates.len() - 1;
    for (index, candidate) in candidates.iter().enumerate() {
        if remainder < candidate.weight {
            selected_index = index;
            break;
        }
        remainder -= candidate.weight;
    }
    let selected = &candidates[selected_index];

    Ok(Rotation {
        date_utc: date.format("%Y-%m-%d").to_string(),
        day_start_unix,
        rng_seed,
        rng_sample_30,
        weighted_roll,
        total_weight,
        selected: SelectedAncestor {
            internal_name: internal_name(&selected.resource).to_string(),
            resource: selected.resource.clone(),
            weight: selected.weight,
            candidate_index: selected_index,
        },
    })
}

fn internal_name(resource: &str) -> &str {
    resource.rsplit('/').next().unwrap_or(resource)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "soulkit-gods-and-ghosts-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn manifest(&self) -> PathBuf {
            let path = self.0.join("ancestors.json");
            let candidates = current_candidates()
                .into_iter()
                .map(|candidate| {
                    serde_json::json!({
                        "resource": candidate.resource,
                        "weight": candidate.weight,
                    })
                })
                .collect::<Vec<_>>();
            write_json(
                &path,
                &serde_json::json!({"event": "GodsAndGhosts", "npc_candidates": candidates}),
            )
            .unwrap();
            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_reader(File::open(path).unwrap()).unwrap()
    }

    fn current_candidates() -> Vec<AncestorCandidate> {
        [
            ("GarrenRoodBoonNpc", 1.0),
            ("BromiusBoonNpc", 1.0),
            ("OrengallBoonNpc", 1.0),
            ("MooseBoonNpc", 0.40000001),
            ("ZenithBoonNpc", 0.80000001),
            ("TuvalkaneBoonNpc", 1.0),
            ("VerminiaBoonNpc", 1.0),
            ("OrlickBoonNpc", 1.0),
        ]
        .into_iter()
        .map(|(name, weight)| AncestorCandidate {
            resource: format!("/SF/Types/Npcs/BoonNPC/{name}"),
            weight,
        })
        .collect()
    }

    fn rotation(date: &str) -> Rotation {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        let unix_time = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() as u64;
        calculate_rotation(&current_candidates(), date, unix_time).unwrap()
    }

    #[test]
    fn current_asset_matches_confirmed_august_17_encounter() {
        assert_eq!(
            rotation("2026-08-17").selected.resource,
            "/SF/Types/Npcs/BoonNPC/TuvalkaneBoonNpc"
        );
    }

    #[test]
    fn current_weights_are_not_a_hard_coded_28_day_chart() {
        assert_eq!(
            rotation("2026-08-27").selected.resource,
            "/SF/Types/Npcs/BoonNPC/ZenithBoonNpc"
        );
    }

    #[test]
    fn unlock_variations_include_every_optional_subset() {
        let pools = unlock_variation_pools(&current_candidates()).unwrap();
        assert_eq!(pools.len(), 64);
        assert_eq!(pools[0].slug, "zenith-moose");
        assert_eq!(pools[0].candidates.len(), 2);
        assert!(pools[0].candidates.iter().all(is_always_unlocked));
        assert_eq!(pools[63].candidates.len(), 8);
    }

    #[test]
    fn variation_files_cover_every_subset_and_refresh_the_calendar_window() {
        let directory = TestDirectory::new();
        let manifest = directory.manifest();
        let output = directory.0.join("gods-and-ghosts-calendar.json");
        run_all_unlock_variations(&manifest, "2024-01-01", 181, &output).unwrap();

        let index = read_json(&output.join("index.json"));
        assert_eq!(index["from_utc"], "2024-01-01");
        assert_eq!(index["days"], 181);
        assert_eq!(index["variation_count"], 64);
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 65);
        let entries = index["variations"].as_array().unwrap();
        assert_eq!(entries.len(), 64);
        let source_candidates = current_candidates();
        let optional = source_candidates
            .iter()
            .filter(|candidate| !is_always_unlocked(candidate))
            .collect::<Vec<_>>();
        let mut files = HashSet::new();
        let mut subsets = HashSet::new();

        for entry in entries {
            let file = entry["file"].as_str().unwrap();
            assert!(files.insert(file));
            let calendar = read_json(&output.join(file));
            assert_eq!(calendar["from_utc"], "2024-01-01");
            assert_eq!(calendar["days"], 181);
            let candidates = calendar["candidates"].as_array().unwrap();
            let resources = candidates
                .iter()
                .map(|candidate| candidate["resource"].as_str().unwrap())
                .collect::<Vec<_>>();
            for required in ALWAYS_UNLOCKED {
                assert!(resources
                    .iter()
                    .any(|resource| internal_name(resource) == required));
            }
            let mask = optional
                .iter()
                .enumerate()
                .fold(0usize, |mask, (bit, candidate)| {
                    mask | if resources.contains(&candidate.resource.as_str()) {
                        1 << bit
                    } else {
                        0
                    }
                });
            assert!(subsets.insert(mask));
            let in_asset_order = source_candidates
                .iter()
                .filter(|candidate| resources.contains(&candidate.resource.as_str()))
                .map(|candidate| candidate.resource.as_str())
                .collect::<Vec<_>>();
            assert_eq!(resources, in_asset_order);
            let names = resources
                .iter()
                .map(|resource| display_name(resource))
                .collect::<Vec<_>>();
            assert_eq!(entry["unlocked_ancestors"], serde_json::json!(names));

            let rotations = calendar["rotations"].as_array().unwrap();
            assert_eq!(rotations.len(), 181);
            assert_eq!(rotations[0]["date_utc"], "2024-01-01");
            assert_eq!(rotations[90]["date_utc"], "2024-03-31");
            assert_eq!(rotations[180]["date_utc"], "2024-06-29");
            for rotation in rotations {
                let selected = &rotation["selected"];
                assert!(resources.contains(&selected["resource"].as_str().unwrap()));
                let selected_index = selected["candidate_index"].as_u64().unwrap() as usize;
                assert_eq!(selected["resource"], candidates[selected_index]["resource"]);
            }
            for pair in rotations.windows(2) {
                assert_eq!(
                    pair[1]["day_start_unix"].as_u64().unwrap()
                        - pair[0]["day_start_unix"].as_u64().unwrap(),
                    86_400
                );
            }
            if mask == 63 {
                let single = directory.0.join("single-calendar.json");
                run_calendar(&manifest, "2024-01-01", 181, &single).unwrap();
                assert_eq!(calendar, read_json(&single));
            }
        }
        assert_eq!(subsets, (0usize..64).collect());

        run_all_unlock_variations(&manifest, "2024-07-01", 2, &output).unwrap();
        let refreshed = read_json(&output.join("index.json"));
        assert_eq!(refreshed["from_utc"], "2024-07-01");
        assert_eq!(refreshed["days"], 2);
        assert_eq!(refreshed["variations"], index["variations"]);
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 65);
        for entry in entries {
            let calendar = read_json(&output.join(entry["file"].as_str().unwrap()));
            assert_eq!(calendar["from_utc"], "2024-07-01");
            assert_eq!(calendar["days"], 2);
            let rotations = calendar["rotations"].as_array().unwrap();
            assert_eq!(rotations.len(), 2);
            assert_eq!(rotations[0]["date_utc"], "2024-07-01");
            assert_eq!(rotations[1]["date_utc"], "2024-07-02");
        }
    }

    #[test]
    fn legacy_calendar_is_backed_up_before_becoming_a_directory() {
        let directory = TestDirectory::new();
        let manifest = directory.manifest();
        let output = directory.0.join("gods-and-ghosts-calendar.json");
        let backup = directory.0.join("gods-and-ghosts-calendar.json.bak");
        let original = b"{\"previous_calendar\":true}\n";
        std::fs::write(&output, original).unwrap();

        run_all_unlock_variations(&manifest, "2024-01-01", 1, &output).unwrap();
        assert!(output.is_dir());
        assert_eq!(std::fs::read(&backup).unwrap(), original);
        assert_eq!(read_json(&output.join("index.json"))["variation_count"], 64);

        run_all_unlock_variations(&manifest, "2024-01-02", 1, &output).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), original);
    }

    #[test]
    fn existing_backup_prevents_migration_without_overwriting_either_file() {
        let directory = TestDirectory::new();
        let manifest = directory.manifest();
        let output = directory.0.join("gods-and-ghosts-calendar.json");
        let backup = directory.0.join("gods-and-ghosts-calendar.json.bak");
        std::fs::write(&output, b"original calendar").unwrap();
        std::fs::write(&backup, b"existing backup").unwrap();

        let error = run_all_unlock_variations(&manifest, "2024-01-01", 1, &output).unwrap_err();
        assert!(error
            .to_string()
            .contains("move any existing backup before retrying"));
        assert_eq!(std::fs::read(&output).unwrap(), b"original calendar");
        assert_eq!(std::fs::read(&backup).unwrap(), b"existing backup");
    }

    #[test]
    fn invalid_calendars_do_not_migrate_the_legacy_file() {
        let directory = TestDirectory::new();
        let manifest = directory.manifest();
        let output = directory.0.join("gods-and-ghosts-calendar.json");
        let backup = directory.0.join("gods-and-ghosts-calendar.json.bak");
        std::fs::write(&output, b"original calendar").unwrap();

        for (from, days) in [
            ("2024-01-01", 0),
            ("invalid date", 181),
            ("2004-01-13", 181),
        ] {
            assert!(run_all_unlock_variations(&manifest, from, days, &output).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), b"original calendar");
            assert!(!backup.exists());
        }
    }
}
