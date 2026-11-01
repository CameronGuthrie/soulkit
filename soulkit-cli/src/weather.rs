//! Midrath dynamic weather, reconstructed from the native `DynamicSky`
//! weather chooser. Weather is a pure function of server Unix time: a 2D
//! simplex noise sampled on a circle whose period is 31 days picks the tier,
//! and an integer hash of 3.3-hour buckets picks the key inside that tier.

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::BufReader,
    path::Path,
};

use anyhow::{bail, Context, Result};
use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SKY: &str = "/SF/Levels/MidrathRedux/DynamicSkyMidrath";
const CYCLE_SECONDS: f64 = 2_678_400.0;
const ANGLE_PER_SECOND: f64 = 2.345872650530013e-06;
const NOISE_RADIUS: f32 = 118.411_28;
const KEY_BUCKETS_PER_SECOND: f64 = 8.417508417508418e-05;
const TIER_NAMES: [&str; 4] = ["clear", "light", "moderate", "heavy"];
/// Hand-written player-facing names/blurbs/effects, keyed by calendar type id.
const GUIDE: &str = include_str!("../data/weather-guide.json");
const GUIDE_FILE: &str = "weather-guide.json";

/// Ken Perlin's reference permutation; the client indexes a doubled copy.
const PERM: [u8; 256] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125, 136, 171, 168, 68, 175,
    74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230,
    220, 105, 92, 41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76,
    132, 187, 208, 89, 18, 169, 200, 196, 135, 130, 116, 188, 159, 86, 164, 100, 109, 198, 173,
    186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212, 207, 206,
    59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163,
    70, 221, 153, 101, 155, 167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232,
    178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193, 238, 210, 144, 12, 191, 179, 162,
    241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
    176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141,
    128, 195, 78, 66, 215, 61, 156, 180,
];

fn perm(i: usize) -> usize {
    PERM[i & 0xff] as usize
}

/// DE's variant of 2D simplex noise: gradient hash masked with 0x3f and a
/// 45.23 output scale. All arithmetic is f32 in the same order as the client.
fn simplex(x: f32, y: f32) -> f32 {
    const F2: f32 = 0.366_025_39;
    const G2: f32 = 0.211_324_87;
    const G2X2_MINUS_1: f32 = 0.577_350_26;

    fn floor(v: f32) -> i32 {
        v as i32 - i32::from(v <= 0.0)
    }
    fn grad(hash: usize, mut u: f32, mut v: f32) -> f32 {
        let h = hash & 0x3f;
        if h >= 4 {
            std::mem::swap(&mut u, &mut v);
        }
        if h & 1 != 0 {
            u = -u;
        }
        v = if h & 2 != 0 { v * -2.0 } else { v + v };
        v + u
    }
    fn corner(x: f32, y: f32, hash: usize) -> f32 {
        let t = 0.5 - x * x - y * y;
        if t < 0.0 {
            return 0.0;
        }
        let t = t * t;
        grad(hash, x, y) * (t * t)
    }

    let s = (x + y) * F2;
    let (i, j) = (floor(s + x), floor(s + y));
    let t = (i + j) as f32 * G2;
    let x0 = x - (i as f32 - t);
    let y0 = y - (j as f32 - t);
    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let (ii, jj) = ((i & 0xff) as usize, (j & 0xff) as usize);

    let n0 = corner(x0, y0, perm(ii + perm(jj)));
    let n1 = corner(
        x0 - i1 as f32 + G2,
        y0 - j1 as f32 + G2,
        perm(ii + i1 + perm(jj + j1)),
    );
    let n2 = corner(
        x0 - G2X2_MINUS_1,
        y0 - G2X2_MINUS_1,
        perm(ii + 1 + perm(jj + 1)),
    );
    (n1 + n0 + n2) * 45.230_652
}

