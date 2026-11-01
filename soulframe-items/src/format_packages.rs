use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Deserialize)]
struct RawEntry {
    parent: String,
    attributes: String,
}

pub fn run(packages_path: &Path, output_path: &Path) -> Result<()> {
    run_with_drop_tables(packages_path, output_path, None)
}

pub fn run_with_drop_tables(
    packages_path: &Path,
    output_path: &Path,
    drop_tables_path: Option<&Path>,
) -> Result<()> {
    println!("Loading {} ...", packages_path.display());
    let raw = fs::read_to_string(packages_path)
        .with_context(|| format!("Reading {:?}", packages_path))?;
    let input: BTreeMap<String, RawEntry> =
        serde_json::from_str(&raw).context("Parsing packages_bin_attrs.json")?;
    println!("  {} entities", input.len());

    println!("Parsing attribute strings ...");
    let mut output: BTreeMap<String, Value> = input
        .into_iter()
        .map(|(path, entry)| {
            let attrs = parse_attrs(&entry.attributes);
            let v = json!({ "parent": entry.parent, "attributes": attrs });
            (path, v)
        })
        .collect();

    if let Some(path) = drop_tables_path {
        println!("Loading {} ...", path.display());
        let raw = fs::read_to_string(path).with_context(|| format!("Reading {:?}", path))?;
        let drop_tables: Vec<Value> =
            serde_json::from_str(&raw).context("Parsing drop_tables.json")?;
        let integrated = integrate_drop_tables(&mut output, drop_tables);
        println!("  integrated {integrated} drop tables");
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    println!("Writing {} ...", output_path.display());
    fs::write(output_path, serde_json::to_string_pretty(&output)?)?;
    println!("Done.");
    Ok(())
}

fn integrate_drop_tables(output: &mut BTreeMap<String, Value>, drop_tables: Vec<Value>) -> usize {
    let mut integrated = 0usize;

    for drop_table in drop_tables {
        let Some(table_name) = drop_table.get("table_name").and_then(Value::as_str) else {
            continue;
        };
        let path = resolve_drop_table_path(output, table_name);
        let entry = output.entry(path).or_insert_with(|| {
            json!({
                "parent": "/SF/Types/PickUps/DropTables",
                "attributes": {}
            })
        });

        if let Some(obj) = entry.as_object_mut() {
            obj.insert("drop_table".to_string(), drop_table);
            integrated += 1;
        }
    }

    integrated
}

fn resolve_drop_table_path(output: &BTreeMap<String, Value>, table_name: &str) -> String {
    let base = format!("/SF/Types/PickUps/DropTables/{table_name}");
    for candidate in [
        base.clone(),
        format!("{base}DropTable"),
        format!("{base}Droptable"),
    ] {
        if output.contains_key(&candidate) {
            return candidate;
        }
    }
    format!("{base}DropTable")
}

// ── Parser ───────────────────────────────────────────────────────────────────

fn parse_attrs(attrs: &str) -> Value {
    let lines: Vec<&str> = attrs.lines().collect();
    let mut pos = 0usize;
    parse_block(&lines, &mut pos, true)
}

/// Recursively parse a block of lines into a JSON Value.
///
/// `top_level` = true when called for the root attrs string (no surrounding
/// braces to consume); false when called after seeing `{` (will stop at `}`).
fn parse_block(lines: &[&str], pos: &mut usize, top_level: bool) -> Value {
    // Entries: (Option<key>, value)
    // None key = anonymous block element or bare string element.
    let mut entries: Vec<(Option<String>, Value)> = Vec::new();

    while *pos < lines.len() {
        let line = lines[*pos].trim();
        *pos += 1;

        // Closing brace — end of current block.
        if !top_level && (line == "}" || line == "},") {
            break;
        }

        if line.is_empty() || line == "}" || line == "}," {
            // At top level, lone `}` is unexpected noise; skip.
            continue;
        }

        if line == "{" {
            // Anonymous block — array element.
            let nested = parse_block(lines, pos, false);
            entries.push((None, nested));
            continue;
        }

        if let Some(eq_pos) = find_eq_pos(line) {
            let key = line[..eq_pos].trim().to_string();
            let val_str = line[eq_pos + 1..].trim();

            let val = if val_str == "{" {
                // Multi-line block.
                parse_block(lines, pos, false)
            } else if val_str.starts_with('{') {
                // Inline brace value (same line open+close).
                parse_inline_brace(val_str)
            } else {
                parse_scalar(val_str)
            };

            entries.push((Some(key), val));
        } else {
            // Bare value — enum constant / list item.
            let clean = line.trim_end_matches(',').trim();
            if !clean.is_empty() {
                entries.push((None, parse_scalar(clean)));
            }
        }
    }

    build_value(entries)
}

/// Turn collected entries into the right JSON shape.
fn build_value(entries: Vec<(Option<String>, Value)>) -> Value {
    if entries.is_empty() {
        return Value::Object(Map::new());
    }

    let has_keys = entries.iter().any(|(k, _)| k.is_some());
    let all_anon = entries.iter().all(|(k, _)| k.is_none());

    if all_anon {
        // Pure array.
        return Value::Array(entries.into_iter().map(|(_, v)| v).collect());
    }

    // Object (possibly with anonymous mixed in — rare; treat anon as array overflow).
    let mut map: Map<String, Value> = Map::new();
    let mut overflow: Vec<Value> = Vec::new();

    for (key_opt, val) in entries {
        if let Some(key) = key_opt {
            // Duplicate keys → coerce to array.
            if let Some(existing) = map.get_mut(&key) {
                match existing {
                    Value::Array(arr) => arr.push(val),
                    other => {
                        let prev = std::mem::replace(other, Value::Null);
                        *other = Value::Array(vec![prev, val]);
                    }
                }
            } else {
                map.insert(key, val);
            }
        } else if has_keys {
            overflow.push(val);
        }
    }

    if !overflow.is_empty() {
        map.insert("__items".to_string(), Value::Array(overflow));
    }

    Value::Object(map)
}

/// Find the position of the key=value separator `=`, ignoring `=` inside
/// path strings by only looking before the first `{`.
fn find_eq_pos(line: &str) -> Option<usize> {
    let search_end = line.find('{').unwrap_or(line.len());
    line[..search_end].find('=')
}

/// Parse a single-line brace value like `{0.1,0.2,0.3}` or `{A,B,C}`.
fn parse_inline_brace(val: &str) -> Value {
    let val = val.trim();

    // Strip surrounding braces — handle optional trailing comma before `}`.
    let inner = match val
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}').or_else(|| s.strip_suffix("},")))
    {
        Some(s) => s.trim(),
        None => return parse_scalar(val),
    };

    if inner.is_empty() {
        return Value::Object(Map::new()); // {}
    }

    // Check if the inner content contains `=` (nested key=value on one line — rare).
    if inner.contains('=') {
        let lines: Vec<&str> = inner.split('\n').collect();
        let mut pos = 0usize;
        return parse_block(&lines, &mut pos, true);
    }

    // Split by comma; try to parse each part as a number.
    let parts: Vec<&str> = inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Value::Object(Map::new());
    }

    let all_numeric = parts.iter().all(|p| p.parse::<f64>().is_ok());

    if all_numeric {
        Value::Array(
            parts
                .iter()
                .map(|p| {
                    let f: f64 = p.parse().unwrap();
                    if f.fract() == 0.0 && f.abs() < 1e15 {
                        json!(f as i64)
                    } else {
                        json!(f)
                    }
                })
                .collect(),
        )
    } else {
        Value::Array(parts.iter().map(|p| parse_scalar(p)).collect())
    }
}

