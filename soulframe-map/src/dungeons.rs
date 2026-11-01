//! Entrance waypoints are distinct from encounter hints, initialization scripts
//! and exit portals. Keep those records separate instead of duplicating markers.
use crate::level::LevelEntity;
use crate::map_json::record;
use crate::properties::fields;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const WAYPOINT: &str = "/SF/Types/Gameplay/DungeonEntranceWaypoint";
// The inspected waypoint/portal pairs are 2.68–4.64 world units apart. Their
// nearest alternative portals are at least 79 units away. Require a unique,
// reciprocal match in this small radius; do not assign the nearest at any cost.
const PORTAL_RADIUS: f32 = 8.0;

pub(crate) struct Dungeons {
    pub entrances: BTreeMap<&'static str, Vec<Value>>,
    pub named: BTreeMap<String, Value>,
    pub portals: BTreeMap<&'static str, Vec<Value>>,
    pub hints: BTreeMap<&'static str, Vec<Value>>,
    pub unresolved: Vec<Value>,
    pub entrance_indices: BTreeSet<usize>,
}

fn groups() -> BTreeMap<&'static str, Vec<Value>> {
    ["Crypt", "Glades", "Neathuns", "Undercity", "FallenFortress"]
        .into_iter()
        .map(|category| (category, Vec::new()))
        .collect()
}

fn portal_category(path: &str) -> Option<&'static str> {
    let relative = path.strip_prefix("/SF/Levels/").unwrap_or(path);
    match relative.strip_prefix("Procs/LevelPortals/")? {
        "CryptStreamingPortalDoor" => Some("Crypt"),
        "GoAPortalSpawn" | "GoAPortalSpawnDoorway" => Some("Glades"),
        "NEAEntrancePortal" | "NEAEntranceRootsPortal" => Some("Neathuns"),
        "UnderCityPortalStreamingEntrance" | "UnderCityPortalStreamingEntranceToPortCity" => {
            Some("Undercity")
        }
        "FallenFortressEntrancePortal" => Some("FallenFortress"),
        _ => None,
    }
}

fn distance(a: &LevelEntity, b: &LevelEntity) -> f32 {
    a.position
        .iter()
        .zip(b.position)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt()
}

