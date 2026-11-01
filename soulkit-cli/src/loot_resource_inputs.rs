//! Package-informed conditional selector inputs, never a runtime observation.
//!
//! Direct InventoryItem and upgrade Rarity fields are tied to native helpers.
//! Pickup item references require an
//! explicitly resolved resource path. Missing package data is not native null.

use serde_json::Value;
use soulframe_droptables::resolved_raw::DropCategory;
use soulframe_droptables::resolved_selector::{
    CategoryInterpolation, EntryEvidence, SelectorInputs,
};
use std::collections::{BTreeMap, BTreeSet};

/// Use inherited, explicitly authored rarity where the reference is resolvable.
/// Missing or unfamiliar data leaves rarity unknown; no serialized/Common fallback.
/// Resource acceptance and successful loading remain declared scenario assumptions.
pub fn package_baseline_inputs(
    packages: &Value,
    category: &DropCategory,
    level: u32,
) -> SelectorInputs {
    let mut entry_evidence = BTreeMap::new();
    for group in &category.level_groups {
        for entry in &group.items {
            let (runtime_rarity, source) = match entry.reference_kind {
                1 | 2 => inherited_rarity(packages, &entry.path),
                0 => match inherited_field(packages, &entry.path, "ItemType") {
                    Lookup::Found(value, pointer) => match value.as_str() {
                        Some(item) if item.starts_with('/') => {
                            let (rarity, source) = inherited_rarity(packages, item);
                            (rarity, format!("pickup ItemType={item} at {pointer}; {source}"))
                        }
                        _ => (
                            None,
                            format!("unresolved pickup item resource: ItemType at {pointer} is not an absolute resource path"),
                        ),
                    },
                    Lookup::Missing(reason) => (
                        None,
                        format!("unresolved pickup item resource: {reason}; PickUpItemType and short names are not substituted without mapping evidence"),
                    ),
                },
                kind => (None, format!("unsupported native reference kind {kind}")),
            };
            entry_evidence.insert(
                entry.file_offset,
                EntryEvidence {
                    accepted: Some(true),
                    runtime_rarity,
                    source: format!("conditional package evidence; resource loading and acceptance assumed: {source}"),
                },
            );
        }
    }
    SelectorInputs {
        generation_level: level,
        random_selection_mode: Some(true),
        category_interpolation: Some(CategoryInterpolation::default()),
        entry_evidence,
        assumptions: vec![
            "Conditional item selection after the category collector runs; not per enemy or per run".into(),
            "All level-eligible references load successfully and pass native resource/exclusion/category-resource checks".into(),
            "Loaded resources retain the explicitly authored inherited package Rarity; runtime overrides are absent".into(),
            "Missing rarity or pickup item-reference evidence remains unknown; serialized rarity and native null-reference fallback are not substituted".into(),
            "Non-overridden groups use native constructor interpolation defaults [0,100,0,0,0,0]".into(),
            "Runtime force-all-items flag is disabled; authored category all-items flags are checked".into(),
            "Uniform U24 variate; no claim of independent game seeds or repeated invocations".into(),
        ],
    }
}

enum Lookup<'a> {
    Found(&'a Value, String),
    Missing(String),
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

/// Explicit null/wrongly typed fields stop inheritance. An absent field does not.
fn inherited_field<'a>(packages: &'a Value, resource: &str, key: &str) -> Lookup<'a> {
    let mut current = resource.to_owned();
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current.clone()) {
            return Lookup::Missing(format!(
                "inheritance cycle at {current} while resolving {key}"
            ));
        }
        let Some(record) = packages.get(&current).and_then(Value::as_object) else {
            return Lookup::Missing(format!(
                "package resource {current} is missing or malformed while resolving {key}"
            ));
        };
        match record.get("attributes") {
            Some(Value::Object(attributes)) => {
                if let Some(value) = attributes.get(key) {
                    return Lookup::Found(
                        value,
                        format!(
                            "/{}/attributes/{}",
                            escape_pointer(&current),
                            escape_pointer(key)
                        ),
                    );
                }
            }
            Some(_) => {
                return Lookup::Missing(format!(
                    "package attributes for {current} are malformed while resolving {key}"
                ))
            }
            None => {}
        }
        let Some(parent) = record
            .get("parent")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
        else {
            return Lookup::Missing(format!(
                "no authored {key} in the complete supplied parent chain for {resource}"
            ));
        };
        current = parent.to_owned();
    }
}

