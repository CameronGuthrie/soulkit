//! Parse decrypted Soulframe drop-table binaries into structured JSON.
//!
//! Pool-boundary detection and category extraction retain the original
//! binary parser's heuristics, documented alongside the implementation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const RARITY_NAMES: &[(u32, &str)] = &[
    (0, "Common"),
    (1, "Uncommon"),
    (2, "Rare"),
    (3, "Legendary"),
];

pub const RARITY_WEIGHTS: &[(u32, f64)] = &[(0, 0.755), (1, 0.22), (2, 0.02), (3, 0.005)];

fn rarity_name(tier: u32) -> String {
    for (t, n) in RARITY_NAMES {
        if *t == tier {
            return (*n).to_string();
        }
    }
    format!("Tier{}", tier)
}

fn rarity_weight(tier: u32) -> f64 {
    for (t, w) in RARITY_WEIGHTS {
        if *t == tier {
            return *w;
        }
    }
    0.755
}

#[derive(Debug, Clone, Default)]
struct RawItem {
    offset: usize,
    end: usize,
    path: String,
    name: String,
    f1: f32,
    tier: u32,
    f2: f32,
    probability_weight_used: f32,
    level_range: Option<[u32; 2]>,
    kind: Option<u32>,
    qualities: Vec<u32>,
    group_value: Option<f32>,
}

