//! Portable source routes whose script entry points have been checked.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const LOOT_CHEST: &str = "/SF/Scripts/Levels/LootChest.lua";
const ODE_DRONE: &str = "/SF/Scripts/GameplayObjects/OdeDrone.lua";

fn resource(owner: &str, value: &Value) -> Option<String> {
    let reference = value.as_str().filter(|s| !s.is_empty())?;
    let joined = if reference.starts_with('/') {
        reference.to_owned()
    } else {
        format!("{}/{}", owner.rsplit_once('/')?.0, reference)
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

fn chain(packages: &Value, path: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = path.to_owned();
    while seen.insert(current.clone()) {
        let Some(record) = packages.get(&current) else {
            break;
        };
        result.push(current.clone());
        let Some(parent) = resource(&current, &record["parent"]) else {
            break;
        };
        current = parent;
    }
    result
}

fn field(packages: &Value, path: &str, name: &str) -> Option<(String, Value)> {
    chain(packages, path).into_iter().find_map(|owner| {
        packages[&owner]["attributes"]
            .get(name)
            .map(|value| (owner, value.clone()))
    })
}

fn resolve_script_value(owner: &str, key: &str, value: &Value) -> Value {
    match key {
        "Script" | "_dropTable" => resource(owner, value).map_or(Value::Null, Value::String),
        "_possibleCarryObjects" | "_physicsObjects" | "_carryObjectDropTables" => value
            .as_array()
            .map(|values| {
                Value::Array(
                    values
                        .iter()
                        .map(|v| resource(owner, v).map_or(Value::Null, Value::String))
                        .collect(),
                )
            })
            .unwrap_or_else(|| value.clone()),
        _ => value.clone(),
    }
}

fn script(packages: &Value, path: &str, name: &str) -> Option<Map<String, Value>> {
    let mut merged: Option<Map<String, Value>> = None;
    for owner in chain(packages, path).into_iter().rev() {
        let Some(layer) = packages[&owner]["attributes"].get(name) else {
            continue;
        };
        let Some(layer) = layer.as_object() else {
            // An explicit null replaces the script, including inherited fields.
            merged = None;
            continue;
        };
        if let Some(next) = layer.get("Script") {
            let next = resolve_script_value(&owner, "Script", next);
            if merged
                .as_ref()
                .and_then(|m| m.get("Script"))
                .is_some_and(|previous| previous != &next)
            {
                merged = None;
            }
        }
        let output = merged.get_or_insert_with(Map::new);
        for (key, value) in layer {
            output.insert(key.clone(), resolve_script_value(&owner, key, value));
        }
    }
    merged
}

fn bool_value(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(v) => Some(*v),
        Value::Number(v) if v.as_i64() == Some(0) => Some(false),
        Value::Number(v) if v.as_i64() == Some(1) => Some(true),
        _ => None,
    }
}

fn script_route(packages: &Value, path: &str, field_name: &str) -> Option<Value> {
    let config = script(packages, path, field_name)?;
    if config.get("Script")?.as_str()? != LOOT_CHEST {
        return None;
    }
    let event = match (field_name, config.get("Function")?.as_str()?) {
        ("CompleteScript", "LootChest") => "open",
        ("StateChangeScript" | "OnDamageScript", "DecorationLootDrop") => {
            // Its region branch indexes the density volume's blue channel.
            // Do not present the default table as unconditional in that branch.
            if bool_value(config.get("_useRegionDropTables")) != Some(false) {
                return None;
            }
            "destroy"
        }
        _ => return None,
    };
    let table = config.get("_dropTable")?.as_str()?;
    let mut row = json!({"path":path,"event":event,"table":table});
    if event == "destroy" {
        if let Some(chance) = config
            .get("_spawnChance")
            .and_then(Value::as_f64)
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
        {
            row["spawn_chance"] = json!(chance);
            row["enabled"] = json!(chance > 0.0);
        }
    }
    Some(row)
}

fn ordinary_routes(packages: &Value, path: &str) -> Vec<Value> {
    let mut result = Vec::new();
    for key in ["CompleteScript", "StateChangeScript", "OnDamageScript"] {
        if let Some(row) = script_route(packages, path, key) {
            if !result.contains(&row) {
                result.push(row);
            }
        }
    }
    // Containers expose their interaction through a server-side child action.
    // Array inheritance is replacement, not concatenation.
    if let Some((owner, children)) = field(packages, path, "ServerChildren") {
        for child in children.as_array().into_iter().flatten() {
            if child["Chance"].as_f64() == Some(0.0) {
                continue;
            }
            let Some(action) = resource(&owner, &child["Type"]) else {
                continue;
            };
            let Some(mut row) = script_route(packages, &action, "CompleteScript") else {
                continue;
            };
            row["path"] = json!(path);
            row["via"] = json!(action);
            if child["Chance"].as_f64().is_some_and(|p| p < 255.0) {
                row["conditions"] = json!(["child_action_spawned"]);
            }
            if !result.contains(&row) {
                result.push(row);
            }
        }
    }
    result
}

fn drone_routes(packages: &Value, path: &str) -> Vec<Value> {
    let Some(config) = script(packages, path, "ScriptInstance") else {
        return Vec::new();
    };
    if config.get("Script").and_then(Value::as_str) != Some(ODE_DRONE)
        || config.get("Function").and_then(Value::as_str) != Some("OdeDrone")
    {
        return Vec::new();
    }
    let Some(objects) = config
        .get("_possibleCarryObjects")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let Some(weights) = config.get("_carryObjectWeights").and_then(Value::as_array) else {
        return Vec::new();
    };
    if objects.is_empty() || objects.len() != weights.len() {
        return Vec::new();
    }
    let Some(weights): Option<Vec<_>> = weights
        .iter()
        .map(|v| v.as_f64().filter(|w| w.is_finite() && *w >= 0.0))
        .collect()
    else {
        return Vec::new();
    };
    let total: f64 = weights.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return Vec::new();
    }
    let Some(physics) = config.get("_physicsObjects").and_then(Value::as_array) else {
        return Vec::new();
    };
    let recovery = config
        .get("_carryObjectDropTables")
        .and_then(Value::as_array);
    let mut result = Vec::new();
    for (index, (object, weight)) in objects.iter().zip(weights).enumerate() {
        if weight <= 0.0 || object.as_str().is_none() {
            continue;
        }
        let Some(physical) = physics.get(index).and_then(Value::as_str) else {
            continue;
        };
        for mut row in ordinary_routes(packages, physical) {
            row["event"] = json!(match row["event"].as_str() {
                Some("open") => "cargo_open",
                Some("destroy") => "cargo_destroy",
                _ => continue,
            });
            row["path"] = json!(path);
            row["via"] = json!(physical);
            row["cargo_index"] = json!(index + 1);
            row["cargo_selection_probability"] = json!(weight / total);
            row["cargo_probability_basis"] = json!("normalized_weights");
            let mut conditions = row["conditions"].as_array().cloned().unwrap_or_default();
            conditions.push(json!("not_siege_mine_laying"));
            row["conditions"] = json!(conditions);
            result.push(row);
        }
        // Recovery is a direct grant only after this same object type moves
        // too far. Different physics types would need a second type-match audit.
        if object.as_str() != Some(physical) {
            continue;
        }
        if let Some(table) = recovery.and_then(|r| r.get(index)).and_then(Value::as_str) {
            let mut row = json!({"path":path,"event":"cargo_recovery","table":table,
                "via":physical,"cargo_index":index+1,"cargo_selection_probability":weight/total,
                "cargo_probability_basis":"normalized_weights",
                "conditions":["not_siege_mine_laying","moved_too_far","cargo_unchanged"]});
            if let Some(distance) = config.get("_maxRewardDistance").and_then(Value::as_f64) {
                row["max_reward_distance"] = json!(distance);
            }
            result.push(row);
        }
    }
    result
}