pub(crate) fn extract(entities: &[LevelEntity], source: &str) -> Dungeons {
    let mut result = Dungeons {
        entrances: groups(),
        named: BTreeMap::new(),
        portals: groups(),
        hints: groups(),
        unresolved: Vec::new(),
        entrance_indices: BTreeSet::new(),
    };
    let portals: Vec<_> = entities
        .iter()
        .filter_map(|e| portal_category(&e.type_path).map(|category| (e, category)))
        .collect();
    let waypoints: Vec<_> = entities
        .iter()
        .filter(|e| e.type_path == WAYPOINT)
        .collect();
    let mut matched_portals = BTreeSet::new();
    for waypoint in &waypoints {
        let nearby: Vec<_> = portals
            .iter()
            .filter(|(portal, _)| distance(waypoint, portal) <= PORTAL_RADIUS)
            .collect();
        let mut value = record(waypoint, source);
        if nearby.len() != 1 {
            result.unresolved.push(json!({"waypoint":value,"reason":"expected exactly one entrance portal within radius",
                "nearby_portal_count":nearby.len(),"radius":PORTAL_RADIUS}));
            continue;
        }
        let (portal, category) = *nearby[0];
        if waypoints
            .iter()
            .filter(|other| distance(other, portal) <= PORTAL_RADIUS)
            .count()
            != 1
        {
            result.unresolved.push(
                json!({"waypoint":value,"reason":"multiple waypoints share the entrance portal",
                "portal":record(portal,source)}),
            );
            continue;
        }
        let props = fields(&waypoint.text);
        value["name"] = json!(props.get("Tag"));
        value["category"] = json!(category);
        value["marker_kind"] = json!("dungeon_entrance");
        value["availability"] =
            json!("authored entrance waypoint; quest and access requirements are not evaluated");
        value["portal"] = record(portal, source);
        value["portal_association"] = json!({"method":"inferred from a unique reciprocal spatial match",
            "distance":distance(waypoint,portal),"maximum_distance":PORTAL_RADIUS});
        result
            .named
            .insert(value["id"].as_str().unwrap().to_owned(), value.clone());
        result.entrances.get_mut(category).unwrap().push(value);
        result.entrance_indices.insert(waypoint.index);
        matched_portals.insert(portal.index);
    }
    for (portal, category) in portals {
        let mut value = record(portal, source);
        value["category"] = json!(category);
        value["marker_kind"] = json!("entrance_portal");
        result
            .portals
            .get_mut(category)
            .unwrap()
            .push(value.clone());
        if !matched_portals.contains(&portal.index) {
            result.unresolved.push(
                json!({"portal":value,"reason":"no unambiguous entrance waypoint association"}),
            );
        }
    }
    for entity in entities {
        if let Some(category) = entity
            .type_path
            .strip_prefix("/SF/Types/Game/Encounters/Locations/DungeonEntranceHint")
        {
            if let Some(group) = result.hints.get_mut(category) {
                let mut value = record(entity, source);
                value["category"] = json!(category);
                value["marker_kind"] = json!("encounter_hint");
                value["availability"] =
                    json!("encounter hint; position is not an entrance waypoint");
                group.push(value);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(index: usize, path: &str, x: f32) -> LevelEntity {
        LevelEntity {
            index,
            header_offset: index * 100,
            body_offset: index * 20,
            type_path: path.into(),
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scope: "/Layer1/".into(),
            instance: index as u32,
            text: format!("Instance={index}\nTag=Entrance{index}\n"),
        }
    }
    #[test]
    fn uses_waypoint_coordinates_and_excludes_exits_hints_and_controllers() {
        let entities = vec![
            entity(0, WAYPOINT, 1.0),
            entity(
                1,
                "Procs/LevelPortals/UnderCityPortalStreamingEntranceToPortCity",
                4.0,
            ),
            entity(
                2,
                "Procs/LevelPortals/UnderCityPortalOpeningStreamingExit",
                1.0,
            ),
            entity(
                3,
                "/SF/Types/Game/Encounters/Locations/DungeonEntranceHintUndercity",
                20.0,
            ),
            entity(4, "/SF/Types/Gameplay/DungeonEntranceInit", 1.0),
        ];
        let result = extract(&entities, "fixture");
        assert_eq!(result.entrances["Undercity"].len(), 1);
        assert_eq!(result.entrances["Undercity"][0]["position"]["x"], 1.0);
        assert_eq!(
            result.entrances["Undercity"][0]["portal"]["position"]["x"],
            4.0
        );
        assert_eq!(result.hints["Undercity"].len(), 1);
        assert!(result.unresolved.is_empty());
    }
    #[test]
    fn refuses_ambiguous_and_distant_portal_matches() {
        for portal_positions in [vec![1.0, 2.0], vec![100.0]] {
            let mut entities = vec![entity(0, WAYPOINT, 0.0)];
            for (index, x) in portal_positions.into_iter().enumerate() {
                entities.push(entity(index + 1, "Procs/LevelPortals/GoAPortalSpawn", x));
            }
            let result = extract(&entities, "fixture");
            assert!(result.named.is_empty());
            assert!(!result.unresolved.is_empty());
        }
        let result = extract(
            &[
                entity(0, WAYPOINT, 0.0),
                entity(1, WAYPOINT, 1.0),
                entity(2, "Procs/LevelPortals/GoAPortalSpawn", 2.0),
            ],
            "fixture",
        );
        assert!(result.named.is_empty());
    }
    #[test]
    fn recognizes_entrance_families_without_including_hub_or_exit_portals() {
        for (typ, category) in [
            ("CryptStreamingPortalDoor", "Crypt"),
            ("GoAPortalSpawn", "Glades"),
            ("NEAEntranceRootsPortal", "Neathuns"),
            ("UnderCityPortalStreamingEntrance", "Undercity"),
            ("FallenFortressEntrancePortal", "FallenFortress"),
        ] {
            assert_eq!(
                portal_category(&format!("/SF/Levels/Procs/LevelPortals/{typ}")),
                Some(category)
            );
        }
        assert_eq!(
            portal_category("Procs/LevelPortals/FeyHubEntrancePortal"),
            None
        );
        assert_eq!(
            portal_category("Procs/LevelPortals/CryptStreamingPortalExitDoor"),
            None
        );
    }
}