#[derive(Debug, Clone)]
struct ManifestEntry {
    path: String,
    #[allow(dead_code)]
    name: String,
    tier: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantity {
    pub min: u64,
    pub max: u64,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pickup_item_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_item_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bonus_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuantitySummary {
    pub enriched_items: usize,
    pub explicit_items: usize,
    pub defaulted_items: usize,
    pub missing_attrs_items: usize,
    pub source_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Item {
    pub path: String,
    pub name: String,
    pub rarity: String,
    /// Probability after the containing pool's drop chance is applied.
    /// `None` means the pool gate could not be recovered from the binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_probability_per_table_roll: Option<f64>,
    /// Conditional probability of this item when its containing pool rolls.
    pub probability: f64,
    pub bias: f64,
    /// Inclusive `[min, max]` level at which this entry can drop. Items are
    /// grouped inside a pool and each group carries its own range; entries
    /// outside the current level are skipped before the pool's weights apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_range: Option<[u32; 2]>,
    /// Chance per level band, for pools whose entries are level-gated. At a
    /// given level only in-range entries compete, so the odds shift between
    /// bands; `probability` above is the ungated value. Bands where this item
    /// cannot drop are omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability_by_level: Option<Vec<LevelProbability>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<Quantity>,
    /// Qualities this entry may roll at, when it narrows its pool's list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualities: Option<Vec<String>>,
    /// Leading `u32` of the entry: 0 for most pickups, 1 for some items and
    /// skins, 2 for store items such as Kintsugi upgrades. Meaning unconfirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_kind: Option<u32>,
    /// Float carried by flagged groups (e.g. 1.0, 0.143, 0.0726). Meaning
    /// unconfirmed; it does not act as the group's own drop chance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_value: Option<f64>,
}

/// Slot-level settings recovered by the strict parser.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolDetails {
    /// Drop category (`DROP_WEAPON`, `DROP_MOD`, ...) of this slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_drops: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_drops: Option<u32>,
    /// Qualities a dropped item may roll at (`IQ_WORN` ... `IQ_LEGENDARY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_qualities: Option<Vec<String>>,
    /// Item the slot requires before it can roll (e.g. the Cogah elixir,
    /// `RedNightUpgrade`); an empty string is a requirement with no item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelProbability {
    /// Inclusive `[from, to]` levels this probability applies to.
    pub levels: [u32; 2],
    pub probability: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_probability_per_table_roll: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub drop_chance: Option<f64>,
    #[serde(flatten)]
    pub details: PoolDetails,
    pub item_count: usize,
    pub probability_sum: f64,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedTable {
    pub table_name: String,
    pub parser_mode: String,
    pub pool_count: usize,
    pub total_items: usize,
    pub pools: Vec<Pool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropTableReferenceSite {
    pub table_name: String,
    pub pool_index: usize,
    pub item_index: usize,
    pub drop_chance: Option<f64>,
    pub probability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropTableReference {
    pub path: String,
    pub table_name: String,
    pub resolved: bool,
    pub referenced_by: Vec<DropTableReferenceSite>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDropTableItem {
    pub offset: usize,
    pub end: usize,
    pub path: String,
    pub name: String,
    pub f1: f32,
    pub tier: u32,
    pub f2: f32,
    pub probability_weight_used: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_range: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub qualities: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_value: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawManifestItem {
    pub path: String,
    pub name: String,
    pub tier: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDropTablePool {
    pub drop_chance: Option<f64>,
    #[serde(flatten)]
    pub details: PoolDetails,
    pub item_count: usize,
    pub items: Vec<RawDropTableItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawParsedTable {
    pub table_name: String,
    pub parser_mode: String,
    pub pool_count: usize,
    pub total_items: usize,
    pub pools: Vec<RawDropTablePool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manifest_items: Vec<RawManifestItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDropTableEntry {
    pub source_file: String,
    pub table: RawParsedTable,
}

// ----- helpers -----

fn read_u32_le(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn read_f32_le(buf: &[u8], off: usize) -> Option<f32> {
    buf.get(off..off + 4)
        .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
}

fn round6(x: f64) -> f64 {
    (x * 1_000_000.0).round() / 1_000_000.0
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

// ----- quantity enrichment -----

#[derive(Debug, Clone, Deserialize)]
struct PackageAttrsEntry {
    #[serde(default)]
    #[allow(dead_code)]
    path: Option<String>,
    parent: String,
    attributes: String,
}

#[derive(Debug, Clone)]
struct PackageAttrsIndex {
    entries: BTreeMap<String, PackageAttrsEntry>,
    by_leaf: BTreeMap<String, Vec<String>>,
}

impl PackageAttrsIndex {
    fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).with_context(|| format!("Reading {:?}", path))?;
        let entries: BTreeMap<String, PackageAttrsEntry> =
            serde_json::from_str(&raw).context("Parsing packages_bin_attrs.json")?;
        Ok(Self::new(entries))
    }

    fn new(entries: BTreeMap<String, PackageAttrsEntry>) -> Self {
        let mut by_leaf: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in entries.keys() {
            let leaf = path.rsplit('/').next().unwrap_or(path).to_string();
            by_leaf.entry(leaf).or_default().push(path.clone());
        }
        Self { entries, by_leaf }
    }

    fn quantity_for_path(&self, path: &str) -> Quantity {
        let Some(entry) = self.entries.get(path) else {
            return default_quantity("default_missing_attrs", None, None, None);
        };

        if let Some((min, max)) = parse_item_amount_override(&entry.attributes) {
            return Quantity {
                min,
                max,
                source: "item_amount_override".to_string(),
                pickup_item_type: None,
                resolved_item_path: None,
                bonus_type: None,
            };
        }

        if let Some((bonus_type, min, max)) = parse_bonus_quantity(&entry.attributes) {
            return Quantity {
                min,
                max,
                source: "item_bonus".to_string(),
                pickup_item_type: None,
                resolved_item_path: None,
                bonus_type: Some(bonus_type),
            };
        }

        if let Some(item_type) = capture_attr_value(&entry.attributes, "PickUpItemType=") {
            if let Some((resolved_path, resolved_entry)) = self.resolve_item_type(item_type) {
                if let Some((bonus_type, min, max)) =
                    parse_bonus_quantity(&resolved_entry.attributes)
                {
                    return Quantity {
                        min,
                        max,
                        source: "pickup_item_bonus".to_string(),
                        pickup_item_type: Some(item_type.to_string()),
                        resolved_item_path: Some(resolved_path.to_string()),
                        bonus_type: Some(bonus_type),
                    };
                }
            }

            return default_quantity("default_pickup", Some(item_type.to_string()), None, None);
        }

        let _ = &entry.parent;
        default_quantity("default_item", None, None, None)
    }

    fn resolve_item_type(&self, item_type: &str) -> Option<(&str, &PackageAttrsEntry)> {
        if item_type.starts_with('/') {
            if let Some((path, entry)) = self.entries.get_key_value(item_type) {
                return Some((path.as_str(), entry));
            }
        }

        let leaf = item_type.rsplit('/').next().unwrap_or(item_type);
        let paths = self.by_leaf.get(leaf)?;

        for path in paths {
            let entry = self.entries.get(path)?;
            if entry.attributes.contains("Bonuses=") {
                return Some((path.as_str(), entry));
            }
        }

        let path = paths.first()?;
        self.entries.get(path).map(|entry| (path.as_str(), entry))
    }
}

fn default_quantity(
    source: &str,
    pickup_item_type: Option<String>,
    resolved_item_path: Option<String>,
    bonus_type: Option<String>,
) -> Quantity {
    Quantity {
        min: 1,
        max: 1,
        source: source.to_string(),
        pickup_item_type,
        resolved_item_path,
        bonus_type,
    }
}

fn capture_attr_value<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let start = attrs.find(key)? + key.len();
    let tail = attrs[start..].trim_start();
    if tail.starts_with('{') {
        let end = tail.find('}')? + 1;
        return Some(&tail[..end]);
    }
    let end = tail
        .find(|c: char| c.is_whitespace() || c == ',' || c == '\0')
        .unwrap_or(tail.len());
    Some(tail[..end].trim_matches('"'))
}

fn parse_item_amount_override(attrs: &str) -> Option<(u64, u64)> {
    let raw = capture_attr_value(attrs, "ItemAmountOverride=")?;
    parse_amount_range(raw)
}

fn parse_amount_range(raw: &str) -> Option<(u64, u64)> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut parts = inner.split(',').map(str::trim);
        let min = parse_amount_number(parts.next()?)?;
        let max = parse_amount_number(parts.next()?)?;
        return Some((min, max));
    }
    let n = parse_amount_number(raw)?;
    Some((n, n))
}

fn parse_amount_number(raw: &str) -> Option<u64> {
    let raw = raw.trim().trim_matches('"').trim_end_matches(',');
    raw.parse::<u64>().ok().or_else(|| {
        let n = raw.parse::<f64>().ok()?;
        (n >= 0.0 && n.fract().abs() < f64::EPSILON).then_some(n as u64)
    })
}

fn parse_bonus_quantity(attrs: &str) -> Option<(String, u64, u64)> {
    let mut bonus_type: Option<String> = None;
    let mut value: Option<u64> = None;
    let mut value_max: Option<u64> = None;

    for line in attrs.lines().map(str::trim) {
        if let Some(raw) = line.strip_prefix("bonusType=") {
            bonus_type = Some(raw.trim().trim_matches('"').to_string());
        } else if let Some(raw) = line.strip_prefix("valueMax=") {
            value_max = parse_amount_number(raw);
        } else if let Some(raw) = line.strip_prefix("value=") {
            value = parse_amount_number(raw);
        }

        if bonus_type.is_some() && value.is_some() && value_max.is_some() {
            break;
        }
    }

    let min = value?;
    let max = value_max.unwrap_or(min);
    Some((
        bonus_type.unwrap_or_else(|| "unknown".to_string()),
        min,
        max,
    ))
}

fn enrich_quantities(results: &mut [ParsedTable], attrs: &PackageAttrsIndex) -> QuantitySummary {
    let mut summary = QuantitySummary::default();

    for item in results
        .iter_mut()
        .flat_map(|table| table.pools.iter_mut())
        .flat_map(|pool| pool.items.iter_mut())
    {
        let quantity = attrs.quantity_for_path(&item.path);
        summary.enriched_items += 1;
        *summary
            .source_counts
            .entry(quantity.source.clone())
            .or_insert(0) += 1;
        match quantity.source.as_str() {
            "item_amount_override" | "item_bonus" | "pickup_item_bonus" => {
                summary.explicit_items += 1;
            }
            "default_missing_attrs" => {
                summary.defaulted_items += 1;
                summary.missing_attrs_items += 1;
            }
            _ => summary.defaulted_items += 1,
        }
        item.quantity = Some(quantity);
    }

    summary
}

// ----- item extraction -----

/// Items inside a pool are grouped; each group starts with
/// `u8 flag, f32 min_level, f32 max_level, u32 item_count`, directly before
/// the first item's leading `u32` and path length. Inside a group the same
/// bytes are the previous item's `f1, tier, f2`, which never pass this check.
fn group_level_range(obj: &[u8], len_at: usize) -> Option<[u32; 2]> {
    let min = read_f32_le(obj, len_at.checked_sub(16)?)?;
    let max = read_f32_le(obj, len_at - 12)?;
    let count = read_u32_le(obj, len_at - 8)?;
    let whole = |v: f32| (0.0..=1000.0).contains(&v) && v.fract() == 0.0;
    (whole(min) && whole(max) && min <= max && max > 0.0 && (1..=500).contains(&count))
        .then_some([min as u32, max as u32])
}

fn find_valid_items(obj: &[u8]) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut level_range = None;
    let mut i = 0usize;
    while i + 4 < obj.len() {
        let slen = match read_u32_le(obj, i) {
            Some(v) => v as usize,
            None => break,
        };
        if (10..=500).contains(&slen) && i + 4 + slen + 16 <= obj.len() {
            let s_bytes = &obj[i + 4..i + 4 + slen];
            if let Ok(s) = std::str::from_utf8(s_bytes) {
                let valid = (s.contains("/SF/") || s.contains("/Lotus/"))
                    && s.chars().all(|c| (32..127).contains(&(c as u32)));
                if valid {
                    let dstart = i + 4 + slen;
                    let f1 = read_f32_le(obj, dstart).unwrap();
                    let u1 = read_u32_le(obj, dstart + 4).unwrap();
                    let f2 = read_f32_le(obj, dstart + 8).unwrap();
                    let weight_ok = (f2 - 0.755).abs() < 0.01
                        || (f2 - 0.22).abs() < 0.01
                        || (f2 - 0.02).abs() < 0.01
                        || (f2 - 0.005).abs() < 0.001;
                    // Some items (e.g. RedNightUpgrade) encode a non-standard value in the
                    // weight field despite having a valid tier. Accept them and substitute
                    // the canonical weight so probability math stays correct.
                    let valid_tier_fallback = u1 < 4 && f2.abs() > 0.01;
                    if weight_ok || valid_tier_fallback {
                        let effective_f2 = if weight_ok {
                            f2
                        } else {
                            rarity_weight(u1) as f32
                        };
                        let name = s.rsplit('/').next().unwrap_or(s).to_string();
                        if let Some(range) = group_level_range(obj, i) {
                            level_range = Some(range);
                        }
                        items.push(RawItem {
                            offset: i,
                            end: dstart + 16,
                            path: s.to_string(),
                            name,
                            f1,
                            tier: u1,
                            f2,
                            probability_weight_used: effective_f2,
                            level_range,
                            kind: None,
                            qualities: Vec::new(),
                            group_value: None,
                        });
                        i = dstart + 16;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    items
}

fn find_manifest_items(obj: &[u8], table_name: &str) -> Vec<ManifestEntry> {
    let mut found = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let is_item_manifest = table_name.ends_with("ItemManifest")
        || table_name.ends_with("SyndicateManifest")
        || table_name == "SF/Syndicates/SilentRoseManifest";

    let mut i = 0usize;
    while i + 4 < obj.len() {
        let slen = match read_u32_le(obj, i) {
            Some(v) => v as usize,
            None => break,
        };
        if (8..=500).contains(&slen) && i + 4 + slen <= obj.len() {
            let s_bytes = &obj[i + 4..i + 4 + slen];
            if let Ok(s) = std::str::from_utf8(s_bytes) {
                let valid = (s.contains("/SF/") || s.contains("/Lotus/"))
                    && s.chars().all(|c| (32..127).contains(&(c as u32)));
                if valid && !seen.contains(s) {
                    let mut tier: Option<u32> = None;
                    let post_start = i + 4 + slen;
                    if post_start + 4 <= obj.len() {
                        let a = read_u32_le(obj, post_start).unwrap();
                        let is_known_tier = a < 4; // 0..=3 are valid tiers
                        if is_known_tier {
                            if post_start + 20 <= obj.len() {
                                let b = read_u32_le(obj, post_start + 4).unwrap();
                                let c = read_u32_le(obj, post_start + 8).unwrap();
                                let d = read_u32_le(obj, post_start + 12).unwrap();
                                let e = read_u32_le(obj, post_start + 16).unwrap();
                                if (b, c, d, e) == (0, 0, 1, 1) {
                                    tier = Some(a);
                                } else if is_item_manifest && b <= 1 && c <= 1 && d <= 1 && e <= 1 {
                                    tier = Some(a);
                                }
                            } else if is_item_manifest {
                                tier = Some(a);
                            }
                        }
                    }
                    seen.insert(s.to_string());
                    let name = s.rsplit('/').next().unwrap_or(s).to_string();
                    found.push(ManifestEntry {
                        path: s.to_string(),
                        name,
                        tier,
                    });
                    i += 4 + slen;
                    continue;
                }
            }
        }
        i += 1;
    }
    found
}

// ----- strict slot parser -----
//
// A drop-table object (after the 12-byte prefix) is a 4-byte header followed
// by one slot per drop category, in `DROP_CATEGORIES` order. Empty slots are
// still present and carry their own drop chance. Layout, little-endian:
//
//   slot  := 3 bytes, f32 drop_chance, u32 min_drops, u32 max_drops,
//            [u32 n, u32 quality * n if 1 <= n <= 16], u32 cond_kind,
//            [u32 len, cond_path if cond_kind != 0],
//            u32 n_groups, group * n_groups
//   group := u8 flag, [u32, u32, f32 group_value, u32 if flag == 1],
//            f32 min_level, f32 max_level, u32 n_items, item * n_items
//   item  := u32 kind, u32 len, path, [u32 n, u32 quality * n if 1 <= n <= 16],
//            f32 bias, u32 tier, f32 weight

/// Drop category of each slot, from the table's `DefaultPickUps` trailer order.
pub const DROP_CATEGORIES: [&str; 8] = [
    "DROP_AMMO",
    "DROP_BLUEPRINT",
    "DROP_MISC_ITEM",
    "DROP_MOD",
    "DROP_POWERUP",
    "DROP_SIGIL",
    "DROP_FUSION_ORNAMENT",
    "DROP_WEAPON",
];

/// `AllowedQualities` values by index.
const QUALITY_NAMES: [&str; 7] = [
    "IQ_NONE",
    "IQ_WORN",
    "IQ_MILITARY",
    "IQ_OFFICER",
    "IQ_NOBLE",
    "IQ_MASTERWORK",
    "IQ_LEGENDARY",
];

fn quality_names(qualities: &[u32]) -> Vec<String> {
    qualities
        .iter()
        .map(|&q| {
            QUALITY_NAMES
                .get(q as usize)
                .map_or_else(|| format!("IQ_{q}"), |n| (*n).to_string())
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Slot {
    category: &'static str,
    drop_chance: f32,
    min_drops: u32,
    max_drops: u32,
    qualities: Vec<u32>,
    /// Path of the item this slot requires (condition kind 61), `""` when the
    /// condition names no item.
    requires: Option<String>,
    items: Vec<RawItem>,
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = read_u32_le(self.buf, self.pos)?;
        self.pos += 4;
        Some(v)
    }
    fn f32(&mut self) -> Option<f32> {
        let v = read_f32_le(self.buf, self.pos)?;
        self.pos += 4;
        Some(v)
    }
    fn path(&mut self, len: usize) -> Option<String> {
        let s = std::str::from_utf8(self.buf.get(self.pos..self.pos + len)?).ok()?;
        s.chars().all(|c| (' '..='~').contains(&c)).then_some(())?;
        self.pos += len;
        Some(s.to_string())
    }
    fn qualities(&mut self, n: u32) -> Option<Vec<u32>> {
        (1..=16)
            .contains(&n)
            .then(|| (0..n).map(|_| self.u32()).collect::<Option<Vec<_>>>())?
    }
}

fn parse_slot(c: &mut Cursor, category: &'static str) -> Option<Slot> {
    c.pos += 3;
    let drop_chance = c.f32()?;
    let min_drops = c.u32()?;
    let max_drops = c.u32()?;
    // Optional quality list, then the condition kind. Builds before quality
    // lists existed go straight to the condition kind.
    let mut cond_kind = c.u32()?;
    let mut qualities = Vec::new();
    if (1..=16).contains(&cond_kind) {
        qualities = c.qualities(cond_kind)?;
        cond_kind = c.u32()?;
    }
    let mut requires = None;
    if cond_kind != 0 {
        let len = c.u32()? as usize;
        requires = Some(if len == 0 {
            String::new()
        } else {
            c.path(len)?
        });
    }
    let n_groups = c.u32()?;
    if n_groups > 64 {
        return None;
    }
    let mut items = Vec::new();
    for _ in 0..n_groups {
        let group_value = match c.u8()? {
            0 => None,
            1 => {
                c.pos += 8;
                let v = c.f32()?;
                c.pos += 4;
                Some(v)
            }
            _ => return None,
        };
        let (lo, hi, n_items) = (c.f32()?, c.f32()?, c.u32()?);
        let whole = |v: f32| (0.0..=1000.0).contains(&v) && v.fract() == 0.0;
        if !(whole(lo) && whole(hi) && lo <= hi && n_items <= 1000) {
            return None;
        }
        for _ in 0..n_items {
            let offset = c.pos;
            let kind = c.u32()?;
            let len = c.u32()? as usize;
            if !(1..400).contains(&len) {
                return None;
            }
            let path = c.path(len)?;
            let peek = read_u32_le(c.buf, c.pos)?;
            let item_qualities = if (1..=16).contains(&peek) {
                c.pos += 4;
                c.qualities(peek)?
            } else {
                Vec::new()
            };
            let f1 = c.f32()?;
            let tier = c.u32()?;
            let f2 = c.f32()?;
            let weight_ok = f2.is_finite() && f2 > 0.0;
            items.push(RawItem {
                offset,
                end: c.pos,
                name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                path,
                f1,
                tier,
                f2,
                probability_weight_used: if weight_ok {
                    f2
                } else {
                    rarity_weight(tier) as f32
                },
                level_range: Some([lo as u32, hi as u32]),
                kind: Some(kind),
                qualities: item_qualities,
                group_value,
            });
        }
    }
    Some(Slot {
        category,
        drop_chance,
        min_drops,
        max_drops,
        qualities,
        requires,
        items,
    })
}

/// Every category slot of a drop table, or `None` when the bytes don't follow
/// the layout exactly (the caller then falls back to the heuristic parser).
fn parse_slots(obj: &[u8]) -> Option<Vec<Slot>> {
    let mut c = Cursor { buf: obj, pos: 4 };
    let slots = DROP_CATEGORIES
        .iter()
        .map(|category| parse_slot(&mut c, category))
        .collect::<Option<Vec<_>>>()?;
    // The table's trailer starts with the length-prefixed StrippedItemType path.
    let len = read_u32_le(obj, c.pos)? as usize;
    (obj.get(c.pos + 4) == Some(&b'/') && (1..400).contains(&len)).then_some(slots)
}

fn slot_pool_fields(slot: &Slot) -> PoolDetails {
    PoolDetails {
        category: Some(slot.category.to_string()),
        min_drops: Some(slot.min_drops),
        max_drops: Some(slot.max_drops),
        allowed_qualities: (!slot.qualities.is_empty()).then(|| quality_names(&slot.qualities)),
        requires: slot.requires.clone(),
    }
}

// ----- pool boundary / dc / ic -----

fn is_pool_boundary(obj: &[u8], gap_start: usize, gap_end: usize) -> bool {
    let gap_size = gap_end.saturating_sub(gap_start);
    if gap_size < 36 {
        return false;
    }
    let f100_offset = gap_size - 12;
    let f100_abs = gap_start + f100_offset;
    if f100_abs + 4 > obj.len() {
        return false;
    }
    let val = read_f32_le(obj, f100_abs).unwrap();
    (val - 100.0).abs() < 0.01
}

fn extract_pool_dc(obj: &[u8], gap_start: usize, gap_size: usize, is_first: bool) -> Option<f64> {
    if is_first {
        if gap_start + 4 <= obj.len() {
            if let Some(dc) = read_f32_le(obj, 7) {
                let dc = dc as f64;
                if (0.001..=1.0).contains(&dc) {
                    return Some(round6(dc));
                }
            }
        }
        return None;
    }

    if gap_size < 36 {
        return None;
    }

    // Primary for large-format gaps (>= 59 bytes): dc at offset 22 in gap.
    let dc_pos = gap_start + 22;
    if dc_pos + 4 <= obj.len() {
        if let Some(dc) = read_f32_le(obj, dc_pos) {
            let dc = dc as f64;
            if (0.001..=1.0).contains(&dc) {
                return Some(round6(dc));
            }
        }
    }

    // Compact (36 / 52 byte) gaps, or large gap with invalid offset-22
    // (e.g. embedded path): dc straddles via prev item's u2.
    if gap_start >= 1 {
        let dc_pos = gap_start - 1;
        if let Some(dc) = read_f32_le(obj, dc_pos) {
            let dc = dc as f64;
            if (0.001..=1.0).contains(&dc) {
                return Some(round6(dc));
            }
        }
    }

    None
}

fn extract_pool_ic(obj: &[u8], gap_start: usize, gap_size: usize) -> Option<u32> {
    let ic_pos = gap_start + gap_size.checked_sub(8)?;
    read_u32_le(obj, ic_pos)
}

// ----- probability calc -----

fn build_uniform_items(paths: &[String], rarity: &str) -> Vec<Item> {
    let n = paths.len() as f64;
    let probability = round6(1.0 / n);
    let mut items = Vec::with_capacity(paths.len());
    for (idx, path) in paths.iter().enumerate() {
        let prob = if idx < paths.len() - 1 {
            probability
        } else {
            round6(1.0 - probability * (paths.len() as f64 - 1.0))
        };
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        items.push(Item {
            path: path.clone(),
            name,
            rarity: rarity.to_string(),
            effective_probability_per_table_roll: None,
            probability: prob,
            bias: 0.0,
            level_range: None,
            probability_by_level: None,
            quantity: None,
            qualities: None,
            entry_kind: None,
            group_value: None,
        });
    }
    items
}

fn build_weighted_manifest_pools(manifest_entries: &[ManifestEntry]) -> (Vec<Pool>, usize) {
    let mut tier_groups: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for e in manifest_entries {
        let t = e.tier.unwrap_or(0);
        tier_groups.entry(t).or_default().push(e.path.clone());
    }

    let present_weights: BTreeMap<u32, f64> = tier_groups
        .keys()
        .map(|t| (*t, rarity_weight(*t)))
        .collect();
    let total_weight: f64 = present_weights.values().sum::<f64>().max(f64::MIN_POSITIVE);

    let mut pools = Vec::new();
    let mut total_items = 0usize;
    for (tier, paths) in &tier_groups {
        let rarity = rarity_name(*tier);
        let mut items = build_uniform_items(paths, &rarity);
        let dc = round6(present_weights[tier] / total_weight);
        for item in &mut items {
            item.effective_probability_per_table_roll = Some(round6(dc * item.probability));
        }
        total_items += items.len();
        let psum: f64 = items.iter().map(|i| i.probability).sum();
        pools.push(Pool {
            drop_chance: Some(dc),
            details: PoolDetails::default(),
            item_count: items.len(),
            probability_sum: round4(psum),
            items,
        });
    }
    (pools, total_items)
}

fn raw_item_output(it: &RawItem) -> RawDropTableItem {
    RawDropTableItem {
        offset: it.offset,
        end: it.end,
        path: it.path.clone(),
        name: it.name.clone(),
        f1: it.f1,
        tier: it.tier,
        f2: it.f2,
        probability_weight_used: it.probability_weight_used,
        level_range: it.level_range,
        kind: it.kind,
        qualities: it.qualities.clone(),
        group_value: it.group_value,
    }
}

fn manifest_item_output(it: &ManifestEntry) -> RawManifestItem {
    RawManifestItem {
        path: it.path.clone(),
        name: it.name.clone(),
        tier: it.tier,
    }
}

/// Chance of each item when its pool rolls: every rarity tier present gets
/// its weight's share of the pool, split evenly among that tier's items.
fn tier_probabilities(items: &[&RawItem]) -> Vec<f64> {
    let mut tier_counts: BTreeMap<u32, usize> = BTreeMap::new();
    let mut tier_weights: BTreeMap<u32, f64> = BTreeMap::new();
    for it in items {
        *tier_counts.entry(it.tier).or_insert(0) += 1;
        tier_weights
            .entry(it.tier)
            .or_insert(it.probability_weight_used as f64);
    }
    let total_weight: f64 = tier_weights.values().sum();
    items
        .iter()
        .map(|it| {
            if total_weight == 0.0 {
                0.0
            } else {
                tier_weights[&it.tier] / total_weight / tier_counts[&it.tier] as f64
            }
        })
        .collect()
}

fn calc_tier_probabilities(items: &[RawItem]) -> Vec<Item> {
    let refs: Vec<&RawItem> = items.iter().collect();
    items
        .iter()
        .zip(tier_probabilities(&refs))
        .map(|(it, prob)| {
            let bias = if (it.f1.abs() as f64) > 0.0001 {
                round4(it.f1 as f64)
            } else {
                0.0
            };
            Item {
                path: it.path.clone(),
                name: it.name.clone(),
                rarity: rarity_name(it.tier),
                effective_probability_per_table_roll: None,
                probability: round6(prob),
                bias,
                level_range: it.level_range,
                probability_by_level: None,
                quantity: None,
                qualities: (!it.qualities.is_empty()).then(|| quality_names(&it.qualities)),
                entry_kind: it.kind,
                group_value: it.group_value.map(|v| round6(v as f64)),
            }
        })
        .collect()
}

/// Highest level drop-table ranges cover.
const MAX_LEVEL: u32 = 100;

fn eligible(it: &RawItem, level: u32) -> bool {
    it.level_range
        .is_none_or(|[lo, hi]| (lo..=hi).contains(&level))
}

/// Per-item odds for each level band of a level-gated pool, or `None` when
/// every entry is available at every level. Bands split wherever eligibility
/// changes; neighbouring levels with identical odds are merged.
fn probabilities_by_level(items: &[RawItem]) -> Option<Vec<Vec<LevelProbability>>> {
    if items
        .iter()
        .all(|it| (0..=MAX_LEVEL).all(|level| eligible(it, level)))
    {
        return None;
    }
    let odds_at = |level: u32| -> Vec<f64> {
        let idx: Vec<usize> = (0..items.len())
            .filter(|&i| eligible(&items[i], level))
            .collect();
        let refs: Vec<&RawItem> = idx.iter().map(|&i| &items[i]).collect();
        let mut odds = vec![0.0; items.len()];
        for (i, p) in idx.into_iter().zip(tier_probabilities(&refs)) {
            odds[i] = round6(p);
        }
        odds
    };

    let mut out = vec![Vec::new(); items.len()];
    let mut start = 0;
    let mut current = odds_at(0);
    for level in 1..=MAX_LEVEL + 1 {
        let next = (level <= MAX_LEVEL).then(|| odds_at(level));
        if next.as_ref() == Some(&current) {
            continue;
        }
        for (i, &p) in current.iter().enumerate() {
            if p > 0.0 {
                out[i].push(LevelProbability {
                    levels: [start, level - 1],
                    probability: p,
                    effective_probability_per_table_roll: None,
                });
            }
        }
        start = level;
        if let Some(next) = next {
            current = next;
        }
    }
    Some(out)
}

// ----- top-level parser -----

pub fn parse_table_raw(obj: &[u8], table_name: &str) -> Option<RawParsedTable> {
    if let Some(slots) = parse_slots(obj) {
        let pools: Vec<RawDropTablePool> = slots
            .iter()
            .filter(|slot| !slot.items.is_empty())
            .map(|slot| RawDropTablePool {
                drop_chance: Some(round6(slot.drop_chance as f64)),
                details: slot_pool_fields(slot),
                item_count: slot.items.len(),
                items: slot.items.iter().map(raw_item_output).collect(),
            })
            .collect();
        return Some(RawParsedTable {
            table_name: table_name.to_string(),
            parser_mode: "slots".to_string(),
            pool_count: pools.len(),
            total_items: pools.iter().map(|p| p.item_count).sum(),
            pools,
            manifest_items: Vec::new(),
        });
    }
    let items = find_valid_items(obj);
    if items.is_empty() {
        let manifest_entries = find_manifest_items(obj, table_name);
        if manifest_entries.is_empty() {
            return None;
        }

        let parser_mode = if manifest_entries.iter().any(|e| e.tier.is_some()) {
            "reference_manifest"
        } else {
            "legacy_string_list"
        };
        let total_items = manifest_entries.len();
        return Some(RawParsedTable {
            table_name: table_name.to_string(),
            parser_mode: parser_mode.to_string(),
            pool_count: 0,
            total_items,
            pools: Vec::new(),
            manifest_items: manifest_entries.iter().map(manifest_item_output).collect(),
        });
    }

    let first_gap_end = items[0].offset;
    let first_dc = extract_pool_dc(obj, 0, first_gap_end, true);

    struct PoolAcc {
        dc: Option<f64>,
        items: Vec<RawItem>,
    }

    let mut current = PoolAcc {
        dc: first_dc,
        items: vec![items[0].clone()],
    };
    let mut pools_acc: Vec<PoolAcc> = Vec::new();

    for idx in 1..items.len() {
        let prev_end = items[idx - 1].end;
        let curr_start = items[idx].offset;
        let gap_size = curr_start.saturating_sub(prev_end);

        if gap_size > 0 && is_pool_boundary(obj, prev_end, curr_start) {
            pools_acc.push(std::mem::replace(
                &mut current,
                PoolAcc {
                    dc: extract_pool_dc(obj, prev_end, gap_size, false),
                    items: vec![items[idx].clone()],
                },
            ));
            let _ = extract_pool_ic(obj, prev_end, gap_size);
        } else {
            current.items.push(items[idx].clone());
        }
    }
    pools_acc.push(current);

    let mut pools = Vec::with_capacity(pools_acc.len());
    let mut total_items = 0usize;
    for p in pools_acc {
        total_items += p.items.len();
        pools.push(RawDropTablePool {
            drop_chance: p.dc,
            details: PoolDetails::default(),
            item_count: p.items.len(),
            items: p.items.iter().map(raw_item_output).collect(),
        });
    }

    Some(RawParsedTable {
        table_name: table_name.to_string(),
        parser_mode: "heuristic".to_string(),
        pool_count: pools.len(),
        total_items,
        pools,
        manifest_items: Vec::new(),
    })
}

/// Item odds for one pool: tier shares, level bands, and the pool gate applied.
fn shape_pool(items: &[RawItem], drop_chance: Option<f64>, details: PoolDetails) -> Pool {
    let mut prob_items = calc_tier_probabilities(items);
    if let Some(bands) = probabilities_by_level(items) {
        for (item, bands) in prob_items.iter_mut().zip(bands) {
            item.probability_by_level = Some(bands);
        }
    }
    if let Some(drop_chance) = drop_chance {
        for item in &mut prob_items {
            item.effective_probability_per_table_roll =
                Some(round6(drop_chance * item.probability));
            for band in item.probability_by_level.iter_mut().flatten() {
                band.effective_probability_per_table_roll =
                    Some(round6(drop_chance * band.probability));
            }
        }
    }
    let psum: f64 = prob_items.iter().map(|i| i.probability).sum();
    Pool {
        drop_chance,
        details,
        item_count: prob_items.len(),
        probability_sum: round4(psum),
        items: prob_items,
    }
}

pub fn parse_table(obj: &[u8], table_name: &str) -> Option<ParsedTable> {
    if let Some(slots) = parse_slots(obj) {
        let pools: Vec<Pool> = slots
            .iter()
            .filter(|slot| !slot.items.is_empty())
            .map(|slot| {
                shape_pool(
                    &slot.items,
                    Some(round6(slot.drop_chance as f64)),
                    slot_pool_fields(slot),
                )
            })
            .collect();
        return Some(ParsedTable {
            table_name: table_name.to_string(),
            parser_mode: "slots".to_string(),
            pool_count: pools.len(),
            total_items: pools.iter().map(|p| p.item_count).sum(),
            pools,
        });
    }
    let items = find_valid_items(obj);
    if items.is_empty() {
        let manifest_entries = find_manifest_items(obj, table_name);
        if manifest_entries.is_empty() {
            return None;
        }

        if manifest_entries.iter().any(|e| e.tier.is_some()) {
            let (pools, total_items) = build_weighted_manifest_pools(&manifest_entries);
            return Some(ParsedTable {
                table_name: table_name.to_string(),
                parser_mode: "reference_manifest".to_string(),
                pool_count: pools.len(),
                total_items,
                pools,
            });
        }

        let paths: Vec<String> = manifest_entries.iter().map(|e| e.path.clone()).collect();
        let mut items = build_uniform_items(&paths, "Unknown");
        for item in &mut items {
            item.effective_probability_per_table_roll = Some(item.probability);
        }
        let psum: f64 = items.iter().map(|i| i.probability).sum();
        return Some(ParsedTable {
            table_name: table_name.to_string(),
            parser_mode: "legacy_string_list".to_string(),
            pool_count: 1,
            total_items: items.len(),
            pools: vec![Pool {
                drop_chance: Some(1.0),
                details: PoolDetails::default(),
                item_count: items.len(),
                probability_sum: round4(psum),
                items,
            }],
        });
    }

    // Structured: group items into pools.
    let first_gap_end = items[0].offset;
    let first_dc = extract_pool_dc(obj, 0, first_gap_end, true);

    struct PoolAcc {
        dc: Option<f64>,
        items: Vec<RawItem>,
    }
    let mut current = PoolAcc {
        dc: first_dc,
        items: vec![items[0].clone()],
    };
    let mut pools_acc: Vec<PoolAcc> = Vec::new();

    for idx in 1..items.len() {
        let prev_end = items[idx - 1].end;
        let curr_start = items[idx].offset;
        let gap_size = curr_start.saturating_sub(prev_end);

        if gap_size > 0 && is_pool_boundary(obj, prev_end, curr_start) {
            pools_acc.push(std::mem::replace(
                &mut current,
                PoolAcc {
                    dc: extract_pool_dc(obj, prev_end, gap_size, false),
                    items: vec![items[idx].clone()],
                },
            ));
            let _ = extract_pool_ic(obj, prev_end, gap_size); // kept for parity, unused
        } else {
            current.items.push(items[idx].clone());
        }
    }
    pools_acc.push(current);

    let output_pools: Vec<Pool> = pools_acc
        .iter()
        .map(|p| shape_pool(&p.items, p.dc, PoolDetails::default()))
        .collect();
    let total_items = output_pools.iter().map(|p| p.item_count).sum();

    Some(ParsedTable {
        table_name: table_name.to_string(),
        parser_mode: "heuristic".to_string(),
        pool_count: output_pools.len(),
        total_items,
        pools: output_pools,
    })
}

// ----- filename -> table-name -----

fn normalize_table_name(name: &str) -> String {
    name.replace('\\', "/")
        .replace('_', "/")
        .trim_matches('/')
        .to_string()
}

pub fn get_table_name(filename: &str) -> String {
    let base = filename.trim_end_matches(".bin");
    let parts: Vec<&str> = base.split('_').collect();
    let meaningful: String = match parts.iter().position(|p| p.contains("DropTables")) {
        Some(idx) => parts[idx + 1..].join("_"),
        None => base.to_string(),
    };
    let trimmed = meaningful
        .strip_suffix("DropTable")
        .or_else(|| meaningful.strip_suffix("Droptable"))
        .map(|s| s.to_string())
        .unwrap_or(meaningful.clone());
    let normalized = normalize_table_name(if trimmed.is_empty() { base } else { &trimmed });
    if normalized.is_empty() {
        base.to_string()
    } else {
        normalized
    }
}

fn table_name_from_drop_table_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/SF/Types/PickUps/DropTables/")?;
    let trimmed = rest
        .strip_suffix("DropTable")
        .or_else(|| rest.strip_suffix("Droptable"))
        .unwrap_or(rest);
    Some(normalize_table_name(trimmed))
}

fn collect_drop_table_references(results: &[ParsedTable]) -> Vec<DropTableReference> {
    let resolved_names: BTreeSet<String> = results.iter().map(|r| r.table_name.clone()).collect();
    let mut refs: BTreeMap<String, DropTableReference> = BTreeMap::new();

    for table in results {
        for (pool_index, pool) in table.pools.iter().enumerate() {
            for (item_index, item) in pool.items.iter().enumerate() {
                let Some(ref_table_name) = table_name_from_drop_table_path(&item.path) else {
                    continue;
                };
                refs.entry(item.path.clone())
                    .or_insert_with(|| DropTableReference {
                        path: item.path.clone(),
                        table_name: ref_table_name.clone(),
                        resolved: resolved_names.contains(&ref_table_name),
                        referenced_by: Vec::new(),
                    })
                    .referenced_by
                    .push(DropTableReferenceSite {
                        table_name: table.table_name.clone(),
                        pool_index,
                        item_index,
                        drop_chance: pool.drop_chance,
                        probability: item.probability,
                    });
            }
        }
    }

    refs.into_values().collect()
}

// ----- pipeline driver -----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseSummary {
    pub table_count: usize,
    pub manifest_count: usize,
    pub total_pools: usize,
    pub total_items: usize,
    pub dc_none_pools: usize,
    pub bad_prob_sum_pools: usize,
    pub drop_table_reference_count: usize,
    pub unresolved_drop_table_reference_count: usize,
    pub quantity_summary: Option<QuantitySummary>,
}

/// Run the full parse over a directory of `.bin` files and emit a combined
/// `drop_tables.json` plus a tree of per-table JSON files.
pub fn parse_directory(decrypted_dir: &Path, output_dir: &Path) -> Result<ParseSummary> {
    parse_directory_with_packages_attrs(decrypted_dir, output_dir, None)
}

/// Parse decrypted drop tables and optionally enrich emitted items with
/// min/max quantity metadata from `packages_bin_attrs.json`.
pub fn parse_directory_with_packages_attrs(
    decrypted_dir: &Path,
    output_dir: &Path,
    packages_attrs: Option<&Path>,
) -> Result<ParseSummary> {
    fs::create_dir_all(output_dir).with_context(|| format!("creating {}", output_dir.display()))?;

    let mut results: Vec<ParsedTable> = Vec::new();
    let mut raw_results: BTreeMap<String, RawDropTableEntry> = BTreeMap::new();
    for entry in fs::read_dir(decrypted_dir)
        .with_context(|| format!("reading {}", decrypted_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("bin") {
            continue;
        }
        let data = fs::read(&path)?;
        if data.len() < 0x0c {
            continue;
        }
        let obj = &data[0x0c..];
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let name = get_table_name(filename);
        if let Some(table) = parse_table(obj, &name) {
            results.push(table);
        }
        if let Some(table) = parse_table_raw(obj, &name) {
            raw_results.insert(
                name,
                RawDropTableEntry {
                    source_file: filename.to_string(),
                    table,
                },
            );
        }
    }

    results.sort_by(|a, b| a.table_name.cmp(&b.table_name));

    let quantity_summary = match packages_attrs {
        Some(path) => {
            let attrs = PackageAttrsIndex::load(path)?;
            Some(enrich_quantities(&mut results, &attrs))
        }
        None => None,
    };

    let manifest_count = results
        .iter()
        .filter(|r| r.parser_mode == "reference_manifest")
        .count();
    let total_pools: usize = results.iter().map(|r| r.pool_count).sum();
    let total_items: usize = results.iter().map(|r| r.total_items).sum();
    let dc_none_pools: usize = results
        .iter()
        .flat_map(|r| &r.pools)
        .filter(|p| p.drop_chance.is_none())
        .count();
    let bad_prob_sum_pools: usize = results
        .iter()
        .flat_map(|r| &r.pools)
        .filter(|p| (p.probability_sum - 1.0).abs() > 0.01)
        .count();
    let drop_table_references = collect_drop_table_references(&results);
    let unresolved_drop_table_reference_count =
        drop_table_references.iter().filter(|r| !r.resolved).count();

    // Combined output.
    let combined_path = output_dir.join("drop_tables.json");
    let combined = serde_json::to_string_pretty(&results)?;
    fs::write(&combined_path, combined)
        .with_context(|| format!("writing {}", combined_path.display()))?;

    let raw_combined_path = output_dir.join("raw_drop_tables.json");
    fs::write(
        &raw_combined_path,
        serde_json::to_string_pretty(&raw_results)?,
    )
    .with_context(|| format!("writing {}", raw_combined_path.display()))?;

    let references_path = output_dir.join("drop_table_references.json");
    fs::write(
        &references_path,
        serde_json::to_string_pretty(&drop_table_references)?,
    )
    .with_context(|| format!("writing {}", references_path.display()))?;

    // Per-table tree.
    for r in &results {
        let rel: Vec<&str> = r
            .table_name
            .replace('\\', "/")
            .split('/')
            .filter(|p| !p.is_empty())
            .map(|s| {
                // SAFETY: leaking is fine for this short-lived collection in this scope;
                // simpler is to build owned PathBuf as we go.
                Box::leak(s.to_string().into_boxed_str()) as &str
            })
            .collect();
        let mut path: PathBuf = output_dir.to_path_buf();
        for p in &rel {
            path.push(p);
        }
        path.set_extension("json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(r)?;
        fs::write(&path, body)?;
    }

    let raw_tree_dir = output_dir.join("raw_drop_tables");
    for (table_name, r) in &raw_results {
        let mut path: PathBuf = raw_tree_dir.clone();
        for p in table_name
            .replace('\\', "/")
            .split('/')
            .filter(|p| !p.is_empty())
        {
            path.push(p);
        }
        path.set_extension("json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(r)?;
        fs::write(&path, body)?;
    }

    Ok(ParseSummary {
        table_count: results.len(),
        manifest_count,
        total_pools,
        total_items,
        dc_none_pools,
        bad_prob_sum_pools,
        drop_table_reference_count: drop_table_references.len(),
        unresolved_drop_table_reference_count,
        quantity_summary,
    })
}

#[cfg(test)]
mod tests {
    /// Glades rare chest pool 4: four common weapons at [11, 100] and the rare
    /// Grimana fragment at [25, 100]. Below 11 nothing is eligible; from 11 the
    /// weapons split the pool; from 25 the fragment takes the Rare share.
    #[test]
    fn pool_odds_follow_level_gates() {
        let raw = |name: &str, tier: u32, range: [u32; 2]| RawItem {
            offset: 0,
            end: 0,
            path: format!("/SF/{name}"),
            name: name.to_string(),
            f1: 0.0,
            tier,
            f2: rarity_weight(tier) as f32,
            probability_weight_used: rarity_weight(tier) as f32,
            level_range: Some(range),
            ..Default::default()
        };
        let mut items: Vec<RawItem> = (0..4)
            .map(|i| raw(&format!("Weapon{i}"), 0, [11, 100]))
            .collect();
        items.push(raw("GrimanaFragment", 2, [25, 100]));

        let bands = probabilities_by_level(&items).expect("pool is gated");
        let lp = |from, to, p| LevelProbability {
            levels: [from, to],
            probability: p,
            effective_probability_per_table_roll: None,
        };
        let weapon_high = round6(0.755 / (0.755 + 0.02) / 4.0);
        assert_eq!(bands[0], vec![lp(11, 24, 0.25), lp(25, 100, weapon_high)]);
        assert_eq!(bands[4], vec![lp(25, 100, round6(0.02 / (0.755 + 0.02)))]);
    }

    /// Pool groups carry a `[min, max]` level header; items after it inherit it
    /// until the next header. Mirrors the Glades rare chest, where the Grimana
    /// fragment sits alone in a `[25, 100]` group.
    #[test]
    fn items_inherit_their_group_level_range() {
        fn item(buf: &mut Vec<u8>, path: &str, tier: u32) {
            buf.extend(0u32.to_le_bytes()); // leading word
            buf.extend((path.len() as u32).to_le_bytes());
            buf.extend(path.as_bytes());
            buf.extend(0f32.to_le_bytes());
            buf.extend(tier.to_le_bytes());

            buf.extend((rarity_weight(tier) as f32).to_le_bytes());
        }
        fn group(buf: &mut Vec<u8>, min: f32, max: f32, count: u32) {
            buf.push(0);
            buf.extend(min.to_le_bytes());
            buf.extend(max.to_le_bytes());
            buf.extend(count.to_le_bytes());
        }
        let mut buf = vec![0u8; 8];
        group(&mut buf, 11.0, 100.0, 2);
        item(&mut buf, "/SF/Weapons/A/ARngWeaponPickup", 0);
        item(&mut buf, "/SF/Weapons/B/BRngWeaponPickup", 0);
        group(&mut buf, 25.0, 100.0, 1);
        item(
            &mut buf,
            "/SF/Types/Recipes/Weapons/Melee/Fragments/GrimanaFragment",
            2,
        );
        buf.extend([0u8; 16]);

        let ranges: Vec<_> = find_valid_items(&buf)
            .iter()
            .map(|i| (i.name.clone(), i.level_range))
            .collect();
        assert_eq!(
            ranges,
            vec![
                ("ARngWeaponPickup".to_string(), Some([11, 100])),
                ("BRngWeaponPickup".to_string(), Some([11, 100])),
                ("GrimanaFragment".to_string(), Some([25, 100])),
            ]
        );
    }

    use super::*;

    #[test]
    fn get_table_name_strips_droptable_suffix() {
        let n =
            get_table_name("SF_Types_PickUps_DropTables_EnemyDrops_GenericBossSummonDropTable.bin");
        assert_eq!(n, "EnemyDrops/GenericBossSummon");
    }

    #[test]
    fn collects_unresolved_drop_table_references() {
        let table = ParsedTable {
            table_name: "EnemyDrops/Foo".to_string(),
            parser_mode: "structured".to_string(),
            pool_count: 1,
            total_items: 1,
            pools: vec![Pool {
                drop_chance: Some(0.5),
                details: PoolDetails::default(),
                item_count: 1,
                probability_sum: 1.0,
                items: vec![Item {
                    path: "/SF/Types/PickUps/DropTables/Defualts/DefaultRngWeaponPickup"
                        .to_string(),
                    name: "DefaultRngWeaponPickup".to_string(),
                    rarity: "Common".to_string(),
                    effective_probability_per_table_roll: Some(0.5),
                    probability: 1.0,
                    bias: 0.0,
                    level_range: None,
                    probability_by_level: None,
                    quantity: None,
                    ..Default::default()
                }],
            }],
        };

        let refs = collect_drop_table_references(&[table]);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].table_name, "Defualts/DefaultRngWeaponPickup");
        assert!(!refs[0].resolved);
        assert_eq!(refs[0].referenced_by[0].table_name, "EnemyDrops/Foo");
    }

    #[test]
    fn parses_fixture_breakable_crypt() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "tests/fixtures/SF_Types_PickUps_DropTables_CryptDrops_BreakableCryptDropTable.bin",
        );
        let data = fs::read(&p).expect("fixture missing");
        let obj = &data[0x0c..];
        let table = parse_table(obj, "CryptDrops/BreakableCrypt").expect("parse");
        assert!(table.total_items > 0);
        assert!(!table.pools.is_empty());
        for pool in &table.pools {
            // every pool's probability_sum should round to 1.0
            assert!(
                (pool.probability_sum - 1.0).abs() < 0.01,
                "pool prob sum {} not near 1.0",
                pool.probability_sum
            );
            if let Some(drop_chance) = pool.drop_chance {
                for item in &pool.items {
                    let expected = round6(drop_chance * item.probability);
                    assert_eq!(item.effective_probability_per_table_roll, Some(expected));
                }
            }
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        fs::read(&p).expect("fixture missing")
    }

    #[test]
    fn raw_parse_reads_category_slots() {
        let data = fixture("SF_Types_PickUps_DropTables_CryptDrops_BreakableCryptDropTable.bin");
        let table = parse_table_raw(&data[0x0c..], "CryptDrops/BreakableCrypt").expect("parse raw");
        assert_eq!(table.parser_mode, "slots");
        let ammo = &table.pools[0];
        assert_eq!(ammo.details.category.as_deref(), Some("DROP_AMMO"));
        assert_eq!(ammo.drop_chance, Some(0.15));
        assert_eq!(ammo.details.max_drops, Some(3));
        assert!(ammo
            .items
            .iter()
            .any(|item| item.path == "/SF/Types/PickUps/Currencies/DracsIncreaseSmall"));
    }

    /// Slot 4 (powerups) of the boss-summon table only rolls under the Cogah
    /// elixir; an older build without quality lists must still parse.
    #[test]
    fn slot_condition_names_required_item() {
        let data = fixture("SF_Types_PickUps_DropTables_EnemyDrops_GenericBossSummonDropTable.bin");
        let table = parse_table(&data[0x0c..], "EnemyDrops/GenericBossSummon").expect("parse");
        assert_eq!(table.parser_mode, "slots");
        let powerup = table
            .pools
            .iter()
            .find(|p| p.details.category.as_deref() == Some("DROP_POWERUP"))
            .expect("powerup slot");
        assert_eq!(powerup.drop_chance, Some(0.3));
        assert_eq!(
            powerup.details.requires.as_deref(),
            Some("/SF/Types/Items/Consumables/RedNightUpgrade")
        );
    }

    #[test]
    fn parses_fixture_dermark_miniboss() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/SF_Types_PickUps_DropTables_EnemyDrops_MinibossDrops_DermarkMiniBossDropTable.bin");
        let data = fs::read(&p).expect("fixture missing");
        let obj = &data[0x0c..];
        let table = parse_table(obj, "EnemyDrops/MinibossDrops/DermarkMiniBoss").expect("parse");
        assert!(table.total_items > 0);
        for pool in &table.pools {
            assert!((pool.probability_sum - 1.0).abs() < 0.01);
        }
    }

    #[test]
    fn resolves_item_amount_override_quantity() {
        let attrs = PackageAttrsIndex::new(BTreeMap::from([(
            "/SF/Types/PickUps/Resources/FooPickup".to_string(),
            PackageAttrsEntry {
                path: None,
                parent: "/SF/Types/PickUps/Resources/ResourcePickup".to_string(),
                attributes: "PickUpItemType=FooResource\nItemAmountOverride={1,3}\n".to_string(),
            },
        )]));

        let q = attrs.quantity_for_path("/SF/Types/PickUps/Resources/FooPickup");
        assert_eq!(q.min, 1);
        assert_eq!(q.max, 3);
        assert_eq!(q.source, "item_amount_override");
    }

    #[test]
    fn resolves_pickup_item_bonus_quantity() {
        let attrs = PackageAttrsIndex::new(BTreeMap::from([
            (
                "/SF/Types/PickUps/Currencies/DracsIncreaseSmall".to_string(),
                PackageAttrsEntry {
                    path: None,
                    parent: "/Lotus/Types/PickUps/SFPickUp".to_string(),
                    attributes: "PickUpItemType=DracsIncreaseSmallItem\n".to_string(),
                },
            ),
            (
                "/SF/Types/PickUps/Currencies/DracsIncreaseSmallItem".to_string(),
                PackageAttrsEntry {
                    path: None,
                    parent: "/SF/Types/Game/SFPickUpItem".to_string(),
                    attributes:
                        "Bonuses={\n{\nbonusType=PUBT_CREDITS_INCREASE\nvalue=40\nvalueMax=60\n}\n}\n"
                            .to_string(),
                },
            ),
        ]));

        let q = attrs.quantity_for_path("/SF/Types/PickUps/Currencies/DracsIncreaseSmall");
        assert_eq!(q.min, 40);
        assert_eq!(q.max, 60);
        assert_eq!(q.source, "pickup_item_bonus");
        assert_eq!(q.bonus_type.as_deref(), Some("PUBT_CREDITS_INCREASE"));
    }

    #[test]
    fn defaults_unoverridden_pickup_quantity_to_one() {
        let attrs = PackageAttrsIndex::new(BTreeMap::from([(
            "/SF/Types/PickUps/Resources/ScrapIronPickup".to_string(),
            PackageAttrsEntry {
                path: None,
                parent: "/SF/Types/PickUps/Resources/ResourcePickup".to_string(),
                attributes: "PickUpItemType=ScrapIronResource\n".to_string(),
            },
        )]));

        let q = attrs.quantity_for_path("/SF/Types/PickUps/Resources/ScrapIronPickup");
        assert_eq!(q.min, 1);
        assert_eq!(q.max, 1);
        assert_eq!(q.source, "default_pickup");
    }
}