/// Authored script routes, not counts of observed invocations or per-kill odds.
pub fn sources(packages: &Value) -> Vec<Value> {
    let Some(records) = packages.as_object() else {
        return Vec::new();
    };
    // Stable order also makes snapshot comparisons useful downstream.
    let paths: BTreeMap<_, _> = records.iter().collect();
    let mut output = Vec::new();
    for path in paths.keys() {
        output.extend(ordinary_routes(packages, path));
        output.extend(drone_routes(packages, path));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        json!({
            "/SF/Base/Action":{"attributes":{"CompleteScript":{"Script":LOOT_CHEST,
                "Function":"LootChest","_dropTable":"Loot/Table"}}},
            "/SF/Containers/Action":{"parent":"/SF/Base/Action","attributes":{}},
            "/SF/Containers/Chest":{"attributes":{"ServerChildren":[{"Type":"Action","Chance":255}]}},
            "/SF/Containers/Crate":{"attributes":{"StateChangeScript":{"Script":LOOT_CHEST,
                "Function":"DecorationLootDrop","_dropTable":"CrateTable","_useRegionDropTables":0,"_spawnChance":1}}},
            "/SF/Drone":{"attributes":{"ScriptInstance":{"Script":ODE_DRONE,"Function":"OdeDrone",
                "_possibleCarryObjects":["/SF/Containers/Crate","/SF/Containers/Chest"],
                "_physicsObjects":["/SF/Containers/Crate","/SF/Containers/Chest"],
                "_carryObjectWeights":[1,1],"_carryObjectDropTables":["RecoveryCrate","RecoveryChest"],
                "_maxRewardDistance":100}}}
        })
    }

    #[test]
    fn inherited_relative_table_belongs_to_declaring_resource() {
        let rows = sources(&fixture());
        let row = rows
            .iter()
            .find(|r| r["path"] == "/SF/Containers/Chest")
            .unwrap();
        assert_eq!(row["table"], "/SF/Base/Loot/Table");
        assert_eq!(row["event"], "open");
        assert_eq!(row["via"], "/SF/Containers/Action");
    }

    #[test]
    fn null_table_script_and_children_block_inheritance() {
        let mut data = fixture();
        data["/SF/Containers/Action"]["attributes"]["CompleteScript"] = json!({"_dropTable":null});
        assert!(!sources(&data)
            .iter()
            .any(|r| r["path"] == "/SF/Containers/Chest"));
        data["/SF/Containers/Action"]["attributes"]["CompleteScript"] = Value::Null;
        assert!(!sources(&data)
            .iter()
            .any(|r| r["path"] == "/SF/Containers/Action"));
        data["/SF/DerivedChest"] =
            json!({"parent":"/SF/Containers/Chest","attributes":{"ServerChildren":null}});
        assert!(!sources(&data)
            .iter()
            .any(|r| r["path"] == "/SF/DerivedChest"));
    }

    #[test]
    fn changing_script_does_not_reuse_old_function_or_table() {
        let mut data = fixture();
        data["/SF/Containers/Action"]["attributes"]["CompleteScript"] =
            json!({"Script":"/SF/Other.lua"});
        data["/SF/Next"] = json!({"parent":"/SF/Containers/Action","attributes":{"CompleteScript":{"Script":LOOT_CHEST}}});
        assert!(!sources(&data).iter().any(|r| r["path"] == "/SF/Next"));
    }

    #[test]
    fn region_selection_and_other_entry_points_are_not_false_default_routes() {
        let mut data = fixture();
        data["/SF/Containers/Crate"]["attributes"]["StateChangeScript"]["_useRegionDropTables"] =
            json!(1);
        data["/SF/Containers/Action"]["attributes"]["CompleteScript"] =
            json!({"Function":"MushroomChest"});
        assert!(!sources(&data)
            .iter()
            .any(|r| r["path"] == "/SF/Containers/Crate" || r["path"] == "/SF/Containers/Chest"));
    }

    #[test]
    fn drone_cargo_keeps_normal_and_recovery_routes_separate() {
        let rows = sources(&fixture());
        let drone: Vec<_> = rows.iter().filter(|r| r["path"] == "/SF/Drone").collect();
        assert_eq!(drone.len(), 4);
        let recovery: Vec<_> = drone
            .iter()
            .filter(|r| r["event"] == "cargo_recovery")
            .collect();
        assert_eq!(recovery.len(), 2);
        assert!(drone
            .iter()
            .all(|r| r["cargo_selection_probability"] == 0.5));
        assert!(drone
            .iter()
            .all(|r| r["cargo_probability_basis"] == "normalized_weights"));
        assert!(drone.iter().all(|r| r.get("probability").is_none()));
        assert_eq!(recovery[1]["table"], "/SF/RecoveryChest");
        assert_eq!(
            recovery[1]["conditions"],
            json!(["not_siege_mine_laying", "moved_too_far", "cargo_unchanged"])
        );
    }

    #[test]
    fn malformed_parallel_arrays_do_not_invent_cargo_routes() {
        let mut data = fixture();
        data["/SF/Drone"]["attributes"]["ScriptInstance"]["_carryObjectWeights"] = json!([1]);
        assert!(!sources(&data).iter().any(|r| r["path"] == "/SF/Drone"));
        data["/SF/Drone"]["attributes"]["ScriptInstance"]["_carryObjectWeights"] = json!([1, 1]);
        data["/SF/Drone"]["attributes"]["ScriptInstance"]["_physicsObjects"] = json!([]);
        assert!(!sources(&data).iter().any(|r| r["path"] == "/SF/Drone"));
    }

    #[test]
    fn inheritance_cycles_terminate_and_zero_chance_child_is_absent() {
        let mut data = fixture();
        data["/SF/Base/Action"]["parent"] = json!("/SF/Containers/Action");
        data["/SF/Containers/Chest"]["attributes"]["ServerChildren"][0]["Chance"] = json!(0);
        assert!(!sources(&data)
            .iter()
            .any(|r| r["path"] == "/SF/Containers/Chest"));
    }
}
