//! Portable loot data. Research/provenance objects stay inside the resolver.
use serde_json::{json, Map, Value};

fn table_name(path: &str) -> String {
    let name = path
        .strip_prefix("/SF/Types/PickUps/DropTables/")
        .unwrap_or(path);
    name.strip_suffix("DropTable")
        .or_else(|| name.strip_suffix("Droptable"))
        .unwrap_or(name)
        .to_owned()
}

fn display_name(packages: &Value, languages: &Value, path: &str) -> String {
    let mut current = path;
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(current) {
        let Some(record) = packages.get(current) else {
            break;
        };
        for field in ["LocTag", "LocalizeTag", "Name"] {
            if let Some(name) = record["attributes"][field]
                .as_str()
                .and_then(|tag| languages.get(tag))
                .and_then(Value::as_str)
            {
                return name.into();
            }
        }
        let Some(parent) = record["parent"].as_str() else {
            break;
        };
        current = parent;
    }
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

fn selection_probabilities(table: &Value, category_index: u64, offset: u64) -> Vec<Value> {
    let mut values = Vec::new();
    if let Some(scenarios) = table["selector_scenarios"]["scenarios"].as_array() {
        for scenario in scenarios {
            let Some(category) = scenario["categories"].as_array().and_then(|rows| {
                rows.iter()
                    .find(|c| c["category_index"].as_u64() == Some(category_index))
            }) else {
                continue;
            };
            let result = &category["package_resource_rarity"];
            if result["status"] != "conditional_selection_resolved" {
                continue;
            }
            let Some(entry) = result["entries"].as_array().and_then(|rows| {
                rows.iter()
                    .find(|e| e["file_offset"].as_u64() == Some(offset))
            }) else {
                continue;
            };
            if entry["conditional_probability"].is_number() {
                values.push(json!({"level":scenario["generation_level"],"probability":entry["conditional_probability"]}));
            }
        }
    }
    values
}

fn pool(table: &Value, category: &Value, packages: &Value, languages: &Value) -> Value {
    let category_index = category["index"].as_u64().unwrap_or(0);
    let mut items = Vec::new();
    if let Some(groups) = category["level_groups"].as_array() {
        for group in groups {
            if let Some(entries) = group["items"].as_array() {
                for entry in entries {
                    let path = entry["path"].as_str().unwrap_or("");
                    let probabilities = selection_probabilities(
                        table,
                        category_index,
                        entry["file_offset"].as_u64().unwrap_or(u64::MAX),
                    );
                    let mut item = json!({"path":path,"name":display_name(packages,languages,path),
                        "rarity":(["Common","Uncommon","Rare","Legendary"].get(entry["rarity_index"].as_u64().unwrap_or(u64::MAX) as usize)),
                        "bias":entry["bias"],"min_level":group["minimum_level"],"max_level":group["maximum_level"],
                        "probability":null});
                    // An unsampled level is not silently assigned the nearest sample's odds.
                    if !probabilities.is_empty() {
                        item["probabilities_by_level"] = json!(probabilities);
                    }
                    items.push(item);
                }
            }
        }
    }
    json!({"category":category_index,"drop_chance":category["chance"],
        "rolls":{"min":category["minimum_drops"],"max":category["maximum_drops"]},
        "quality_tiers":category["quality_indices"],"items":items})
}

fn source(binding: &Value) -> Option<Value> {
    let mut row = json!({"path":binding["source"],"name":binding["source_name"]});
    match binding["kind"].as_str()? {
        "authored_table_binding" => {
            row["event"] = json!(match binding["field"].as_str() {
                Some("CatchDropTable") => "catch",
                Some("ReleaseDropTable") => "release",
                _ => "drop",
            });
            row["table"] = binding["table"]["value"].clone();
            if binding["field"] == "DropTable" {
                row["authored_drop_chance"] = binding["authored_drop_chance"].clone();
                row["enabled"] = binding["authored_drop_chance"]
                    .as_f64()
                    .map(|chance| json!(chance > 0.0))
                    .unwrap_or(Value::Null);
            }
        }
        "mushroom_chest_mode_swap" => {
            row["event"] = json!("collect");
            row["tables"] = json!({"normal":binding["normal_table"]["value"],"cogah":binding["hard_mode_table"]["value"]});
            row["enemies"] = json!(binding["enemy_bindings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| e["source"].clone())
                .collect::<Vec<_>>());
            row["enemy_triggers"] = Value::Object(
                binding["enemy_bindings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|enemy| {
                        Some((
                            enemy["source"].as_str()?.to_owned(),
                            enemy["trigger"].clone(),
                        ))
                    })
                    .collect(),
            );
        }
        "region_table_override" => {
            row["event"] = json!("spawn");
            row["region"] = binding["region_level_info"].clone();
            row["tables"] =
                json!({"normal":binding["normal_table"],"cogah":binding["hard_mode_table"]});
            row["authored_drop_chance"] = binding["authored_drop_chance"].clone();
            row["enabled"] = binding["authored_drop_chance"]
                .as_f64()
                .map(|chance| json!(chance > 0.0))
                .unwrap_or(Value::Null);
        }
        _ => return None,
    }
    Some(row)
}

fn only_keys(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(*key).map(|v| ((*key).to_string(), v.clone())))
            .collect::<Map<_, _>>(),
    )
}

