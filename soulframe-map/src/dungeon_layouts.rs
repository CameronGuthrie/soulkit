//! Baked procedural component tables and room-local door transforms.
//!
//! A component is accepted only when its two length-prefixed prefab names agree,
//! its bounds, serialized index and component type are valid, and every counted
//! door has an affine orthonormal transform. This also reads inherited configs
//! whose text override is empty: the baked tables are complete in each resource.
use crate::cli::{level_key, CacheStore};
use anyhow::{bail, ensure, Context, Result};
use regex::bytes::Regex;
use serde_json::{json, Value};
use soulframe_cache::Oodle;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

const PREFIX: &str = "SF/Levels/Procs/";
const TYPES: &[&str] = &[
    "spawn",
    "objective",
    "boss",
    "interior",
    "exit",
    "connector",
    "dead_end",
    "special",
    "fitting",
    "exit_gate",
    "boss_gate",
    "objective_gate",
    "special_gate",
    "cap",
    "backdrop",
    "cinematic",
];

pub(crate) fn extract_layouts(cache_path: &Path, oodle: &Oodle, output: &Path) -> Result<()> {
    let cache = CacheStore::load_named(cache_path, &["H.AnimRetarget"])?;
    let mut paths = BTreeSet::new();
    for entry in cache.entries() {
        if entry.entry.is_dir {
            continue;
        }
        if let Some(rest) = entry.entry.full_path.strip_prefix(PREFIX) {
            let parts: Vec<_> = rest.split('/').collect();
            if (1..=2).contains(&parts.len()) && !parts.last().unwrap().contains('.') {
                paths.insert(entry.entry.full_path.clone());
            }
        }
    }
    let mut sets = BTreeMap::<String, Vec<Value>>::new();
    let mut skipped = Vec::new();
    for path in paths {
        let mut entries = cache.matching_entries(&path);
        entries.sort_by_key(|e| (Reverse(e.entry.timestamp), Reverse(e.entry.index)));
        let entry = &entries[0];
        let bytes = cache.read_entry(oodle, entry.cache_index, entry.entry)?;
        match parse_layout(&path, &bytes) {
            Ok(Some(layout)) => {
                let set = layout["set"].as_str().unwrap();
                let key = level_key(set);
                if output.join(&key).join("rooms.json").is_file() {
                    sets.entry(key).or_default().push(layout);
                }
            }
            Ok(None) => {}
            Err(error) => {
                skipped.push(json!({"path":path,"error":error.to_string()}));
            }
        }
    }
    let mut index_sets = Vec::new();
    let mut count = 0;
    // Preserve floor-plan sets even when no generator config references them.
    for entry in fs::read_dir(output)? {
        let entry = entry?;
        if entry.path().join("rooms.json").is_file() {
            sets.entry(entry.file_name().to_string_lossy().into_owned())
                .or_default();
        }
    }
    for (key, layouts) in sets {
        let set_dir = output.join(&key);
        fs::create_dir_all(&set_dir)?;
        count += layouts.len();
        let level_paths: Vec<_> = layouts.iter().map(|v| v["level_path"].clone()).collect();
        let set = if let Some(layout) = layouts.first() {
            layout["set"].clone()
        } else {
            let rooms: Value = serde_json::from_slice(&fs::read(set_dir.join("rooms.json"))?)?;
            rooms["rooms"][0]["set"].clone()
        };
        write_json(
            &set_dir.join("layouts.json"),
            &json!({"schema":1,"layouts":layouts}),
        )?;
        index_sets.push(json!({"key":key,"set":set,"rooms":format!("dungeons/{key}/rooms.json"),"layouts":format!("dungeons/{key}/layouts.json"),"level_paths":level_paths}));
    }
    write_json(
        &output.join("dungeons.json"),
        &json!({"schema":1,"coordinate_space":"layout-local","sets":index_sets,"skipped":skipped}),
    )?;
    println!("Dungeon layouts complete: {count} procedural level(s)");
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_string(value)? + "\n")
        .with_context(|| format!("write {}", path.display()))
}

