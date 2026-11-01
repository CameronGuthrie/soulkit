use crate::dungeon_tiles::extract_dungeon_tiles;
use crate::tiles::tile_dds;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::Subcommand;
use serde_json::{json, Value};
use soulframe_cache::{
    detect_soulframe_cache, shcc_decompress_chunk_oodle, toc_decode, Oodle, TocEntry,
};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MIDRATH_LEVEL: &str = "SF/Levels/MidrathRedux.level";
const GODS_AND_GHOST: &str =
    "SF/Types/Game/Encounters/Ambient/Dynamic/DynamicGodsAndGhostEncounter";
const TILE_PREFIX: &str = "SF/Levels/";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Extract every tiled level map plus Midrath JSON into an output/maps-style directory.
    Extract {
        /// Override Soulframe cache directory.
        #[arg(long)]
        cache_path: Option<PathBuf>,
        /// Output directory; tiles go to <levelKey>/ (e.g. midrathRedux/) and JSON goes to map-json/.
        #[arg(short, long, default_value = "output/maps")]
        output: PathBuf,
        /// Formatted package definitions for map names (defaults beside map output).
        #[arg(long)]
        packages: Option<PathBuf>,
        /// Skip WebP tile extraction.
        #[arg(long, default_value_t = false)]
        skip_tiles: bool,
        /// Skip map JSON extraction.
        #[arg(long, default_value_t = false)]
        skip_json: bool,
        /// List discovered map tile paths and exit.
        #[arg(long, default_value_t = false)]
        list_tiles: bool,
        /// Skip procedural dungeon floor plans (one `<setKey>/` map per tileset, from navmeshes).
        #[arg(long, default_value_t = false)]
        skip_dungeons: bool,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Extract {
            cache_path,
            output,
            packages,
            skip_tiles,
            skip_json,
            list_tiles,
            skip_dungeons,
        } => run_extract(ExtractArgs {
            cache_path,
            output,
            packages,
            skip_tiles,
            skip_json,
            list_tiles,
            skip_dungeons,
        }),
    }
}

#[derive(Debug)]
pub struct ExtractArgs {
    pub cache_path: Option<PathBuf>,
    pub output: PathBuf,
    pub packages: Option<PathBuf>,
    pub skip_tiles: bool,
    pub skip_json: bool,
    pub list_tiles: bool,
    pub skip_dungeons: bool,
}

pub fn run_extract(args: ExtractArgs) -> Result<()> {
    println!("{}", "=".repeat(70));
    println!("Soulframe Map Extractor");
    println!("{}", "=".repeat(70));
    println!();

    let cache_path = resolve_cache_path(args.cache_path)?;
    println!("Found Soulframe cache at:\n  {:?}\n", cache_path);

    let oodle =
        Oodle::new().map_err(|e| anyhow!("Failed to initialize Oodle decompression: {e}"))?;

    let mut summary = ExtractionSummary::default();

    if !args.skip_tiles || args.list_tiles {
        let count = extract_tiles(&cache_path, &oodle, &args.output, args.list_tiles)?;
        summary.tile_count = count;
        if args.list_tiles {
            return Ok(());
        }
    }

    if !args.skip_dungeons {
        summary.dungeon_tile_count = extract_dungeon_tiles(&cache_path, &oodle, &args.output)?;
        crate::dungeon_layouts::extract_layouts(&cache_path, &oodle, &args.output)?;
    }

    if !args.skip_json {
        let json_dir = args.output.join("map-json");
        let packages = args.packages.unwrap_or_else(|| args.output.parent()
            .unwrap_or(Path::new(".")).join("formatted_packages.json"));
        summary.json_counts = extract_json(&cache_path, &oodle, &json_dir, summary.tile_count, &packages)?;
    }

    println!();
    println!("{}", "=".repeat(70));
    println!("Map extraction complete:");
    if !args.skip_tiles {
        println!("  {} WebP tile(s)", summary.tile_count);
    }
    if !args.skip_dungeons {
        println!("  {} dungeon room floor plan(s)", summary.dungeon_tile_count);
    }
    if !args.skip_json {
        println!("  {} JSON file(s)", summary.json_counts.len());
    }
    println!("Output directory: {:?}", args.output);
    println!("{}", "=".repeat(70));
    Ok(())
}

#[derive(Default)]
struct ExtractionSummary {
    tile_count: usize,
    dungeon_tile_count: usize,
    json_counts: BTreeMap<String, usize>,
}