fn observations(aggregation: &Value) -> Value {
    let mut result = only_keys(
        aggregation,
        &[
            "capture_complete",
            "observed_native_invocations",
            "independent_uniform_draws_model",
        ],
    );
    result["calls"] = json!(aggregation["calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| only_keys(
            c,
            &[
                "invocation_id",
                "run_id",
                "enemy_id",
                "selected_table",
                "generation_level",
                "conditional_item_probabilities"
            ]
        ))
        .collect::<Vec<_>>());
    for key in ["enemies", "runs"] {
        result[key] = json!(aggregation[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| only_keys(
                r,
                &[
                    "run_id",
                    "enemy_id",
                    "observed_native_invocations",
                    "conditional_item_probabilities"
                ]
            ))
            .collect::<Vec<_>>());
    }
    result
}

pub fn droptables(report: &Value, packages: &Value, languages: &Value) -> Value {
    let native_matched = report["native_evidence"]["matched"] == true;
    let tables=report["tables"].as_object().into_iter().flatten().map(|(path,table)| {
        let pools:Vec<_>=table["categories"].as_array().into_iter().flatten().map(|c|pool(table,c,packages,languages))
            .filter(|p|p["items"].as_array().is_some_and(|items|!items.is_empty())).collect();
        json!({"table_name":table_name(path),"path":path,"pool_count":pools.len(),
            "total_items":pools.iter().map(|p|p["items"].as_array().map_or(0,Vec::len)).sum::<usize>(),"pools":pools})
    }).collect::<Vec<_>>();
    let mut sources = report["source_bindings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(source)
        .collect::<Vec<_>>();
    if native_matched {
        for row in &mut sources {
            if row["event"] == "collect" {
                row["weapon_context"] = json!("collector");
            }
        }
    }
    for mut row in crate::loot_script_sources::sources(packages) {
        if let Some(path) = row["path"].as_str() {
            row["name"] = json!(display_name(packages, languages, path));
        }
        sources.push(row);
    }
    let groups =
        crate::loot_sources::groups(&sources, |path| display_name(packages, languages, path));
    let mut result = json!({"probability_unit":"fraction","probability_scope":"conditional_item_selection",
        "drop_chance_basis":"unmodified_category","tables":tables,"sources":sources,"source_groups":groups});
    result["weapon_resolution"] = if native_matched {
        json!({
            "generic_pickup":"/SF/Types/PickUps/DropTables/Defualts/DefaultRngWeaponPickup",
            "method":"context_weapon_store_item_drop",
            "requires_unassigned_item":true,
            "weapon_search_order":["selected_weapon","inventory_slot_5","inventory_slot_1"],
            "stop_at_first_compatible_weapon":true,
            "missing_store_item_drop":"no_weapon_item"
        })
    } else {
        Value::Null
    };
    if report["aggregation"]["calls"].is_array() {
        result["observations"] = observations(&report["aggregation"]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report() -> Value {
        json!({"inputs":{"file":"C:/private/file.json"},"tables":{
            "/SF/Types/PickUps/DropTables/ExampleDropTable":{
                "source":{"file":"C:/private/table.bin","sha256":"secret-hash"},
                "categories":[{"index":7,"chance":0.8,"minimum_drops":0,"maximum_drops":1,"quality_indices":[2,3],
                    "level_groups":[{"minimum_level":15,"maximum_level":100,"items":[{"path":"/SF/Weapon","file_offset":123,"rarity_index":1,"bias":0.2}]}]}],
                "selector_scenarios":{"scenarios":[{"generation_level":35,"categories":[{"category_index":7,
                    "package_resource_rarity":{"status":"conditional_selection_resolved","entries":[{"file_offset":123,"conditional_probability":0.25}]},
                    "serialized_rarity_scenario":{"status":"conditional_selection_resolved","entries":[{"file_offset":123,"conditional_probability":0.99}]}}]}]}
            }},"source_bindings":[],"aggregation":{}})
    }
    #[test]
    fn portable_rows_preserve_ranges_without_research_or_estimated_odds() {
        let output = droptables(&report(), &Value::Null, &Value::Null);
        let table = &output["tables"][0];
        let item = &table["pools"][0]["items"][0];
        assert_eq!(table["table_name"], "Example");
        assert_eq!(item["min_level"], 15);
        assert_eq!(item["max_level"], 100);
        assert_eq!(
            item["probabilities_by_level"],
            json!([{"level":35,"probability":0.25}])
        );
        let serialized = output.to_string();
        for forbidden in [
            "C:/",
            "file_offset",
            "sha256",
            "selector_scenarios",
            "secret-hash",
            "0.99",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }
    #[test]
    fn unknown_native_or_resource_inputs_stay_null() {
        let mut input = report();
        input["tables"]["/SF/Types/PickUps/DropTables/ExampleDropTable"]["selector_scenarios"] =
            Value::Null;
        let out = droptables(&input, &Value::Null, &Value::Null);
        let item = &out["tables"][0]["pools"][0]["items"][0];
        assert!(item["probability"].is_null());
        assert!(item.get("probabilities_by_level").is_none());
    }
    #[test]
    fn cogah_null_and_game_resource_ids_survive_projection() {
        let result=source(&json!({"kind":"mushroom_chest_mode_swap","source":"/SF/Chest","source_name":"Chest",
            "normal_table":{"value":"/SF/Normal","package_pointer":"secret-pointer"},"hard_mode_table":{"value":null},"enemy_bindings":[{"source":"/SF/Enemy"}]})).unwrap();
        assert_eq!(result["tables"]["normal"], "/SF/Normal");
        assert!(result["tables"]["cogah"].is_null());
        assert_eq!(result["enemies"], json!(["/SF/Enemy"]));
        assert!(!result.to_string().contains("pointer"));
    }
    #[test]
    fn observed_counts_and_null_models_survive_without_evidence_paths() {
        let mut input = report();
        input["aggregation"] = json!({"capture_complete":false,"observed_native_invocations":2,"input":{"file":"C:/trace"},"calls":[{"invocation_id":"a","selected_table":"/SF/Table","selection_evidence":"private","categories":[{}],"conditional_item_probabilities":null}],"enemies":[{"run_id":"run","enemy_id":"enemy","observed_native_invocations":2,"conditional_item_probabilities":null}],"runs":[]});
        let output = droptables(&input, &Value::Null, &Value::Null);
        assert_eq!(output["observations"]["observed_native_invocations"], 2);
        assert!(output["observations"]["enemies"][0]["conditional_item_probabilities"].is_null());
        for forbidden in ["C:/trace", "private", "selection_evidence", "categories"] {
            assert!(!output["observations"].to_string().contains(forbidden));
        }
    }

    #[test]
    fn native_weapon_rule_and_collector_context_require_verified_build() {
        let mut input = report();
        input["source_bindings"] = json!([{"kind":"mushroom_chest_mode_swap","source":"/SF/Chest","source_name":"Chest",
            "normal_table":{"value":"/SF/Normal"},"hard_mode_table":{"value":"/SF/Hard"},"enemy_bindings":[{"source":"/SF/Boss","trigger":"death"}]}]);
        let unknown = droptables(&input, &Value::Null, &Value::Null);
        assert!(unknown["weapon_resolution"].is_null());
        assert!(unknown["sources"][0].get("weapon_context").is_none());
        input["native_evidence"] = json!({"matched":true});
        let known = droptables(&input, &Value::Null, &Value::Null);
        assert_eq!(
            known["weapon_resolution"]["method"],
            "context_weapon_store_item_drop"
        );
        assert_eq!(known["sources"][0]["weapon_context"], "collector");
        assert!(known["source_groups"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["routes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["weapon_context"] == "collector")));
    }
}
