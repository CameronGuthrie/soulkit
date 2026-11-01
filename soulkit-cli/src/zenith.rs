use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::de_rng::{daily_seed, DeRandom};

const PRODUCTS_MANIFEST: &str = "/Lotus/Types/Game/Store/ProductsManifest";
const ONE_HAND_MELEE: &str = "/SF/Weapons/Melee/OneHandMeleeWeapon";
const TWO_HAND_MELEE: &str = "/SF/Weapons/Melee/TwoHandMeleeWeapon";
const CASTER_MELEE: &str = "/SF/Types/Game/SFCasterMeleeWeapon";
const RANGED_MELEE: &str = "/SF/Types/Game/SFRangedMeleeWeapon";

#[derive(Debug, Deserialize)]
struct Package {
    #[serde(default)]
    attributes: HashMap<String, Value>,
    parent: Option<String>,
}

type Packages = HashMap<String, Option<Package>>;

#[derive(Debug, Deserialize)]
struct MarketItems {
    items: Vec<MarketItem>,
}

#[derive(Debug, Deserialize)]
struct MarketItem {
    path: String,
    name: String,
}

struct Inputs {
    packages: Packages,
    names: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum OfferCategory {
    BowOrRanged,
    Melee,
    Caster,
}

impl OfferCategory {
    fn display_name(self) -> &'static str {
        match self {
            Self::BowOrRanged => "Bow/ranged",
            Self::Melee => "Melee",
            Self::Caster => "Caster",
        }
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    path: String,
    name: String,
    premium_price: Option<u64>,
    rarity: Option<String>,
}

#[derive(Debug, Serialize)]
struct Offer {
    category: OfferCategory,
    name: String,
    path: String,
    premium_price: Option<u64>,
    rarity: Option<String>,
}

#[derive(Debug, Serialize)]
struct ZenithRotation {
    unix_time: u64,
    day_start_unix: u64,
    rng_seed: u64,
    pool_sizes: PoolSizes,
    offers: Vec<Offer>,
}

#[derive(Debug, Serialize)]
struct PoolSizes {
    bow_or_ranged: usize,
    melee: usize,
    caster: usize,
}

#[derive(Debug, Serialize)]
struct CalendarRotation {
    date_utc: String,
    #[serde(flatten)]
    rotation: ZenithRotation,
}

#[derive(Debug, Serialize)]
struct ZenithCalendar {
    manifest: String,
    from_utc: String,
    days: u32,
    rotations: Vec<CalendarRotation>,
}

pub fn run(
    packages_path: &Path,
    market_items_path: Option<&Path>,
    worldstate_path: Option<&Path>,
    unix_time: Option<u64>,
    json: bool,
) -> Result<()> {
    if worldstate_path.is_some() && unix_time.is_some() {
        bail!("use either --worldstate or --unix-time, not both");
    }

    let unix_time = match (worldstate_path, unix_time) {
        (Some(path), None) => read_worldstate_time(path)?,
        (None, Some(value)) => value,
        (None, None) => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_secs(),
        (Some(_), Some(_)) => unreachable!(),
    };

    let inputs = load_inputs(packages_path, market_items_path)?;

    let rotation = calculate_rotation(&inputs, unix_time)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rotation)?);
    } else {
        println!("Zenith daily weapon offers");
        println!("  Unix time:      {}", rotation.unix_time);
        println!("  UTC day start:  {}", rotation.day_start_unix);
        println!("  Engine RNG seed: {}", rotation.rng_seed);
        println!(
            "  Eligible pools: {} bow/ranged, {} melee, {} caster",
            rotation.pool_sizes.bow_or_ranged,
            rotation.pool_sizes.melee,
            rotation.pool_sizes.caster
        );
        for offer in rotation.offers {
            let price = offer
                .premium_price
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown price".to_string());
            let rarity = offer.rarity.as_deref().unwrap_or("COMMON/default");
            println!(
                "  - {:<11} {} — {} ({})\n                  {}",
                offer.category.display_name(),
                offer.name,
                price,
                rarity,
                offer.path
            );
        }
    }

    Ok(())
}