fn resolve_cache_path(cache_path: Option<PathBuf>) -> Result<PathBuf> {
    match cache_path {
        Some(p) => {
            if !p.exists() || !p.is_dir() {
                return Err(anyhow!(
                    "Custom cache path does not exist or is not a directory: {:?}",
                    p
                ));
            }
            Ok(p)
        }
        None => detect_soulframe_cache().ok_or_else(|| {
            anyhow!("Soulframe cache not found at default location. Pass --cache-path to override.")
        }),
    }
}

fn extract_tiles(
    cache_path: &Path,
    oodle: &Oodle,
    output: &Path,
    list_only: bool,
) -> Result<usize> {
    println!("Scanning level map tiles...");

    let caches = CacheStore::load_named(cache_path, &["H.LightMap", "F.LightMap", "B.LightMap"])?;
    let tiles = discover_tiles(&caches);
    println!("Found {} candidate tile path(s)", tiles.len());

    if list_only {
        for tile in &tiles {
            println!("{}", tile.path);
        }
        return Ok(tiles.len());
    }

    let mut written = 0usize;
    let mut skipped = 0usize;
    for tile in &tiles {
        let dds = match read_tile_dds(&caches, oodle, &tile.path) {
            Ok(dds) => dds,
            Err(e) => {
                skipped += 1;
                eprintln!("  [tile] skipped {}: {e:#}", tile.path);
                continue;
            }
        };

        let tile_dir = output.join(&tile.level);
        fs::create_dir_all(&tile_dir)
            .with_context(|| format!("Failed to create tile output dir {}", tile_dir.display()))?;
        let out_path = tile_dir.join(format!("{}.webp", tile.index));
        let dds = ddsfile::Dds::read(&mut Cursor::new(&dds))
            .map_err(|e| anyhow!("Failed to parse DDS for {}: {e}", tile.path))?;
        let img = image_dds::image_from_dds(&dds, 0)
            .map_err(|e| anyhow!("Failed to decode DDS for {}: {e}", tile.path))?;
        let mut out_file = File::create(&out_path)
            .with_context(|| format!("Failed to create {}", out_path.display()))?;
        image::codecs::webp::WebPEncoder::new_lossless(&mut out_file)
            .encode(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| anyhow!("Failed to save {}: {e}", out_path.display()))?;

        written += 1;
        if written % 25 == 0 {
            println!("  Wrote {written} tile(s)...");
        }
    }

    println!("Tiles complete: {written} written, {skipped} skipped");
    Ok(written)
}

fn read_tile_dds(caches: &CacheStore, oodle: &Oodle, path: &str) -> Result<Vec<u8>> {
    let mut entries = caches.matching_entries(path);
    // Zero-timestamp records can be obsolete variants with a different format.
    entries.sort_by_key(|e| (Reverse(e.entry.timestamp), Reverse(e.entry.index)));
    let headers: Vec<_> = entries
        .iter()
        .filter(|e| e.cache_name.starts_with('H'))
        .map(|e| caches.read_entry(oodle, e.cache_index, e.entry))
        .collect::<Result<_>>()?;
    let mut last_error = anyhow!("no compatible header/body pair for {path}");
    for role in ['F', 'B'] {
        for entry in entries.iter().filter(|e| e.cache_name.starts_with(role)) {
            let body = match caches.read_entry(oodle, entry.cache_index, entry.entry) {
                Ok(body) => body,
                Err(error) => {
                    last_error = error;
                    continue;
                }
            };
            for header in &headers {
                match tile_dds(header, &body, role == 'F') {
                    Ok(dds) => return Ok(dds),
                    Err(error) => last_error = error,
                }
            }
        }
    }
    Err(last_error)
}

#[derive(Debug, Clone, PartialEq)]
struct TileRef {
    /// Output folder for the level, e.g. `midrathRedux`.
    level: String,
    index: u32,
    path: String,
}

fn discover_tiles(caches: &CacheStore) -> Vec<TileRef> {
    discover_tile_paths(
        caches
            .entries()
            .into_iter()
            .filter(|e| !e.entry.is_dir)
            .map(|e| e.entry.full_path.as_str()),
    )
}

/// Every level with a tiled `<n>_cmm.png` map becomes its own map folder. Levels with only
/// an `_mm` capture are skipped: those are 8-128 px minimap masks, not maps.
fn discover_tile_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<TileRef> {
    let mut by_index = BTreeMap::new();
    for path in paths {
        if let Some(key) = tile_index(path) {
            by_index.entry(key).or_insert_with(|| path.to_owned());
        }
    }
    by_index
        .into_iter()
        .map(|((level, index), path)| TileRef { level, index, path })
        .collect()
}

