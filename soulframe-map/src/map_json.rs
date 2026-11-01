use crate::level::LevelEntity;
use crate::properties::{deep_field, fields, list, nested_field, vector};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
fn position(p: [f32; 3]) -> Value {
    json!({"x":p[0], "y":p[1], "z":p[2]})
}
fn boolean(value: Option<&String>) -> Value {
    match value.map(String::as_str) {
        Some("0") => json!(false),
        Some("1") => json!(true),
        _ => Value::Null,
    }
}

pub(crate) fn lore_name<'a>(packages: &'a Value, resource: &str) -> Option<&'a str> {
    let mut current = resource;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        let definition = packages.get(current)?;
        if let Some(name) = definition.get("attributes")?.get("Name") {
            return name.as_str().filter(|s| s.starts_with("/SF/Language/Locations/"));
        }
        current = definition.get("parent")?.as_str()?;
    }
    None
}

pub(crate) fn record(e: &LevelEntity, source: &str) -> Value {
    let props = fields(&e.text);
    json!({
        "id": format!("{}{}:{}", e.scope, e.type_path.trim_start_matches('/'), e.instance),
        "entity_type": leaf(&e.type_path), "type_path": e.type_path,
        "instance_id": e.instance, "layer": e.scope.split('/').find(|s| s.starts_with("Layer")),
        "scope": e.scope, "position": position(e.position), "rotation": e.rotation,
        "position_source": source, "position_offset": e.header_offset,
        "body_offset": e.body_offset, "record_index": e.index,
        "tag": props.get("Tag"), "enabled": boolean(props.get("Enabled")),
        "start_hidden": boolean(props.get("StartHidden")),
        "availability": "authored placement; runtime activation is not evaluated"
    })
}

fn collection(description: &str, key: &str, values: Vec<Value>) -> Value {
    let mut result = json!({"description":description, "count":values.len()});
    result[key] = json!(values);
    result
}