/// Returns (tier 1..=4, key index within the tier) for a server Unix time.
fn choose(unix_time: f64, key_counts: [usize; 4]) -> (usize, usize) {
    let angle = (unix_time % CYCLE_SECONDS) * ANGLE_PER_SECOND;
    let noise = simplex(
        angle.cos() as f32 * NOISE_RADIUS,
        angle.sin() as f32 * NOISE_RADIUS,
    );
    let v = (noise + 1.0) * 0.5;
    let v = f64::from(v).powf(1.25) as f32;
    let tier = if v < 0.25 {
        1
    } else if v < 0.5 {
        2
    } else if v < 0.75 {
        3
    } else {
        4
    };

    let mut h = ((unix_time * KEY_BUCKETS_PER_SECOND) as i64 as u32)
        ^ (tier as u32).wrapping_mul(0x9e37_79b1);
    h = ((h >> 16) ^ h).wrapping_mul(0x045d_9f3b);
    h ^= h >> 16;
    let count = key_counts[tier - 1];
    (tier, if count == 0 { 0 } else { h as usize % count })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WeatherKey {
    #[serde(rename = "overcastAmount", default)]
    overcast: f64,
    #[serde(rename = "windAmount", default)]
    wind: f64,
    #[serde(default)]
    rain: f64,
    #[serde(default)]
    lightning: f64,
    #[serde(rename = "rainbowChance", default)]
    rainbow_chance: f64,
}

#[derive(Debug, Serialize)]
struct WeatherType {
    tier: &'static str,
    key_index: usize,
    conditions: Vec<&'static str>,
    key: WeatherKey,
}

fn conditions(k: &WeatherKey) -> Vec<&'static str> {
    let mut c = Vec::new();
    if k.rain > 0.0 {
        c.push("rain");
    }
    if k.lightning > 0.0 {
        c.push("lightning");
    }
    if k.rainbow_chance > 0.0 {
        c.push("rainbow_chance");
    }
    if k.overcast >= 0.5 {
        c.push("overcast");
    } else if k.overcast > 0.0 {
        c.push("light_cloud");
    }
    if k.wind >= 1.0 {
        c.push("strong_wind");
    } else if k.wind > 0.0 {
        c.push("breeze");
    }
    if c.is_empty() {
        c.push("calm");
    }
    c
}

/// `calendar` maps each UTC date to `"type:unix_seconds"` entries. The first
/// entry of every day is the weather in effect at 00:00:00; each later entry
/// is an exact (to the second) change. `types` is the legend for `type`.
#[derive(Debug, Serialize)]
struct Calendar {
    sky: &'static str,
    method: &'static str,
    note: &'static str,
    from_utc: String,
    days: u32,
    types: BTreeMap<String, WeatherType>,
    calendar: BTreeMap<String, Vec<String>>,
}

fn load_tiers(packages_path: &Path) -> Result<[Vec<WeatherKey>; 4]> {
    let file = File::open(packages_path)
        .with_context(|| format!("opening {}", packages_path.display()))?;
    let packages: HashMap<String, Value> = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parsing {}", packages_path.display()))?;
    let weather = packages
        .get(SKY)
        .and_then(|p| p.pointer("/attributes/DynamicWeather"))
        .with_context(|| format!("{SKY} has no DynamicWeather block"))?;
    let tier = |name: &str| -> Result<Vec<WeatherKey>> {
        serde_json::from_value(weather.get(name).cloned().unwrap_or(Value::Null))
            .with_context(|| format!("reading {name}"))
    };
    Ok([
        tier("ClearWeatherKeys")?,
        tier("LightWeatherKeys")?,
        tier("ModerateWeatherKeys")?,
        tier("HeavyWeatherKeys")?,
    ])
}

fn type_id(tier: usize, key_index: usize) -> String {
    format!("{}.{key_index}", TIER_NAMES[tier - 1])
}