/// `SF/Levels/MidrathRedux/12_cmm.png` -> (`midrathRedux`, 12).
fn tile_index(path: &str) -> Option<(String, u32)> {
    let (dir, file) = path.strip_prefix(TILE_PREFIX)?.rsplit_once('/')?;
    let stem = file.strip_suffix("_cmm.png")?;
    if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((level_key(dir), stem.parse().ok()?))
}

/// CamelCase level directory (nested dirs joined) with a lowercase first letter.
pub(crate) fn level_key(dir: &str) -> String {
    let joined: String = dir.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    let mut chars = joined.chars();
    chars.next().map(|c| c.to_ascii_lowercase().to_string() + chars.as_str()).unwrap_or_default()
}

fn current_source(
    caches: &CacheStore,
    oodle: &Oodle,
    path: &str,
    role: char,
) -> Result<(String, Vec<u8>)> {
    let entry = caches
        .matching_entries(path)
        .into_iter()
        .filter(|e| e.cache_name.starts_with(role))
        .max_by_key(|e| (e.entry.timestamp, e.entry.index))
        .with_context(|| format!("missing {role} record for {path}"))?;
    let label = format!("{}:{}:{path}", entry.cache_name, entry.entry.index);
    let data = caches.read_entry(oodle, entry.cache_index, entry.entry)?;
    Ok((label, data))
}

fn extract_json(
    cache_path: &Path,
    oodle: &Oodle,
    json_dir: &Path,
    tiles_written: usize,
    packages_path: &Path,
) -> Result<BTreeMap<String, usize>> {
    println!("Parsing Midrath level records...");
    let caches = CacheStore::load_named(
        cache_path,
        &["H.AnimRetarget", "B.AnimRetarget", "H.LightMap"],
    )?;
    let (header_source, header) = current_source(&caches, oodle, MIDRATH_LEVEL, 'H')?;
    let (body_source, body) = current_source(&caches, oodle, MIDRATH_LEVEL, 'B')?;
    let entities = crate::level::parse_level(&header, &body)
        .context("Midrath H/B records could not be paired; refusing to guess map positions")?;
    let (event_source, event_header) = current_source(&caches, oodle, GODS_AND_GHOST, 'H')?;
    let event = crate::level::resource_metadata(&event_header)?;
    // These virtual resource definitions live in Packages.bin, not separate
    // TOC records. Reuse the formatted output produced by the items stage.
    let packages: Value = match fs::read_to_string(packages_path) {
        Ok(text) => serde_json::from_str(&text).context("invalid formatted packages for map names")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("  [name] {} is missing; lore labels will remain unresolved", packages_path.display());
            json!({})
        }
        Err(error) => return Err(error.into()),
    };
    let mut location_names = BTreeMap::new();
    let mut location_sources = BTreeMap::new();
    for entity in entities.iter().filter(|e| e.type_path == "/SF/Types/Gameplay/DiscoveryTrigger") {
        let Some(resource) = crate::properties::nested_field(&entity.text, "_loreFragmentRes")
            .filter(|s| s.starts_with("/SF/Types/Quests/LocationLoreQuests/")) else { continue; };
        if location_sources.contains_key(resource) { continue; }
        match crate::map_json::lore_name(&packages, resource) {
            Some(name) => {
                location_names.insert(resource.to_owned(), name.to_owned());
                location_sources.insert(resource.to_owned(), json!({"loc_tag":name}));
            }
            None => {
                eprintln!("  [name] {resource}: no location Name in package definitions");
                location_sources.insert(resource.to_owned(), json!({"error":"unresolved_lore_name"}));
            }
        }
    }
    let mut documents = crate::map_json::documents(&entities, &header_source, &event, &location_names)?;
    documents.get_mut("coverage.json").unwrap()["location_name_sources"] = json!(location_sources);
    documents.get_mut("coverage.json").unwrap()["location_name_packages"] = json!(packages_path.to_string_lossy());
    let tiles = documents["tile_layout.json"]["tiles"]
        .as_array()
        .context("missing tile layout")?;
    let tile_count = tiles.len();
    let extent = |field: &str, minimum: bool| -> f64 {
        tiles
            .iter()
            .filter_map(|v| v["world_bounds"][field].as_f64())
            .reduce(|a, b| if minimum { a.min(b) } else { a.max(b) })
            .unwrap()
    };
    let (min_x, max_x, min_z, max_z) = (
        extent("min_x", true),
        extent("max_x", false),
        extent("min_z", true),
        extent("max_z", false),
    );
    let texture = tiles[0]["texture"]
        .as_str()
        .context("missing tile texture")?
        .trim_start_matches('/');
    let (_, texture_header) = current_source(&caches, oodle, texture, 'H')?;
    let texture_info = soulframe_images::texture_unpack_header(&texture_header)?;
    let tile_width = tiles[0]["world_bounds"]["max_x"].as_f64().unwrap()
        - tiles[0]["world_bounds"]["min_x"].as_f64().unwrap();
    let scale = f64::from(texture_info.width) / tile_width;
    let bounds = json!({"min_x":min_x,"max_x":max_x,"min_z":min_z,"max_z":max_z,
        "map_size_pixels":(max_x-min_x)*scale,"tile_size_pixels":texture_info.width,
        "tile_grid":format!("{}x{}", (max_x-min_x)/tile_width,(max_z-min_z)/tile_width),
        "pixels_per_world_unit":scale});
    documents.get_mut("tile_layout.json").unwrap()["world_bounds"] = bounds.clone();
    let counts: BTreeMap<String, usize> = documents
        .iter()
        .map(|(name, v)| (name.clone(), document_count(v)))
        .collect();
    documents.insert("metadata.json".into(), json!({
        "schema_version":2,"generated_at":Utc::now().to_rfc3339(),
        "source":"MidrathRedux.level structured H/B records",
        "source_files":[header_source,body_source,event_source],
        "level_header_hash":header[..16].iter().map(|b|format!("{b:02x}")).collect::<String>(),
        "extraction_method":"Entity records (level format tag 212) paired by ordinal, length and instance ID; explicit POI types and transformed sap attachments",
        "entity_count":entities.len(),"tile_count":tile_count,"tiles_written_this_run":tiles_written,
        "counts":counts.iter().map(|(name,n)|(name.trim_end_matches(".json"),n)).collect::<BTreeMap<_,_>>(),
        "coordinate_system":"Soulframe world coordinates: x/east-west, y/height, z/north-south",
        "world_bounds":bounds,"world_bounds_approx":bounds,
        "coverage_file":"coverage.json","tile_layout_file":"tile_layout.json",
        "runtime_state_evaluated":false
    }));
    // Validate every document before replacing any existing JSON output.
    let serialized: Vec<_> = documents
        .iter()
        .map(|(name, value)| Ok((name, serde_json::to_string_pretty(value)?)))
        .collect::<Result<_>>()?;
    fs::create_dir_all(json_dir)?;
    for (name, text) in serialized {
        fs::write(json_dir.join(name), format!("{text}\n"))?;
    }
    archive_disabled_outputs(json_dir)?;
    println!(
        "  Validated {} entity records and {} tile placements",
        entities.len(),
        tile_count
    );
    for (name, count) in &counts {
        println!("  {name}: {count}");
    }
    Ok(documents
        .iter()
        .map(|(name, v)| (name.clone(), document_count(v)))
        .collect())
}