fn parse_layout(path: &str, bytes: &[u8]) -> Result<Option<Value>> {
    let mut reader = Reader { bytes, pos: 16 };
    let dependency_count = reader.u32()? as usize;
    ensure!(dependency_count < 100_000, "too many dependencies");
    let mut dependencies = Vec::new();
    for _ in 0..dependency_count {
        dependencies.push(reader.string()?.to_string());
    }
    let text_size = reader.u32()? as usize;
    reader.skip(text_size)?;
    let baked_start = reader.pos;
    let prefab = Regex::new(r"/?[A-Za-z0-9_/]+\.level")?;
    let mut records = Vec::new();
    for matched in prefab.find_iter(&bytes[baked_start..]) {
        let start = baked_start + matched.start();
        let name = std::str::from_utf8(matched.as_bytes())?;
        if start < 4
            || u32::from_le_bytes(bytes[start - 4..start].try_into().unwrap()) as usize
                != name.len()
        {
            continue;
        }
        // Other prefab references do not have this complete component structure.
        if let Ok(record) = read_component(bytes, start - 4, name) {
            records.push(record);
        }
    }
    if records.is_empty() {
        return Ok(None);
    }
    let mut tables = BTreeMap::<u8, Vec<Value>>::new();
    for (index, typ, value) in records {
        let table = tables
            .entry(if (1..=7).contains(&typ) { 1 } else { typ })
            .or_default();
        ensure!(
            table.iter().all(|v| v["index"] != index),
            "duplicate serialized index {index} in component type {typ}"
        );
        table.push(value);
    }
    for table in tables.values_mut() {
        table.sort_by_key(|v| v["index"].as_u64().unwrap());
        for (expected, record) in table.iter().enumerate() {
            ensure!(
                record["index"] == expected,
                "non-contiguous serialized component indices"
            );
        }
    }
    let spawns = tables.remove(&0).context("no spawn table")?;
    let components = tables.remove(&1).context("no main component table")?;
    let caps = tables.remove(&13).context("no cap table")?;
    let extra: BTreeMap<_, _> = tables
        .into_iter()
        .map(|(typ, table)| (TYPES[typ as usize], table))
        .collect();
    let names: BTreeSet<_> = spawns
        .iter()
        .chain(&components)
        .map(|r| r["room"].as_str().unwrap())
        .collect();
    let mut candidate_sets = BTreeMap::<String, usize>::new();
    for dependency in dependencies {
        if let Some(rest) = dependency.trim_start_matches('/').strip_prefix(PREFIX) {
            let parts: Vec<_> = rest.split('/').collect();
            if parts.len() == 2
                && parts[1]
                    .strip_suffix(".level")
                    .is_some_and(|room| names.contains(room))
            {
                *candidate_sets.entry(parts[0].to_string()).or_default() += 1;
            }
        }
    }
    let set = candidate_sets
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .context("no dungeon tileset in prefab dependencies")?
        .0;
    Ok(Some(json!({
        "schema":1,"level_path":format!("/{path}"),"set":set,
        "coordinate_space":"layout-local","index_bits":ceil_log2(components.len()),
        "spawns":spawns,"components":components,"caps":caps,"extra_components":extra,
        "tree_only":path.ends_with("/FallenFortressProcLevel"),
        "portal_transform":"row-major affine matrix; translation at 12..14; outward +Z normal at 8..10",
    })))
}

fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