/// Rotate a local attachment offset by the parent's world quaternion.
fn child_position(parent: &LevelEntity, local: [f32; 3], scale: f32) -> [f32; 3] {
    let [qx, qy, qz, qw] = parent.rotation;
    let [x, y, z] = local.map(|v| v * scale);
    let t = [
        2.0 * (qy * z - qz * y),
        2.0 * (qz * x - qx * z),
        2.0 * (qx * y - qy * x),
    ];
    [
        parent.position[0] + x + qw * t[0] + qy * t[2] - qz * t[1],
        parent.position[1] + y + qw * t[1] + qz * t[0] - qx * t[2],
        parent.position[2] + z + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

pub(crate) fn documents(
    entities: &[LevelEntity],
    source: &str,
    event_text: &str,
    location_names: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Value>> {
    let mut files = BTreeMap::new();
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for name in [
        "named_locations",
        "discovery_triggers",
        "encounter_hints",
        "loot_chests",
        // Paused at the user's request. Preserve these for later work.
        // "campfires",
        // "envoy_camps",
        // "revive_shrines",
        "world_trees",
        "ode_organs",
        "ode_checkpoints",
        "atmos_polluters",
        "avakot_spawns",
        "sap_trees",
        "vadagar_pools",
        "faction_enclaves",
        "boss_encounters",
        "subboss_markers",
        "graveyard_zones",
        "ancestors",
        "oro_chests",
        "collectable_notes",
    ] {
        groups.insert(name, Vec::new());
    }
    let dungeons = crate::dungeons::extract(entities, source);
    let mut tiles = Vec::new();
    let mut unresolved_children = Vec::new();
    let mut inventory = Vec::new();
    for e in entities {
        let props = fields(&e.text);
        let typ = leaf(&e.type_path);
        let mut value = record(e, source);
        let mut categories = BTreeSet::new();
        let mut add = |category| {
            categories.insert(category);
        };
        let loc =
            nested_field(&e.text, "_locTag").filter(|s| s.starts_with("/SF/Language/Locations/"));
        if typ == "DiscoveryTrigger" {
            add("discovery_triggers");
            if let Some(loc) = loc {
                add("named_locations");
                // Many authored scripts reuse FortCurlail as their notification
                // text. The linked lore resource supplies the actual map name.
                let lore = nested_field(&e.text, "_loreFragmentRes")
                    .filter(|s| s.starts_with("/SF/Types/Quests/LocationLoreQuests/"));
                let resolved = match lore {
                    Some(resource) => location_names.get(resource).map(String::as_str),
                    None => Some(loc),
                };
                value["discovery_loc_tag"] = json!(loc);
                value["lore_resource"] = json!(lore);
                value["name"] = json!(resolved.map(leaf).unwrap_or_else(||
                    leaf(lore.unwrap()).trim_end_matches("LocationLoreFragment").trim_end_matches("CampLoreFragment")));
                value["loc_tag"] = json!(resolved);
                value["label_source"] = json!(if resolved.is_none() { "unresolved_lore_resource" }
                    else if lore.is_some() { "lore_resource_name" } else { "discovery_script" });
                value["has_position"] = json!(true);
                value["radius"] = json!(props.get("Radius").and_then(|s| s.parse::<f64>().ok()));
            }
        }
        if e.type_path.starts_with("/SF/Types/Game/Encounters/") && typ.contains("Hint") {
            add("encounter_hints");
            value["type"] = json!(typ);
            value["encounter_tags"] = json!(props
                .get("EncounterTags")
                .map(|s| list(s))
                .unwrap_or_default());
            value["available_encounters"] = json!(props
                .get("AvailableEncounters")
                .map(|s| list(s))
                .unwrap_or_default());
            value["availability"] =
                json!("candidate encounter location; selection and activation occur at runtime");
        }
        if e.type_path.starts_with("/SF/Types/Gameplay/LootChestSpawn") {
            add("loot_chests");
            value["availability"] = json!("loot spawn candidate; a chest is not guaranteed");
        }
        if typ == "LootChestSpawnWishingWell" {
            add("oro_chests");
        }
        // Paused: these identify Ode encounter camps, not verified Envoy camps.
        // if typ.starts_with("CampfireEncounterHint") {
        //     add("campfires");
        // }
        // Revive shrines: no verified mapping yet. Do not use decorative
        // SpiritCreatures/Shrine records as a substitute.
        if typ == "MapTreeBaseAction" {
            add("world_trees");
        }
        if typ == "PollutionTowerEncounterHint" {
            add("ode_organs");
        }
        if typ == "OdeCheckpointHint" {
            add("ode_checkpoints");
        }
        if typ == "PolluterEncounterHintDynamic" {
            add("atmos_polluters");
        }
        if e.type_path == "/SF/Types/Npcs/AvakotNpc"
            || e.type_path == "/SF/Types/Npcs/AvakotVendorNpc"
        {
            add("avakot_spawns");
            value["npc_type"] = json!(typ);
        }
        if typ == "WevetWoodSapManualPickup" {
            add("sap_trees");
            value["source"] = json!("level");
        }
        if typ == "VadagarEncounterHint" {
            add("vadagar_pools");
            value["encounter_types"] = value["available_encounters"].clone();
            value["has_stag"] = json!(props
                .get("AvailableEncounters")
                .is_some_and(|s| s.contains("Stag")));
            value["has_bear"] = json!(props
                .get("AvailableEncounters")
                .is_some_and(|s| s.contains("Bear")));
        }
        let faction = match typ {
            "MapChampionFactionMarkerInfo" => Some("Champion"),
            "MapFeyFactionMarkerInfo" => Some("Alca's Children"),
            "MapSilentRoseFactionMarkerInfo" => Some("Silent Rose"),
            _ => None,
        };
        if let Some(name) = faction {
            add("faction_enclaves");
            value["faction"] = json!(name);
        }
        // Retain the dungeon family in the general encounter exports too.
        // These hints are not classified as entrance waypoints.
        if let Some(category) = typ.strip_prefix("DungeonEntranceHint") {
            if dungeons.hints.contains_key(category) {
                value["category"] = json!(category);
            }
        }
        if e.type_path.starts_with("/SF/Types/Game/Encounters/")
            && typ.contains("Hint")
            && (typ.contains("Boss")
                || typ.contains("MendicantKing")
                || props
                    .get("AvailableEncounters")
                    .is_some_and(|s| s.contains("Boss") || s.contains("Miniboss")))
        {
            add("boss_encounters");
            value["camp_tag"] = json!(props.get("Tag"));
            value["subboss_type"] = json!(props
                .get("AvailableEncounters")
                .and_then(|s| list(s)
                    .into_iter()
                    .find(|p| p.to_ascii_lowercase().contains("boss")))
                .map(leaf)
                .unwrap_or(typ));
            value["is_siege"] = json!(typ.contains("Siege"));
        }
        if typ == "SubBossRednightMarkerInfo" {
            add("subboss_markers");
        }
        if typ == "GraveyardHint" {
            add("graveyard_zones");
        }
        if typ == "GodsAndGhostsContainerHint" {
            add("ancestors");
        }
        if e.type_path
            .starts_with("/SF/Types/Gameplay/CollectableNotes/")
        {
            add("collectable_notes");
        }
        for category in &categories {
            groups.get_mut(category).unwrap().push(value.clone());
        }

        // Sap pickups attached to decorative trees are explicit child records.
        // Their LocalPosition is not the parent's world position.
        for property in ["ServerChildren", "AutoSimChildren"] {
            if let Some(children) = props.get(property) {
                for (index, child) in list(children).into_iter().enumerate() {
                    let child = child
                        .strip_prefix('{')
                        .and_then(|s| s.strip_suffix('}'))
                        .unwrap_or(child);
                    let cp = fields(child);
                    if !cp
                        .get("Type")
                        .is_some_and(|s| leaf(s) == "WevetWoodSapManualPickup")
                    {
                        continue;
                    }
                    let local = cp.get("LocalPosition").and_then(|s| vector(s));
                    if cp.get("BoneName").is_some_and(|s| !s.is_empty())
                        || cp.get("RelativeToBounds").is_some_and(|s| s != "0")
                        || local.is_none()
                    {
                        unresolved_children.push(json!({"parent_id":value["id"], "property":property, "index":index,
                            "reason":"attachment requires a bone, bounds or unresolved local transform"}));
                        continue;
                    }
                    let mut child_value = value.clone();
                    child_value["id"] = json!(format!(
                        "{}:{property}:{index}",
                        value["id"].as_str().unwrap()
                    ));
                    child_value["type_path"] = json!(cp["Type"]);
                    child_value["entity_type"] = json!("WevetWoodSapManualPickup");
                    child_value["source"] = json!("attached_child");
                    child_value["parent_id"] = value["id"].clone();
                    child_value["local_position"] = json!(local.unwrap());
                    child_value.as_object_mut().unwrap().remove("rotation");
                    child_value["chance"] =
                        json!(cp.get("Chance").and_then(|s| s.parse::<u32>().ok()));
                    let scale = props
                        .get("MeshScale")
                        .or_else(|| props.get("Scale"))
                        .map(|s| s.parse::<f32>())
                        .transpose()
                        .context("unsupported attachment scale")?
                        .unwrap_or(1.0);
                    ensure!(scale.is_finite(), "non-finite attachment scale");
                    child_value["position"] = position(child_position(e, local.unwrap(), scale));
                    groups.get_mut("sap_trees").unwrap().push(child_value);
                }
            }
        }

        if typ == "EPOutdoorZoneAttribs" {
            let path = props
                .get("MapTexture")
                .context("outdoor zone missing MapTexture")?;
            let index: u32 = leaf(path)
                .strip_suffix("_cmm.png")
                .context("unsupported map texture")?
                .parse()?;
            let bounds = fields(
                props
                    .get("MapBounds")
                    .context("outdoor zone missing MapBounds")?
                    .trim()
                    .strip_prefix('{')
                    .and_then(|s| s.strip_suffix('}'))
                    .context("invalid MapBounds")?,
            );
            let min = vector(bounds.get("Min").context("missing minimum map bound")?)
                .context("invalid minimum map bound")?;
            let max = vector(bounds.get("Max").context("missing maximum map bound")?)
                .context("invalid maximum map bound")?;
            let (width, depth) = (max[0] - min[0], max[2] - min[2]);
            ensure!(width > 0.0 && depth > 0.0, "invalid tile bounds");
            tiles.push(
                json!({"index":index, "image":format!("midrathRedux/{index}.webp"), "texture":path,
                "world_bounds":{"min_x":e.position[0]-width/2.0, "max_x":e.position[0]+width/2.0,
                    "min_z":e.position[2]-depth/2.0, "max_z":e.position[2]+depth/2.0},
                "map_bounds":{"min":min,"max":max}}),
            );
        }
        if dungeons.entrance_indices.contains(&e.index) {
            categories.insert("dungeon_entrances");
        }
        value["categories"] = json!(categories);
        value["properties"] = json!(props);
        inventory.push(value);
    }
    tiles.sort_by_key(|t| t["index"].as_u64());
    ensure!(!tiles.is_empty(), "no outdoor map tile placements in level");
    let mut indices = BTreeSet::new();
    ensure!(
        tiles
            .iter()
            .all(|t| indices.insert(t["index"].as_u64().unwrap())),
        "duplicate tile placements"
    );
    for (name, values) in groups {
        let document = match name {
            "named_locations" | "discovery_triggers" | "encounter_hints" | "boss_encounters" => {
                json!(values)
            }
            "faction_enclaves" => collection("Faction map markers", "enclaves", values),
            "vadagar_pools" => {
                let mut v = collection(
                    "Vadagar encounter candidates; null enabled means no explicit level override",
                    "pools",
                    values,
                );
                let pools = v["pools"].as_array().unwrap();
                let active = pools.iter().filter(|v| v["enabled"] == true).count();
                let disabled = pools.iter().filter(|v| v["enabled"] == false).count();
                let unknown = pools.len() - active - disabled;
                v["active_pools"] = json!(active);
                v["disabled_pools"] = json!(disabled);
                v["unknown_pools"] = json!(unknown);
                v
            }
            "subboss_markers" => json!([{"type":"SubBossMarkerInfo","markers":values}]),
            "graveyard_zones" => collection("Graveyard location hints", "hints", values),
            "ancestors" => ancestor_document(values, event_text),
            "sap_trees" => {
                let attached = values
                    .iter()
                    .filter(|v| v["source"] == "attached_child")
                    .count();
                let mut v = collection(
                    "Explicit Wevetwood sap pickups, including transformed attachments",
                    "positions",
                    values,
                );
                v["prefab_count"] = json!(attached);
                v["standalone_count"] = json!(v["count"].as_u64().unwrap() - attached as u64);
                v
            }
            _ => collection(name, "positions", values),
        };
        files.insert(format!("{name}.json"), document);
    }
    files.insert("dungeon_entrances.json".into(), json!(dungeons.entrances));
    files.insert("named_dungeons.json".into(), json!(dungeons.named));
    files.insert("dungeon_portals.json".into(), json!(dungeons.portals));
    files.insert("dungeon_entrance_hints.json".into(), json!(dungeons.hints));
    files.insert("tile_layout.json".into(), json!({"tiles":tiles,
        "coordinate_system":"x east, y elevation, z north; tile images are north-up",
        "pixel_transform":"pixel_x=(x-min_x)*pixels_per_world_unit; pixel_y=(max_z-z)*pixels_per_world_unit",
        "note":"Use world_bounds to place tiles; filename ordering is column-major, south to north."}));
    files.insert("entities.json".into(), json!(inventory));
    files.insert("coverage.json".into(), json!({
        "position_method":"structured H/B entity records (level format tag 212), paired by ordinal and checked against instance IDs",
        "runtime_state_evaluated":false,
        "disabled_categories": ["campfires", "envoy_camps", "revive_shrines"],
        "unresolved_dungeon_records": dungeons.unresolved,
        "unresolved_children":unresolved_children,
        "limitations":["Only the base MidrathRedux level is included; quest .tlevel variants and procedural dungeon interiors are separate assets.",
            "Type defaults, inheritance, scripts, encounter selection and player progression are not evaluated. enabled/start_hidden are null when absent from the level override.",
            "Category rules use explicit types and supported child records. entities.json retains all authored records for further classification."]
    }));
    Ok(files)
}

fn ancestor_document(values: Vec<Value>, event_text: &str) -> Value {
    let strings = |key| {
        deep_field(event_text, key)
            .map(|s| list(&s).into_iter().map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let resources = strings("_npcResources");
    let weights = strings("_npcWeights");
    let candidates: Vec<_> = resources.iter().enumerate().map(|(index, resource)| {
        let name = leaf(resource).trim_end_matches("BoonNpc");
        let mut label = String::new();
        for (i, ch) in name.chars().enumerate() { if i > 0 && ch.is_ascii_uppercase() { label.push(' '); } label.push(ch); }
        json!({"name":label,"resource":resource,"weight":weights.get(index).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0)})
    }).collect();
    json!({"description":"Authored Gods and Ghosts meeting candidates; not current active spawns",
        "event":"DynamicGodsAndGhostEncounter", "allowed_environment_tags":strings("AllowedEnvironmentTags"),
        "disallowed_tags":strings("DisallowedTags"), "npc_candidates":candidates, "count":values.len(), "positions":values,
        "position_source_note":"Direct transforms of GodsAndGhostsContainerHint level records"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: usize, typ: &str, text: &str) -> LevelEntity {
        LevelEntity {
            index,
            header_offset: 100 + index * 100,
            body_offset: 20 + index * 20,
            type_path: typ.into(),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scope: "/Layer1/".into(),
            instance: index as u32,
            text: text.into(),
        }
    }

    #[test]
    fn classifies_placements_without_promoting_references_or_decorations() {
        let records = vec![
            entity(0,"/Lotus/Types/Gameplay/Eidolon/ZoneAttribs/EPOutdoorZoneAttribs",
                "MapTexture=/SF/Levels/MidrathRedux/0_cmm.png\nMapBounds={Min={0,0,0}\nMax={256,100,256}\n}\n"),
            entity(1,"/SF/Types/Quests/LocationLoreQuests/MapTreeBaseAction","Instance=1\n"),
            entity(2,"/SF/Fx/Cinematics/MapTree/TreeTransformFX","Instance=2\n"),
            entity(3,"Procs/MidrathLevelInfo","References={/SF/Types/Gameplay/LootChestSpawn}\n"),
            entity(4,"/SF/Types/Gameplay/LootChestSpawn","Instance=4\nEnabled=0\n"),
            entity(5,"/SF/Types/Npcs/AvakotVendorNpc","Instance=5\n"),
            entity(6,"/EE/Types/Alias/Decoration",concat!("ServerChildren={{Type=/SF/Types/PickUps/Resources/Organics/Sap/WevetWoodSapManualPickup\n",
                "LocalPosition={1,2,3}\nBoneName=\"\"\nRelativeToBounds=0\nChance=255\n}}\n")),
        ];
        let docs = documents(
            &records,
            "fixture",
            "AllowedEnvironmentTags={GodsAndGhost}\n",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(docs["world_trees.json"]["count"], 1);
        assert_eq!(docs["loot_chests.json"]["count"], 1);
        assert_eq!(docs["loot_chests.json"]["positions"][0]["enabled"], false);
        assert_eq!(
            docs["avakot_spawns.json"]["positions"][0]["npc_type"],
            "AvakotVendorNpc"
        );
        assert!(docs["avakot_spawns.json"]["positions"][0]["enabled"].is_null());
        assert_eq!(
            docs["sap_trees.json"]["positions"][0]["position"],
            json!({"x":1.0,"y":2.0,"z":3.0})
        );
        assert_eq!(docs["entities.json"].as_array().unwrap().len(), 7);
    }

    #[test]
    fn preserves_nested_event_weights_for_calendar_consumers() {
        let doc = ancestor_document(Vec::new(), concat!("AllowedEnvironmentTags={GodsAndGhost}\n",
            "ScriptInstance={_npcResources={/SF/Types/Npcs/BoonNPC/GarrenRoodBoonNpc,/SF/Types/Npcs/BoonNPC/MooseBoonNpc}\n_npcWeights={1,0.4}\n}\n"));
        assert_eq!(doc["npc_candidates"][0]["name"], "Garren Rood");
        assert_eq!(doc["npc_candidates"][1]["weight"], 0.4);
        assert_eq!(doc["allowed_environment_tags"], json!(["GodsAndGhost"]));
    }
    #[test]
    fn location_labels_use_lore_names_instead_of_reused_notification_text() {
        let records = vec![
            entity(0,"/Lotus/Types/Gameplay/Eidolon/ZoneAttribs/EPOutdoorZoneAttribs",
                "MapTexture=/SF/Levels/MidrathRedux/0_cmm.png\nMapBounds={Min={0,0,0}\nMax={256,100,256}\n}\n"),
            entity(1,"/SF/Types/Gameplay/DiscoveryTrigger",
                "Scripts={{Script={_locTag=/SF/Language/Locations/FortCurlail\n_loreFragmentRes=/SF/Types/Quests/LocationLoreQuests/FonnishFortLocationLoreFragment\n}}}"),
            entity(2,"/SF/Types/Gameplay/DiscoveryTrigger",
                "Scripts={{Script={_locTag=/SF/Language/Locations/FortCurlail\n_loreFragmentRes=/SF/Types/Quests/LocationLoreQuests/WyndounLocationLoreFragment\n}}}"),
            entity(3,"/SF/Types/Gameplay/DiscoveryTrigger", "Script={_locTag=/SF/Language/Locations/Curlail\n}"),
        ];
        let names = BTreeMap::from([("/SF/Types/Quests/LocationLoreQuests/FonnishFortLocationLoreFragment".into(),
            "/SF/Language/Locations/FonnishFortRuinsName".into())]);
        let docs = documents(&records, "fixture", "", &names).unwrap();
        let rows = docs["named_locations.json"].as_array().unwrap();
        assert_eq!(rows[0]["loc_tag"], "/SF/Language/Locations/FonnishFortRuinsName");
        assert_eq!(rows[0]["discovery_loc_tag"], "/SF/Language/Locations/FortCurlail");
        assert_eq!(rows[1]["name"], "Wyndoun");
        assert!(rows[1]["loc_tag"].is_null());
        assert_eq!(rows[2]["loc_tag"], "/SF/Language/Locations/Curlail");
    }

    #[test]
    fn lore_name_resolves_inheritance_and_stops_at_missing_or_cyclic_resources() {
        let packages = json!({
            "child":{"parent":"parent", "attributes":{}},
            "parent":{"attributes":{"Name":"/SF/Language/Locations/FonnishFortRuinsName"}},
            "cycle":{"parent":"cycle", "attributes":{}},
            "empty":{"parent":"parent", "attributes":{"Name":null}}
        });
        assert_eq!(lore_name(&packages, "child"), Some("/SF/Language/Locations/FonnishFortRuinsName"));
        assert_eq!(lore_name(&packages, "missing"), None);
        assert_eq!(lore_name(&packages, "cycle"), None);
        assert_eq!(lore_name(&packages, "empty"), None);
    }
    #[test]
    fn rotates_and_scales_child_offset_in_world_space() {
        let e = LevelEntity {
            index: 0,
            header_offset: 0,
            body_offset: 0,
            type_path: "tree".into(),
            position: [10.0, 20.0, 30.0],
            rotation: [
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            scope: "/".into(),
            instance: 0,
            text: String::new(),
        };
        let p = child_position(&e, [1.0, 0.0, 0.0], 2.0);
        assert!((p[0] - 10.0).abs() < 0.0001 && (p[2] - 28.0).abs() < 0.0001);
    }
}
