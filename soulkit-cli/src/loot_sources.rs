//! Join authored loot routes without treating mode choices or disabled tables as
//! simultaneous table invocations.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Group {
    name: String,
    routes: Vec<Value>,
}

fn add(groups: &mut BTreeMap<String, Group>, path: &str, name: String, route: Value) {
    let group = groups.entry(path.into()).or_default();
    if group.name.is_empty() {
        group.name = name;
    }
    if !group.routes.contains(&route) {
        group.routes.push(route);
    }
}

fn enabled(chance: &Value) -> Value {
    chance
        .as_f64()
        .filter(|p| p.is_finite())
        .map(|p| json!(p > 0.0))
        .unwrap_or(Value::Null)
}

/// Only multi-table configurations are included. `enabled` describes authored
/// configuration, not proof that an event ran or a table was invoked.
pub fn groups(sources: &[Value], name_for: impl Fn(&str) -> String) -> Vec<Value> {
    let mut groups = BTreeMap::<String, Group>::new();
    let overridden: BTreeSet<_> = sources
        .iter()
        .filter(|s| s["event"] == "spawn")
        .filter_map(|s| s["path"].as_str())
        .collect();
    for source in sources {
        let Some(path) = source["path"].as_str() else {
            continue;
        };
        let name = source["name"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| name_for(path));
        let Some(event) = source["event"].as_str() else {
            continue;
        };
        if let Some(table) = source["table"].as_str() {
            let mut route = json!({"table":table,"event":event,
                "enabled":source.get("enabled").cloned().unwrap_or_else(||
                    if event=="drop" {enabled(&source["authored_drop_chance"])}else{Value::Null})});
            for key in [
                "via",
                "conditions",
                "cargo_index",
                "cargo_selection_probability",
                "cargo_probability_basis",
                "max_reward_distance",
                "spawn_chance",
                "weapon_context",
            ] {
                if let Some(value) = source.get(key) {
                    route[key] = value.clone();
                }
            }
            if event == "drop" {
                route["authored_drop_chance"] = source["authored_drop_chance"].clone();
                if overridden.contains(path) {
                    route["selection"] = json!("region_fallback");
                }
            }
            add(&mut groups, path, name, route);
            continue;
        }
        for mode in ["normal", "cogah"] {
            let Some(table) = source["tables"][mode].as_str() else {
                continue;
            };
            let mut route = json!({"table":table,"event":if event=="spawn"{"drop"}else{event},
                "mode":mode,"enabled":if event=="collect" {json!(true)}else{enabled(&source["authored_drop_chance"])}});
            if let Some(context) = source.get("weapon_context") {
                route["weapon_context"] = context.clone();
            }
            if event == "spawn" {
                route["region"] = source["region"].clone();
                route["selection"] = json!("region_override");
                route["authored_drop_chance"] = source["authored_drop_chance"].clone();
            }
            add(&mut groups, path, name.clone(), route.clone());
            if event == "collect" {
                route["via"] = json!(path);
                for enemy in source["enemies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let mut enemy_route = route.clone();
                    if let Some(trigger) = source["enemy_triggers"]
                        .get(enemy)
                        .filter(|v| v.is_string())
                    {
                        enemy_route["trigger"] = trigger.clone();
                    }
                    add(&mut groups, enemy, name_for(enemy), enemy_route);
                }
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|(path, group)| {
            let configured: BTreeSet<_> = group
                .routes
                .iter()
                .filter_map(|r| r["table"].as_str())
                .collect();
            if configured.len() < 2 {
                return None;
            }
            let possible: BTreeSet<_> = group
                .routes
                .iter()
                .filter(|r| r["enabled"] != false)
                .filter_map(|r| r["table"].as_str())
                .collect();
            Some(
                json!({"path":path,"name":group.name,"table_count":possible.len(),
            "configured_table_count":configured.len(),"routes":group.routes}),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sources() -> Vec<Value> {
        vec![
            json!({"path":"/SF/Boss","name":"Boss","event":"drop","table":"/SF/OldTable","authored_drop_chance":0}),
            json!({"path":"/SF/Chest","name":"Chest","event":"collect","tables":{"normal":"/SF/Normal","cogah":"/SF/Hard"},"enemies":["/SF/Boss"]}),
        ]
    }
    #[test]
    fn disabled_direct_route_does_not_inflate_available_table_count() {
        let groups = groups(&sources(), str::to_owned);
        let boss = groups.iter().find(|g| g["path"] == "/SF/Boss").unwrap();
        assert_eq!(boss["configured_table_count"], 3);
        assert_eq!(boss["table_count"], 2);
        let routes = boss["routes"].as_array().unwrap();
        assert_eq!(routes[0]["enabled"], false);
        assert_eq!(routes[1]["via"], "/SF/Chest");
        assert_eq!(routes[1]["mode"], "normal");
        assert_eq!(routes[2]["mode"], "cogah");
    }
    #[test]
    fn repeated_references_and_same_mode_table_count_once() {
        let mut input = sources();
        input[0]["table"] = json!("/SF/Normal");
        input[0]["authored_drop_chance"] = json!(1);
        input[1]["tables"]["cogah"] = json!("/SF/Normal");
        input.push(input[0].clone());
        assert!(groups(&input, str::to_owned).is_empty());
    }
    #[test]
    fn region_tables_replace_the_default_route() {
        let input = vec![
            json!({"path":"/SF/Boss","event":"drop","table":"/SF/Base","authored_drop_chance":1}),
            json!({"path":"/SF/Boss","event":"spawn","region":"/SF/Region","tables":{"normal":"/SF/Other","cogah":"/SF/Other"},"authored_drop_chance":1}),
        ];
        let output = groups(&input, str::to_owned);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["table_count"], 2);
        assert_eq!(output[0]["routes"][0]["selection"], "region_fallback");
        assert_eq!(output[0]["routes"][1]["selection"], "region_override");
    }
    #[test]
    fn unknown_chance_and_null_hard_table_are_not_guessed() {
        let mut input = sources();
        input[0]["authored_drop_chance"] = Value::Null;
        input[1]["tables"]["cogah"] = Value::Null;
        let output = groups(&input, str::to_owned);
        let boss = output.iter().find(|g| g["path"] == "/SF/Boss").unwrap();
        assert_eq!(boss["table_count"], 2);
        assert!(boss["routes"][0]["enabled"].is_null());
        assert_eq!(boss["routes"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn conditional_cargo_routes_keep_their_conditions_and_separate_events() {
        let input = vec![
            json!({"path":"/SF/Drone","event":"drop","table":"/SF/Direct","authored_drop_chance":1}),
            json!({"path":"/SF/Drone","event":"cargo_open","table":"/SF/ChestTable","via":"/SF/Chest","cargo_index":2,"cargo_selection_probability":0.5,"conditions":["not_siege_mine_laying"]}),
            json!({"path":"/SF/Drone","event":"cargo_recovery","table":"/SF/Recovery","via":"/SF/Chest","cargo_index":2,"cargo_selection_probability":0.5,"conditions":["not_siege_mine_laying","moved_too_far"]}),
        ];
        let output = groups(&input, str::to_owned);
        assert_eq!(output[0]["table_count"], 3);
        let route = &output[0]["routes"][2];
        assert_eq!(route["event"], "cargo_recovery");
        assert_eq!(route["via"], "/SF/Chest");
        assert_eq!(route["cargo_selection_probability"], 0.5);
        assert_eq!(route["conditions"][1], "moved_too_far");
        assert!(route["enabled"].is_null());
    }
}