fn inherited_rarity(packages: &Value, resource: &str) -> (Option<u32>, String) {
    match inherited_field(packages, resource, "Rarity") {
        Lookup::Found(value, pointer) => {
            let rarity = match value.as_str() {
                Some("COMMON") => Some(0),
                Some("UNCOMMON") => Some(1),
                Some("RARE") => Some(2),
                Some("LEGENDARY") => Some(3),
                _ => None,
            };
            let label = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            let status = if rarity.is_some() {
                "authored"
            } else {
                "unresolved enum"
            };
            (
                rarity,
                format!("{status} Rarity={label} for {resource} at {pointer}"),
            )
        }
        Lookup::Missing(reason) => (None, reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn category(kind: u32, path: &str) -> DropCategory {
        serde_json::from_value(json!({
            "index":7,"file_offset":0,"end_offset":100,"prefix_bytes":[0,0,0],
            "reserved_field":0,"modifier_reference":null,"chance_file_offset":3,
            "chance":0.8,"minimum_drops":0,"maximum_drops":1,"quality_indices":[2,3,4,5,6],
            "level_groups":[{"index":0,"file_offset":10,"end_offset":100,"interpolated":false,
                "interpolation_parameters":null,"minimum_level":0.0,"maximum_level":100.0,
                "items":[{"index":0,"file_offset":20,"end_offset":80,"reference_kind":kind,
                    "path":path,"bias":0.0,"rarity_index":0,"authored_weight":0.755}]}]
        }))
        .unwrap()
    }

    #[test]
    fn direct_kinds_use_inherited_rarity_and_exact_escaped_pointer() {
        let packages = json!({
            "/child":{"attributes":{},"parent":"/parent~special"},
            "/parent~special":{"attributes":{"Rarity":"RARE"},"parent":null}
        });
        for kind in [1, 2] {
            let result = package_baseline_inputs(&packages, &category(kind, "/child"), 15);
            let e = &result.entry_evidence[&20];
            assert_eq!(e.runtime_rarity, Some(2));
            assert!(e.source.contains("/~1parent~0special/attributes/Rarity"));
            assert!(e.source.contains("acceptance assumed"));
        }
    }

    #[test]
    fn pickup_uses_item_resource_rarity_not_pickup_rarity() {
        let packages = json!({
            "/pickup":{"attributes":{"Rarity":"LEGENDARY"},"parent":"/pickup_base"},
            "/pickup_base":{"attributes":{"ItemType":"/item"},"parent":null},
            "/item":{"attributes":{"Rarity":"UNCOMMON"},"parent":null}
        });
        let result = package_baseline_inputs(&packages, &category(0, "/pickup"), 1);
        let e = &result.entry_evidence[&20];
        assert_eq!(e.runtime_rarity, Some(1));
        assert!(e.source.contains("/~1pickup_base/attributes/ItemType"));
        assert!(e.source.contains("/~1item/attributes/Rarity"));
    }

    #[test]
    fn missing_or_null_rarity_never_defaults_to_serialized_common() {
        let packages = json!({
            "/missing":{"attributes":{},"parent":null},
            "/null":{"attributes":{"Rarity":null},"parent":"/parent"},
            "/parent":{"attributes":{"Rarity":"COMMON"},"parent":null}
        });
        for path in ["/missing", "/null", "/absent"] {
            assert_eq!(
                package_baseline_inputs(&packages, &category(1, path), 1).entry_evidence[&20]
                    .runtime_rarity,
                None
            );
        }
    }

    #[test]
    fn unknown_spelling_relative_item_and_unproven_pickup_field_stay_unknown() {
        let packages = json!({
            "/weird":{"attributes":{"Rarity":"RARE_PLUS"},"parent":null},
            "/relative":{"attributes":{"ItemType":"ShortName"},"parent":null},
            "/other":{"attributes":{"PickUpItemType":"/item"},"parent":null},
            "/item":{"attributes":{"Rarity":"RARE"},"parent":null}
        });
        for (kind, path) in [(1, "/weird"), (0, "/relative"), (0, "/other")] {
            assert_eq!(
                package_baseline_inputs(&packages, &category(kind, path), 1).entry_evidence[&20]
                    .runtime_rarity,
                None
            );
        }
    }

    #[test]
    fn parent_cycles_and_malformed_fields_are_bounded_unknowns() {
        let packages = json!({
            "/a":{"attributes":{},"parent":"/b"},
            "/b":{"attributes":{},"parent":"/a"},
            "/bad":{"attributes":null,"parent":"/good"},
            "/good":{"attributes":{"Rarity":"RARE"},"parent":null}
        });
        let a = package_baseline_inputs(&packages, &category(1, "/a"), 1);
        assert!(a.entry_evidence[&20].source.contains("cycle"));
        assert_eq!(a.entry_evidence[&20].runtime_rarity, None);
        assert_eq!(
            package_baseline_inputs(&packages, &category(1, "/bad"), 1).entry_evidence[&20]
                .runtime_rarity,
            None
        );
    }
}