pub fn run_calendar(packages_path: &Path, from: &str, days: u32, output: &Path) -> Result<()> {
    if days == 0 {
        bail!("calendar days must be at least 1");
    }
    let first = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .with_context(|| format!("invalid --from date {from:?}; expected YYYY-MM-DD"))?;
    let tiers = load_tiers(packages_path)?;
    let counts = tiers.each_ref().map(Vec::len);

    let mut types = BTreeMap::new();
    for (t, keys) in tiers.iter().enumerate() {
        for (i, key) in keys.iter().enumerate() {
            types.insert(
                type_id(t + 1, i),
                WeatherType {
                    tier: TIER_NAMES[t],
                    key_index: i,
                    conditions: conditions(key),
                    key: key.clone(),
                },
            );
        }
    }

    // ponytail: scan per minute, then bisect each change to the second;
    // a sub-minute flicker right on a threshold could be missed.
    const STEP: i64 = 60;
    let mut calendar = BTreeMap::new();
    let mut changes = 0;
    for d in 0..days {
        let date = first
            .checked_add_days(Days::new(u64::from(d)))
            .context("calendar date overflow")?;
        let day_start = date
            .and_hms_opt(0, 0, 0)
            .context("invalid date")?
            .and_utc()
            .timestamp();
        let day_end = day_start + 86_400;
        let mut current = choose(day_start as f64, counts);
        let mut entries = vec![format!("{}:{day_start}", type_id(current.0, current.1))];
        let mut t = day_start;
        while t < day_end {
            let next = (t + STEP).min(day_end - 1);
            if choose(next as f64, counts) != current {
                let (mut lo, mut hi) = (t, next);
                while hi - lo > 1 {
                    let mid = (lo + hi) / 2;
                    if choose(mid as f64, counts) == current {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                current = choose(hi as f64, counts);
                entries.push(format!("{}:{hi}", type_id(current.0, current.1)));
                changes += 1;
                t = hi;
            } else {
                t += STEP;
            }
        }
        calendar.insert(date.format("%Y-%m-%d").to_string(), entries);
    }

    let out = Calendar {
        sky: SKY,
        method:
            "native DynamicSky chooser: simplex(cos,sin of 31-day cycle) tiers, 11880 s key buckets",
        note: "Target weather; the client blends into each change over a few seconds. \
               Quests and scripts can pause the cycle locally.",
        from_utc: from.to_string(),
        days,
        types,
        calendar,
    };
    let dir = output.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    serde_json::to_writer_pretty(
        std::io::BufWriter::new(
            File::create(output).with_context(|| format!("creating {}", output.display()))?,
        ),
        &out,
    )?;
    let guide = dir.map_or_else(
        || Path::new(GUIDE_FILE).to_path_buf(),
        |d| d.join(GUIDE_FILE),
    );
    std::fs::write(&guide, GUIDE).with_context(|| format!("writing {}", guide.display()))?;
    println!(
        "Wrote {changes} weather changes over {days} day(s) from {from} to {} (+ {})",
        output.display(),
        guide.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (unix_time, tier, key) produced by running the client's own chooser
    /// (0x140ac0430, build of 2026-09-24) under an emulator.
    const GOLDEN: [(i64, usize, usize); 24] = [
        (1793464097, 4, 1),
        (1770246633, 2, 1),
        (1802992312, 3, 0),
        (1837366946, 3, 2),
        (1756480894, 2, 1),
        (1759722233, 1, 0),
        (1821924865, 4, 1),
        (1762633920, 1, 0),
        (1799081935, 2, 0),
        (1828220482, 4, 1),
        (1757784483, 1, 0),
        (1818106871, 3, 1),
        (1778816302, 1, 0),
        (1755032582, 3, 0),
        (1761535642, 1, 0),
        (1808202938, 4, 1),
        (1806126116, 4, 0),
        (1759375836, 2, 1),
        (1782301241, 3, 1),
        (1762175294, 2, 1),
        (1823960310, 3, 2),
        (1806978001, 2, 0),
        (1757933677, 2, 0),
        (1825893910, 3, 0),
    ];

    #[test]
    fn guide_covers_every_type() {
        let guide: Value = serde_json::from_str(GUIDE).expect("weather-guide.json parses");
        let ids: Vec<&String> = guide["types"]
            .as_object()
            .expect("types object")
            .keys()
            .collect();
        let expected: Vec<String> = [1, 2, 2, 3, 3, 3, 4, 4]
            .iter()
            .zip([0, 0, 1, 0, 1, 2, 0, 1])
            .map(|(&t, k)| type_id(t, k))
            .collect();
        let mut expected: Vec<&String> = expected.iter().collect();
        expected.sort();
        assert_eq!(
            ids, expected,
            "guide types must match the calendar's Midrath presets"
        );
        for (id, t) in guide["types"].as_object().unwrap() {
            for e in t["effects"].as_array().unwrap() {
                let eid = e["id"].as_str().unwrap();
                assert!(
                    guide["effects"].get(eid).is_some(),
                    "{id} references unknown effect {eid}"
                );
            }
        }
    }

    #[test]
    fn matches_client_chooser() {
        for (t, tier, key) in GOLDEN {
            assert_eq!(choose(t as f64, [1, 2, 3, 2]), (tier, key), "t={t}");
        }
    }
}