fn parse_scalar(val: &str) -> Value {
    let val = val.trim().trim_end_matches(',').trim();

    if val.is_empty() || val == "\"\"" {
        return Value::Null;
    }

    // Remove surrounding quotes if present.
    if val.starts_with('"') && val.ends_with('"') && val.len() >= 2 {
        return Value::String(val[1..val.len() - 1].to_string());
    }

    if let Ok(i) = val.parse::<i64>() {
        return json!(i);
    }

    if let Ok(f) = val.parse::<f64>() {
        return json!(f);
    }

    Value::String(val.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrates_drop_table_into_existing_package_entry() {
        let path = "/SF/Types/PickUps/DropTables/CryptDrops/BreakableCryptDropTable";
        let mut output = BTreeMap::from([(
            path.to_string(),
            json!({
                "parent": "/SF/Types/PickUps/DropTables/CryptDrops",
                "attributes": {
                    "InternalName": "BreakableCryptDropTable"
                }
            }),
        )]);

        let count = integrate_drop_tables(
            &mut output,
            vec![json!({
                "table_name": "CryptDrops/BreakableCrypt",
                "parser_mode": "structured",
                "pools": [{
                    "items": [{
                        "path": "/SF/Types/PickUps/Currencies/DracsIncreaseSmall",
                        "name": "DracsIncreaseSmall"
                    }]
                }]
            })],
        );

        assert_eq!(count, 1);
        let entry = &output[path];
        assert_eq!(
            entry["attributes"]["InternalName"],
            "BreakableCryptDropTable"
        );
        assert_eq!(
            entry["drop_table"]["table_name"],
            "CryptDrops/BreakableCrypt"
        );
        assert_eq!(
            entry["drop_table"]["pools"][0]["items"][0]["name"],
            "DracsIncreaseSmall"
        );
    }

    #[test]
    fn missing_package_entry_uses_canonical_drop_table_path() {
        let mut output = BTreeMap::new();
        let count = integrate_drop_tables(
            &mut output,
            vec![json!({
                "table_name": "EnemyDrops/GenericBossSummon",
                "parser_mode": "structured",
                "pools": []
            })],
        );

        let path = "/SF/Types/PickUps/DropTables/EnemyDrops/GenericBossSummonDropTable";
        assert_eq!(count, 1);
        assert!(output.contains_key(path));
        assert_eq!(
            output[path]["drop_table"]["table_name"],
            "EnemyDrops/GenericBossSummon"
        );
    }
}
