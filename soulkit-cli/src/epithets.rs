//! Weapon Epithets (Title items) keyed by the inventory `internalId`
//! (`FlavourItems.<Name>`, as inventory.php logs it), with lang key, English
//! value and the unlocking challenge.
//!
//! Challenge -> epithet goes through the challenge's `ProgressLoc`
//! (`/SF/Language/Titles/<Name>_Req`), which survives renames like
//! `CastingSkillTreeKillsTitleChallengeFix`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::BufReader,
    path::Path,
};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};

const ITEM_ROOT: &str = "/SF/Types/Items/Titles/";
const CHAL_ROOT: &str = "/SF/Types/Challenges/Titles/";
const LANG_ROOT: &str = "/SF/Language/Titles/";
const CHALLENGE_MGR: &str = "/SF/Types/Challenges/SFChallengeMgr";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Epithet {
    internal_id: String,
    item_type: String,
    store_item: String,
    lang_key: String,
    value: String,
    challenge: Challenge,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Challenge {
    /// `inventory_data.ChallengeProgress[].Name`.
    name: String,
    /// Older accounts still carry progress under the pre-"Fix" name.
    legacy_name: Option<String>,
    path: String,
    enabled: bool,
    required_count: u64,
    lang_key: String,
    value: String,
    text: String,
}

fn load(path: &Path) -> Result<Map<String, Value>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parsing {}", path.display()))
}

/// Attributes merged root-first down the parent chain (shallow: the fields
/// read here are all top-level).
fn merged<'a>(pk: &'a Map<String, Value>, mut path: &'a str) -> HashMap<&'a str, &'a Value> {
    let mut chain = Vec::new();
    while let Some(p) = pk.get(path) {
        chain.push(p);
        path = p["parent"].as_str().unwrap_or("");
    }
    let mut out = HashMap::new();
    for p in chain.iter().rev() {
        if let Some(attrs) = p["attributes"].as_object() {
            out.extend(attrs.iter().map(|(k, v)| (k.as_str(), v)));
        }
    }
    out
}

pub fn run(
    formatted_packages: &Path,
    packages_bin_attrs: &Path,
    languages: &Path,
    output: &Path,
) -> Result<()> {
    let pk = load(formatted_packages)?;
    let lang = load(languages)?;
    let bin = load(packages_bin_attrs)?;
    let text = |key: &str| lang.get(key).and_then(Value::as_str).map(str::to_owned);

    let mgr = bin
        .get(CHALLENGE_MGR)
        .and_then(|m| m["attributes"].as_str())
        .with_context(|| {
            format!(
                "{CHALLENGE_MGR} missing from {}",
                packages_bin_attrs.display()
            )
        })?;
    let enabled: HashSet<&str> = mgr
        .split_once("EnabledChallenges={")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(list, _)| {
            list.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut challenges = HashMap::new();
    for path in pk.keys().filter(|p| p.starts_with(CHAL_ROOT)) {
        let a = merged(&pk, path);
        let loc = a.get("ProgressLoc").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(name) = loc
            .strip_prefix(LANG_ROOT)
            .and_then(|s| s.strip_suffix("_Req"))
        {
            challenges.insert(name, (path.as_str(), loc, a));
        }
    }

    let mut out = BTreeMap::new();
    let mut missing = Vec::new();
    for path in pk.keys().filter(|p| p.starts_with(ITEM_ROOT)) {
        let name = &path[ITEM_ROOT.len()..];
        if matches!(name, "BaseTitle" | "TitleItem") {
            continue;
        }
        let a = merged(&pk, path);
        let lang_key = a
            .get("TitleTag")
            .or_else(|| a.get("LocalizeTag"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (Some(value), Some((cpath, loc, c))) = (text(lang_key), challenges.get(name)) else {
            missing.push(name.to_owned());
            continue;
        };
        let cname = cpath.rsplit('/').next().unwrap_or(cpath);
        // Single-boss kills set no count ("Defeat the X Agari").
        let count = ["DisplayRequiredCount", "RequiredCount"]
            .iter()
            .find_map(|k| c.get(k).and_then(|v| v.as_u64()))
            .unwrap_or(1);
        let req = text(loc).unwrap_or_default();
        let internal_id = format!("FlavourItems.{name}");
        out.insert(
            internal_id.clone(),
            Epithet {
                internal_id,
                item_type: path.clone(),
                store_item: format!("/SF/StoreItems/Types/Items/Titles/{name}"),
                lang_key: lang_key.to_owned(),
                value,
                challenge: Challenge {
                    name: cname.to_owned(),
                    legacy_name: cname.strip_suffix("Fix").map(str::to_owned),
                    path: cpath.to_string(),
                    enabled: cpath
                        .strip_prefix("/SF/Types/Challenges/")
                        .is_some_and(|p| enabled.contains(p)),
                    required_count: count,
                    lang_key: loc.to_string(),
                    text: req
                        .replace("|PROGRESS|/|REQUIRED|", &count.to_string())
                        .trim()
                        .to_owned(),
                    value: req,
                },
            },
        );
    }
    if !missing.is_empty() {
        bail!("epithets without name or challenge: {missing:?}");
    }

    let enabled_count = out.values().filter(|e| e.challenge.enabled).count();
    std::fs::write(output, serde_json::to_string_pretty(&out)?)
        .with_context(|| format!("writing {}", output.display()))?;
    println!(
        "  [epithets] {} epithets ({enabled_count} enabled) -> {}",
        out.len(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs against the checked-in pipeline outputs; skipped when they are absent.
    #[test]
    fn real_outputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../output");
        let fp = root.join("formatted_packages.json");
        if !fp.exists() {
            return;
        }
        let out = std::env::temp_dir().join("soulkit-epithets-test.json");
        super::run(
            &fp,
            &root.join("packages_bin_attrs.json"),
            &root.join("Languages/Languages.json"),
            &out,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let casting = &v["FlavourItems.CastingSkillTreeKillsTitle"];
        assert_eq!(casting["value"], "Gandura");
        assert_eq!(
            casting["challenge"]["legacyName"],
            "CastingSkillTreeKillsTitleChallenge"
        );
        assert_eq!(casting["challenge"]["requiredCount"], 500);
        assert!(v
            .as_object()
            .unwrap()
            .values()
            .all(|e| e["challenge"]["enabled"] == true));
    }
}