pub fn run_calendar(
    packages_path: &Path,
    market_items_path: Option<&Path>,
    from: &str,
    days: u32,
    output: &Path,
) -> Result<()> {
    let first_date = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .with_context(|| format!("invalid --from date {from:?}; expected YYYY-MM-DD"))?;
    let inputs = load_inputs(packages_path, market_items_path)?;
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
        rotations.push(CalendarRotation {
            date_utc: date.format("%Y-%m-%d").to_string(),
            rotation: calculate_rotation(&inputs, unix_time)?,
        });
    }

    let last_date = rotations
        .last()
        .map(|rotation| rotation.date_utc.clone())
        .unwrap_or_else(|| from.to_string());
    let calendar = ZenithCalendar {
        manifest: packages_path.display().to_string(),
        from_utc: from.to_string(),
        days,
        rotations,
    };

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let file = File::create(output).with_context(|| format!("creating {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &calendar)
        .with_context(|| format!("writing {}", output.display()))?;
    writeln!(writer)?;
    writer.flush()?;

    println!(
        "Wrote {} Zenith rotations ({} through {}) to {}",
        days,
        from,
        last_date,
        output.display()
    );
    Ok(())
}

fn load_inputs(packages_path: &Path, market_items_path: Option<&Path>) -> Result<Inputs> {
    let packages_file = File::open(packages_path)
        .with_context(|| format!("opening {}", packages_path.display()))?;
    let packages: Packages = serde_json::from_reader(BufReader::new(packages_file))
        .with_context(|| format!("parsing {}", packages_path.display()))?;

    let market_items_path = market_items_path
        .map(PathBuf::from)
        .or_else(|| packages_path.parent().map(|p| p.join("market_items.json")))
        .filter(|p| p.exists());
    let names = match market_items_path.as_deref() {
        Some(path) => read_market_names(path)?,
        None => HashMap::new(),
    };

    Ok(Inputs { packages, names })
}

fn read_worldstate_time(path: &Path) -> Result<u64> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let value: Value = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parsing {}", path.display()))?;
    for key in ["server_time", "Time", "time"] {
        if let Some(timestamp) = value.get(key).and_then(value_as_u64) {
            return Ok(timestamp);
        }
    }
    bail!(
        "{} does not contain a numeric server_time, Time, or time field",
        path.display()
    )
}

fn read_market_names(path: &Path) -> Result<HashMap<String, String>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let market: MarketItems = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(market
        .items
        .into_iter()
        .map(|item| (item.path, item.name))
        .collect())
}