fn read_component(bytes: &[u8], start: usize, expected: &str) -> Result<(u32, u8, Value)> {
    let mut reader = Reader { bytes, pos: start };
    ensure!(reader.string()? == expected, "prefab mismatch");
    let mut bounds = [0.0f32; 8];
    for value in &mut bounds {
        *value = reader.f32()?;
    }
    ensure!(
        bounds.iter().all(|v| v.is_finite() && v.abs() < 100_000.0),
        "invalid component bounds"
    );
    ensure!(
        (0..3).all(|i| bounds[i] <= bounds[i + 4]),
        "reversed component bounds"
    );
    let index = reader.u32()?;
    let typ = reader.byte()?;
    ensure!(
        index < 4096 && (typ as usize) < TYPES.len(),
        "invalid component index/type"
    );
    let flags = reader.u32()?;
    let baked_prefab = reader.string()?.to_string();
    let room = expected
        .strip_suffix(".level")
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap();
    ensure!(
        baked_prefab
            .strip_suffix(".level")
            .and_then(|s| s.rsplit('/').next())
            == Some(room),
        "baked prefab mismatch"
    );
    let baked_stem = baked_prefab.strip_suffix(".level").unwrap();
    let bsp = reader.string()?;
    ensure!(
        bsp.is_empty() || (bsp.starts_with(&format!("{baked_stem}/")) && bsp.ends_with(".bsp")),
        "invalid BSP reference"
    );
    let count = reader.u32()? as usize;
    ensure!(count <= 64, "too many component portals");
    let mut portals = Vec::new();
    for _ in 0..count {
        let mut type_path = reader.string()?.to_string();
        let name = reader.string()?.to_string();
        if type_path.starts_with("LevelPortals/") {
            type_path = format!("/{PREFIX}{type_path}");
        }
        ensure!(
            type_path.starts_with('/') && !name.is_empty(),
            "invalid portal resource/name"
        );
        let flags = reader.u32()?;
        let mut matrix = [0.0f32; 16];
        for value in &mut matrix {
            *value = reader.f32()?;
        }
        ensure!(
            matrix.iter().all(|v| v.is_finite() && v.abs() < 100_000.0),
            "invalid portal transform"
        );
        ensure!(
            [3, 7, 11].iter().all(|&i| matrix[i].abs() < 0.001) && (matrix[15] - 1.0).abs() < 0.001,
            "non-affine portal transform"
        );
        for row in 0..3 {
            let length2: f32 = (0..3).map(|col| matrix[row * 4 + col].powi(2)).sum();
            ensure!((length2 - 1.0).abs() < 0.02, "non-unit portal basis");
            for other in 0..row {
                let dot: f32 = (0..3)
                    .map(|col| matrix[row * 4 + col] * matrix[other * 4 + col])
                    .sum();
                ensure!(dot.abs() < 0.02, "non-orthogonal portal basis");
            }
        }
        portals.push(json!({"name":name,"type_path":type_path,"flags":flags,
            "position":[matrix[12],matrix[13],matrix[14]],"normal":[matrix[8],matrix[9],matrix[10]],"transform":matrix}));
    }
    Ok((
        index,
        typ,
        json!({"index":index,"type":typ,"kind":TYPES[typ as usize],"room":room,"flags":flags,"portals":portals}),
    ))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.pos.checked_add(N).context("reader overflow")?;
        let value = self
            .bytes
            .get(self.pos..end)
            .context("truncated procedural component")?;
        self.pos = end;
        Ok(value.try_into().unwrap())
    }
    fn skip(&mut self, size: usize) -> Result<()> {
        self.pos = self.pos.checked_add(size).context("reader overflow")?;
        ensure!(self.pos <= self.bytes.len(), "truncated procedural header");
        Ok(())
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn string(&mut self) -> Result<&str> {
        let size = self.u32()? as usize;
        if size > 65_536 {
            bail!("implausible procedural string length");
        }
        let start = self.pos;
        self.skip(size)?;
        Ok(std::str::from_utf8(&self.bytes[start..self.pos])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn string(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend((value.len() as u32).to_le_bytes());
        bytes.extend(value.bytes());
    }
    fn component(matrix: [f32; 16]) -> Vec<u8> {
        let mut bytes = Vec::new();
        string(&mut bytes, "Room.level");
        for value in [-10.0f32, -2.0, -10.0, 1.0, 10.0, 4.0, 10.0, 1.0] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes());
        bytes.push(0);
        bytes.extend(0u32.to_le_bytes());
        string(&mut bytes, "Room.level");
        string(&mut bytes, "Room/0.bsp");
        bytes.extend(1u32.to_le_bytes());
        string(&mut bytes, "/SF/Levels/Procs/LevelPortals/Test");
        string(&mut bytes, "Door0");
        bytes.extend(0u32.to_le_bytes());
        for value in matrix {
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn portal_transform_preserves_position_and_outward_normal() {
        let bytes = component([
            -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 2.0, 3.5, -2.0, 1.0,
        ]);
        let (_, _, record) = read_component(&bytes, 0, "Room.level").unwrap();
        assert_eq!(record["portals"][0]["position"], json!([2.0, 3.5, -2.0]));
        assert_eq!(record["portals"][0]["normal"], json!([0.0, 0.0, -1.0]));
        assert!(read_component(&bytes[..bytes.len() - 1], 0, "Room.level").is_err());
    }
    #[test]
    fn rejects_invalid_affine_and_rotation_data() {
        for matrix in [[0.0; 16], [f32::NAN; 16]] {
            assert!(read_component(&component(matrix), 0, "Room.level").is_err());
        }
    }

    #[test]
    fn empty_text_override_still_exports_baked_tables() {
        let mut bytes = vec![0; 16];
        bytes.extend(1u32.to_le_bytes());
        string(&mut bytes, "/SF/Levels/Procs/Test/Room.level");
        bytes.extend(0u32.to_le_bytes());
        let matrix = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let type_offset = 4 + "Room.level".len() + 32 + 4;
        for typ in [0, 5, 13] {
            let mut record = component(matrix);
            record[type_offset] = typ;
            bytes.extend(record);
        }
        let layout = parse_layout("SF/Levels/Procs/Test/TestProcLevel", &bytes).unwrap().unwrap();
        assert_eq!(layout["set"], "Test");
        assert_eq!(layout["spawns"][0]["room"], "Room");
        assert_eq!(layout["components"][0]["type"], 5);
        assert_eq!(layout["caps"][0]["type"], 13);
        assert_eq!(layout["tree_only"], false);
        // A non-contiguous main table is rejected, not silently reindexed.
        let first = 16 + 4 + 4 + "/SF/Levels/Procs/Test/Room.level".len() + 4;
        let main_index = first + component(matrix).len() + type_offset - 4;
        bytes[main_index..main_index+4].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse_layout("SF/Levels/Procs/Test/TestProcLevel", &bytes).is_err());
    }
}