fn archive_disabled_outputs(json_dir: &Path) -> Result<()> {
    // Preserve old generated data, but keep it out of the active map-json root.
    for name in ["campfires", "envoy_camps", "revive_shrines"] {
        let source = json_dir.join(format!("{name}.json"));
        if !source.is_file() {
            continue;
        }
        let archive = json_dir.join("disabled");
        fs::create_dir_all(&archive)?;
        let mut destination = archive.join(format!("{name}.json"));
        let mut revision = 1;
        while destination.exists() {
            destination = archive.join(format!("{name}.{revision}.json"));
            revision += 1;
        }
        fs::rename(&source, &destination)
            .with_context(|| format!("Failed to archive disabled output {}", source.display()))?;
    }
    Ok(())
}

fn document_count(value: &Value) -> usize {
    if let Some(count) = value.get("count").and_then(Value::as_u64) {
        return count as usize;
    }
    if let Some(tiles) = value.get("tiles").and_then(Value::as_array) {
        return tiles.len();
    }
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .map(|v| {
                v.get("markers")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(1)
            })
            .sum();
    }
    value
        .as_object()
        .map(|o| {
            o.values()
                .map(|v| v.as_array().map(Vec::len).unwrap_or(1))
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tile_index_only_accepts_decimal_cmm_tiles() {
        assert_eq!(tile_index("SF/Levels/MidrathRedux/195_cmm.png"), Some(("midrathRedux".into(), 195)));
        assert_eq!(tile_index("SF/Levels/Regions/NewLand/3_cmm.png"), Some(("regionsNewLand".into(), 3)));
        assert_eq!(tile_index("SF/Levels/MidrathRedux/10A_cmm.png"), None);
        assert_eq!(tile_index("SF/Levels/MidrathRedux/1_mm.png"), None);
        assert_eq!(tile_index("SF/Levels/Tent/LevelCapture_mm.png"), None);
        assert_eq!(tile_index("SF/Levels/_cmm.png"), None);
    }

    #[test]
    fn every_level_with_cmm_tiles_becomes_its_own_map() {
        let tiles = discover_tile_paths(
            [
                "SF/Levels/MidrathRedux/1_cmm.png",
                "SF/Levels/MidrathRedux/0_cmm.png",
                "SF/Levels/MidrathRedux/0_cmm.png", // duplicate record in another cache
                "SF/Levels/MidrathRedux/0_mm.png",
                "SF/Levels/Hubs/FeyHub/LevelCapture_mm.png",
                "SF/Levels/NewRegion/0_cmm.png",
            ]
            .into_iter(),
        );
        let got: Vec<_> = tiles.iter().map(|t| (t.level.as_str(), t.index)).collect();
        assert_eq!(got, [("midrathRedux", 0), ("midrathRedux", 1), ("newRegion", 0)]);
        assert_eq!(tiles[2].path, "SF/Levels/NewRegion/0_cmm.png");
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CacheStore {
    root: PathBuf,
    sets: Vec<CacheSet>,
}

#[derive(Debug, Clone)]
struct CacheSet {
    name: String,
    entries: Vec<TocEntry>,
}

#[derive(Debug)]
pub(crate) struct StoreEntry<'a> {
    pub(crate) cache_index: usize,
    pub(crate) cache_name: &'a str,
    pub(crate) entry: &'a TocEntry,
}

impl CacheStore {
    pub(crate) fn load_named(root: &Path, names: &[&str]) -> Result<Self> {
        let mut sets = Vec::new();
        for name in names {
            let toc_path = root.join(format!("{name}.toc"));
            if !toc_path.exists() {
                continue;
            }
            let toc = fs::read(&toc_path)
                .with_context(|| format!("Failed to read {}", toc_path.display()))?;
            let entries = toc_decode(&toc)
                .with_context(|| format!("Failed to decode {}", toc_path.display()))?;
            sets.push(CacheSet {
                name: (*name).to_string(),
                entries,
            });
        }
        Ok(Self {
            root: root.to_path_buf(),
            sets,
        })
    }

    pub(crate) fn entries(&self) -> Vec<StoreEntry<'_>> {
        let mut out = Vec::new();
        for (cache_index, set) in self.sets.iter().enumerate() {
            for entry in &set.entries {
                out.push(StoreEntry {
                    cache_index,
                    cache_name: &set.name,
                    entry,
                });
            }
        }
        out
    }

    pub(crate) fn matching_entries<'a>(&'a self, path: &str) -> Vec<StoreEntry<'a>> {
        let mut out = Vec::new();
        for (cache_index, set) in self.sets.iter().enumerate() {
            for entry in &set.entries {
                if !entry.is_dir && entry.full_path == path {
                    out.push(StoreEntry {
                        cache_index,
                        cache_name: &set.name,
                        entry,
                    });
                }
            }
        }
        out
    }

    pub(crate) fn read_entry(&self, oodle: &Oodle, cache_index: usize, entry: &TocEntry) -> Result<Vec<u8>> {
        let set = self
            .sets
            .get(cache_index)
            .ok_or_else(|| anyhow!("cache index out of range"))?;
        let cache_path = self.root.join(format!("{}.cache", set.name));
        let mut file = File::open(&cache_path)
            .with_context(|| format!("Failed to open {}", cache_path.display()))?;
        file.seek(SeekFrom::Start(entry.cache_offset))
            .with_context(|| format!("Failed to seek {}", cache_path.display()))?;

        let read_size = if entry.decompressed_size == entry.compressed_size {
            entry.decompressed_size
        } else {
            entry.compressed_size
        } as usize;
        let mut chunk = vec![0u8; read_size];
        file.read_exact(&mut chunk)
            .with_context(|| format!("Failed to read {}", cache_path.display()))?;

        if entry.decompressed_size == entry.compressed_size {
            return Ok(chunk);
        }

        if chunk.is_empty() {
            return Ok(Vec::new());
        }

        let mut decompressed = if chunk[0] == 0x80 {
            shcc_decompress_chunk_oodle(oodle, &chunk, 0, entry.decompressed_size as usize)?.0
        } else {
            oodle.decompress(&chunk, entry.decompressed_size as usize)?
        };

        if decompressed.len() < entry.decompressed_size as usize {
            decompressed.resize(entry.decompressed_size as usize, 0);
        } else if decompressed.len() > entry.decompressed_size as usize {
            decompressed.truncate(entry.decompressed_size as usize);
        }
        Ok(decompressed)
    }
}