fn calculate_rotation(inputs: &Inputs, unix_time: u64) -> Result<ZenithRotation> {
    let packages = &inputs.packages;
    let manifest = package(packages, PRODUCTS_MANIFEST)?;
    let store_items = manifest
        .attributes
        .get("StoreItems")
        .and_then(Value::as_array)
        .context("ProductsManifest has no StoreItems array")?;

    let mut melee = Vec::new();
    let mut caster = Vec::new();
    let mut ranged = Vec::new();

    for store_item in store_items.iter().filter_map(Value::as_str) {
        let Some(item) = packages.get(store_item).and_then(Option::as_ref) else {
            continue;
        };
        if item
            .attributes
            .get("ShowInMarket")
            .is_some_and(value_is_false)
        {
            continue;
        }
        let Some(type_name) = item.attributes.get("TypeName").and_then(Value::as_str) else {
            continue;
        };

        let candidate = Candidate {
            path: store_item.to_string(),
            name: inputs
                .names
                .get(store_item)
                .cloned()
                .unwrap_or_else(|| asset_name(store_item).to_string()),
            premium_price: item.attributes.get("PremiumPrice").and_then(value_as_u64),
            rarity: item
                .attributes
                .get("Rarity")
                .and_then(Value::as_str)
                .map(str::to_string),
        };

        // This order matters: caster weapons also inherit from the general
        // ranged weapon type, exactly as StoreRedux checks them.
        if is_a(packages, type_name, ONE_HAND_MELEE) || is_a(packages, type_name, TWO_HAND_MELEE) {
            melee.push(candidate);
        } else if is_a(packages, type_name, CASTER_MELEE) {
            caster.push(candidate);
        } else if is_a(packages, type_name, RANGED_MELEE) {
            ranged.push(candidate);
        }
    }

    let (day_start_unix, rng_seed) =
        daily_seed(unix_time).context("Zenith rotation dates before 2004-01-14 are unsupported")?;
    let mut rng = DeRandom::seeded(rng_seed);

    // StoreRedux calls SeededShuffleTable in this exact order. A single shared
    // global generator means changing the order changes every later result.
    rng.shuffle(&mut caster);
    rng.shuffle(&mut ranged);
    rng.shuffle(&mut melee);

    let pool_sizes = PoolSizes {
        bow_or_ranged: ranged.len(),
        melee: melee.len(),
        caster: caster.len(),
    };

    if ranged.is_empty() || melee.len() < 2 || caster.is_empty() {
        bail!(
            "eligible ProductsManifest pools are incomplete (bow/ranged={}, melee={}, caster={})",
            ranged.len(),
            melee.len(),
            caster.len()
        );
    }

    // Match the in-game presentation: bow/ranged, two melee, then caster.
    let selected = [
        (OfferCategory::BowOrRanged, &ranged[0]),
        (OfferCategory::Melee, &melee[0]),
        (OfferCategory::Melee, &melee[1]),
        (OfferCategory::Caster, &caster[0]),
    ];
    let offers = selected
        .into_iter()
        .map(|(category, item)| Offer {
            category,
            name: item.name.clone(),
            path: item.path.clone(),
            premium_price: item.premium_price,
            rarity: item.rarity.clone(),
        })
        .collect();

    Ok(ZenithRotation {
        unix_time,
        day_start_unix,
        rng_seed,
        pool_sizes,
        offers,
    })
}

fn package<'a>(packages: &'a Packages, path: &str) -> Result<&'a Package> {
    packages
        .get(path)
        .and_then(Option::as_ref)
        .with_context(|| format!("missing package {path}"))
}

fn is_a(packages: &Packages, type_name: &str, base_type: &str) -> bool {
    let mut current = Some(type_name);
    for _ in 0..128 {
        let Some(path) = current else {
            return false;
        };
        if path == base_type {
            return true;
        }
        current = packages
            .get(path)
            .and_then(Option::as_ref)
            .and_then(|entry| entry.parent.as_deref());
    }
    false
}

fn value_is_false(value: &Value) -> bool {
    matches!(value, Value::Bool(false)) || value.as_i64() == Some(0)
}

fn value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|v| *v >= 0.0).map(|v| v as u64))
        .or_else(|| value.as_str()?.parse().ok())
}

fn asset_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_rotation_seed_uses_de_epoch() {
        let unix_time = 1_786_916_650;
        let (day_start, seed) = daily_seed(unix_time).unwrap();
        assert_eq!(day_start, 1_786_838_400);
        assert_eq!(seed, 712_800_000);
    }

    #[test]
    fn equal_random_bounds_do_not_advance_state() {
        let mut rng = DeRandom::seeded(123);
        assert_eq!(rng.int_inclusive(7, 7), 7);
        assert_eq!(rng.state, 123);
    }

    #[test]
    fn captured_rotation_selects_known_candidate_indexes() {
        let mut caster: Vec<_> = (0..5).collect();
        let mut ranged: Vec<_> = (0..4).collect();
        let mut melee: Vec<_> = (0..35).collect();
        let mut rng = DeRandom::seeded(712_800_000);

        rng.shuffle(&mut caster);
        rng.shuffle(&mut ranged);
        rng.shuffle(&mut melee);

        assert_eq!(caster[0], 0); // DendritStaffCaster (Seathorn)
        assert_eq!(ranged[0], 1); // DendritBow (Maestro)
        assert_eq!(&melee[..2], &[19, 27]); // SeedRapier (Navalha), DeoraPolearm (Rook)
    }
}
