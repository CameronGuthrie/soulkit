//! Port of `generate_reward_distributions.py`.
//!
//! Reads a `manifest_rewards_raw.json` dump (`{manifest_path: plaintext}`)
//! and emits a Soulframe-syndicate reward distribution JSON keyed by
//! short syndicate names.

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Split a chunk of text into the contents of each top-level `{ ... }` block.
fn split_top_level_braces(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut depth: i32 = 0;
    let mut start: Option<usize> = None;
    for (i, ch) in text.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        blocks.push(text[s..i].to_string());
                    }
                }
            }
            _ => {}
        }
    }
    blocks
}

/// Parse the `Favors={ {...}, {...}, ... }` block from a manifest's text.
fn parse_favors(text: &str) -> Vec<Value> {
    let mut items = Vec::new();
    let needle = "Favors={";
    let Some(start) = text.find(needle) else {
        return items;
    };
    let rest = &text[start + needle.len()..];

    // Walk to the matching closing brace of the outer Favors block.
    let mut depth: i32 = 1;
    let mut end = 0;
    for (i, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let favors_block = &rest[..end];
    // Match each `key=value` independently (value is bounded by whitespace),
    // so the parser works whether entries are space- or newline-separated.
    let kv_re = Regex::new(r"(\w+)=(\S+)").unwrap();

    for entry_text in split_top_level_braces(favors_block) {
        let mut entry = Map::new();
        for cap in kv_re.captures_iter(&entry_text) {
            let key = &cap[1];
            let val = cap[2].trim();
            match key {
                "storeItem" => {
                    entry.insert("store_item".to_string(), Value::String(val.to_string()));
                }
                "standingCost" => {
                    if let Ok(n) = val.parse::<i64>() {
                        entry.insert("standing_cost".to_string(), Value::from(n));
                    }
                }
                "requiredLevel" => {
                    if let Ok(n) = val.parse::<i64>() {
                        entry.insert("required_rank".to_string(), Value::from(n));
                    }
                }
                "availableAsFreeFavor" => {
                    entry.insert("always_available".to_string(), Value::Bool(val == "1"));
                }
                _ => {}
            }
        }
        if entry.contains_key("store_item") {
            items.push(Value::Object(entry));
        }
    }
    items
}

struct Syndicate {
    manifest_path: &'static str,
    key: &'static str,
    name: &'static str,
}

const SYNDICATES: &[Syndicate] = &[
    Syndicate {
        manifest_path: "/SF/Syndicates/ChampionSyndicateManifest",
        key: "champion_kith_of_kings",
        name: "Champion (Kith of Kings)",
    },
    Syndicate {
        manifest_path: "/SF/Syndicates/SilentRoseManifest",
        key: "silent_rose",
        name: "Silent Rose",
    },
    Syndicate {
        manifest_path: "/SF/Syndicates/FeySyndicateManifest",
        key: "alcas_children_fey",
        name: "Alca's Children (Fey)",
    },
];

pub fn run(manifest_rewards_raw: &Path, output: &Path) -> Result<()> {
    let raw = fs::read_to_string(manifest_rewards_raw)
        .with_context(|| format!("Reading {:?}", manifest_rewards_raw))?;
    let manifests: BTreeMap<String, String> =
        serde_json::from_str(&raw).context("Parsing manifest_rewards_raw.json")?;

    let mut stores = Map::new();
    let mut total = 0usize;
    for s in SYNDICATES {
        let text = manifests
            .get(s.manifest_path)
            .map(String::as_str)
            .unwrap_or("");
        let items = parse_favors(text);
        println!("  {}: {} items", s.name, items.len());
        total += items.len();
        stores.insert(
            s.key.to_string(),
            json!({
                "manifest_path": s.manifest_path,
                "items": items,
            }),
        );
    }

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let result = json!({
        "metadata": {
            "title": "Soulframe Reward Distributions",
            "generated_at": today,
            "source": "Syndicate manifest entries extracted from H.Misc.cache",
        },
        "syndicate_store_rewards": stores,
    });

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, serde_json::to_string_pretty(&result)?)?;
    println!("\nWrote {} ({} total store items)", output.display(), total);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_braces_basic() {
        let blocks = split_top_level_braces(" {a={b}} {c} {d{e}f} ");
        assert_eq!(blocks, vec!["a={b}", "c", "d{e}f"]);
    }

    #[test]
    fn parse_favors_extracts_fields() {
        let text = "\
Favors={
  {storeItem=/SF/X standingCost=100 requiredLevel=2 availableAsFreeFavor=1}
  {storeItem=/SF/Y standingCost=500 availableAsFreeFavor=0}
}";
        let items = parse_favors(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["store_item"], "/SF/X");
        assert_eq!(items[0]["standing_cost"], 100);
        assert_eq!(items[0]["required_rank"], 2);
        assert_eq!(items[0]["always_available"], true);
        assert_eq!(items[1]["always_available"], false);
    }
}
