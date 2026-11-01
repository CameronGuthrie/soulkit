//! Reconstruct playable audio from the installed Soulframe cache.
//!
//! Every cache family and installed locale is scanned. H contains the sound
//! header; B and F contain sound data or (for localized speech) auxiliary data.
//! Format reconstruction decides which pieces form the actual audio stream.

pub mod cli;
pub mod format;
pub mod vorbis;

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use soulframe_cache::{detect_soulframe_cache, toc_decode, LazyCache, Oodle, TocEntry};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// Bump when reconstruction changes so previously exported containers refresh.
const INDEX_VERSION: u32 = 2;

#[derive(Debug)]
pub struct ExtractArgs {
    pub cache_path: Option<PathBuf>,
    pub game_exe: Option<PathBuf>,
    pub output: PathBuf,
    pub path_filter: Option<String>,
    pub limit: usize,
    pub list_paths: bool,
    pub force: bool,
}

/// Locale/cache family is part of an audio asset's identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AudioAssetKey {
    pub family: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct AudioAssetInfo {
    pub key: AudioAssetKey,
    pub category: String,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
}

fn asset_key(family: &str, path: &str) -> (String, String) {
    (
        family.replace('\\', "/").to_ascii_lowercase(),
        path.replace('\\', "/").to_ascii_lowercase(),
    )
}

impl Default for ExtractArgs {
    fn default() -> Self {
        Self {
            cache_path: None,
            game_exe: None,
            output: PathBuf::from("output/audio"),
            path_filter: None,
            limit: 0,
            list_paths: false,
            force: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Part {
    Header,
    Body,
    Stream,
}

struct CacheSet {
    name: String,
    cache: LazyCache,
}

#[derive(Clone, Copy, Debug)]
struct Location {
    cache: usize,
    entry: usize,
    rank: (u64, usize),
}

#[derive(Debug)]
struct Asset {
    family: String,
    path: String,
    parts: BTreeMap<Part, Location>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Source {
    pub cache: String,
    pub toc_index: usize,
    pub timestamp: u64,
    pub offset: u64,
    pub compressed_bytes: u32,
    pub decompressed_bytes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Extracted,
    Native,
    Unchanged,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeFile {
    pub part: String,
    pub path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioRecord {
    pub cache_family: String,
    pub source_path: String,
    pub category: String,
    pub sources: Vec<Source>,
    pub status: Status,
    pub playable: bool,
    pub native_files: Vec<NativeFile>,
    pub output_path: Option<String>,
    pub output_bytes: Option<u64>,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub duration_seconds: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AudioIndex {
    pub version: u32,
    pub selected: usize,
    pub matched: usize,
    pub extracted: usize,
    pub unchanged: usize,
    pub native: usize,
    pub failed: usize,
    pub files: Vec<AudioRecord>,
}

/// Audio containers seen in Evolution caches, plus standard audio containers.
/// Selection uses the extension, not a sound-directory filter, so music,
/// speech, weapons, UI sounds and impulse responses are all included.
pub fn is_audio_file(path: &str) -> bool {
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "wav"
            | "wave"
            | "ogg"
            | "opus"
            | "mp3"
            | "flac"
            | "wem"
            | "bnk"
            | "fsb"
            | "aif"
            | "aiff"
            | "wma"
            | "xma"
            | "mid"
            | "midi"
    )
}

fn is_midi(path: &str) -> bool {
    let extension = path.rsplit('.').next().unwrap_or("");
    extension.eq_ignore_ascii_case("mid") || extension.eq_ignore_ascii_case("midi")
}

fn cache_family(name: &str) -> (Part, &str) {
    match name.split_once('.') {
        Some(("H", family)) => (Part::Header, family),
        Some(("B", family)) => (Part::Body, family),
        Some(("F", family)) => (Part::Stream, family),
        _ => (Part::Body, name),
    }
}

fn add_cache_assets(
    assets: &mut BTreeMap<(String, String), Asset>,
    cache_index: usize,
    name: &str,
    entries: &[TocEntry],
) {
    let (part, family) = cache_family(name);
    for (entry_index, entry) in entries.iter().enumerate() {
        if entry.is_dir || !is_audio_file(&entry.full_path) {
            continue;
        }
        let path = entry.full_path.replace('\\', "/");
        let key = (family.to_ascii_lowercase(), path.to_ascii_lowercase());
        let asset = assets.entry(key).or_insert_with(|| Asset {
            family: family.to_owned(),
            path,
            parts: BTreeMap::new(),
        });
        // Append-only TOCs can contain several records for one file. Select the
        // newest timestamp, then the last record, just like the other tools.
        if let Some(previous) = asset.parts.get(&part) {
            if previous.rank > (entry.timestamp, entry.index) {
                continue;
            }
        }
        asset.parts.insert(
            part,
            Location {
                cache: cache_index,
                entry: entry_index,
                rank: (entry.timestamp, entry.index),
            },
        );
    }
}

fn discover(cache_path: &Path) -> Result<(Vec<CacheSet>, Vec<Asset>)> {
    let mut paths = fs::read_dir(cache_path)
        .with_context(|| format!("Cannot read cache directory {}", cache_path.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut caches = Vec::new();
    let mut assets = BTreeMap::new();
    for path in paths {
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("toc"))
        {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Cache name is not UTF-8")?;
        let bytes = fs::read(&path)?;
        if bytes.is_empty() {
            continue;
        }
        let entries =
            toc_decode(&bytes).with_context(|| format!("Cannot parse {}", path.display()))?;
        if !entries
            .iter()
            .any(|e| !e.is_dir && is_audio_file(&e.full_path))
        {
            continue;
        }
        let cache = LazyCache::open(cache_path, name)?
            .ok_or_else(|| anyhow!("Audio cache {name} disappeared during scan"))?;
        println!(
            "  {name}: {} audio records",
            entries
                .iter()
                .filter(|e| !e.is_dir && is_audio_file(&e.full_path))
                .count()
        );
        add_cache_assets(&mut assets, caches.len(), name, &entries);
        caches.push(CacheSet {
            name: name.to_owned(),
            cache,
        });
    }
    Ok((caches, assets.into_values().collect()))
}

fn source_records(asset: &Asset, caches: &[CacheSet]) -> Vec<Source> {
    asset
        .parts
        .values()
        .map(|location| {
            let set = &caches[location.cache];
            let entry = &set.cache.entries[location.entry];
            Source {
                cache: set.name.clone(),
                toc_index: entry.index,
                timestamp: entry.timestamp,
                offset: entry.cache_offset,
                compressed_bytes: entry.compressed_size,
                decompressed_bytes: entry.decompressed_size,
            }
        })
        .collect()
}

/// Read inventory metadata only; payloads and codec libraries are not loaded.
pub fn list_assets(cache_path: &Path) -> Result<Vec<AudioAssetInfo>> {
    let (caches, assets) = discover(cache_path)?;
    assets
        .into_iter()
        .map(|asset| {
            relative_path(&asset.family)?;
            relative_path(&asset.path)?;
            let sources = source_records(&asset, &caches);
            Ok(AudioAssetInfo {
                key: AudioAssetKey {
                    family: asset.family,
                    path: asset.path.clone(),
                },
                category: category(&asset.path).to_owned(),
                compressed_bytes: sources.iter().map(|s| u64::from(s.compressed_bytes)).sum(),
                decompressed_bytes: sources
                    .iter()
                    .map(|s| u64::from(s.decompressed_bytes))
                    .sum(),
            })
        })
        .collect()
}

fn category(path: &str) -> &'static str {
    let lower = format!("/{}/", path.to_ascii_lowercase());
    if lower.contains("/music/") {
        "music"
    } else if ["/dialog/", "/dialogue/", "/voice/", "/voices/", "/speech/"]
        .iter()
        .any(|p| lower.contains(p))
    {
        "dialogue"
    } else if lower.contains("/ambience/") || lower.contains("/ambient/") {
        "ambience"
    } else {
        "sound_effects"
    }
}

/// Preserve the game tree while preventing archive paths from escaping output.
/// Invalid Windows filename characters are encoded, so no asset is discarded.
fn relative_path(path: &str) -> Result<PathBuf> {
    let normalized = path.replace('\\', "/");
    ensure!(
        !normalized.starts_with('/') && normalized.as_bytes().get(1) != Some(&b':'),
        "Absolute audio output path: {path}"
    );
    let mut result = PathBuf::new();
    for component in normalized.split('/') {
        ensure!(
            !component.is_empty() && component != "." && component != "..",
            "Unsafe audio output path: {path}"
        );
        let mut escaped = String::new();
        for (index, ch) in component.char_indices() {
            let trailing = index + ch.len_utf8() == component.len() && matches!(ch, '.' | ' ');
            if ch.is_control() || "<>:\"|?*%".contains(ch) || trailing {
                for byte in ch.to_string().bytes() {
                    escaped.push_str(&format!("%{byte:02X}"));
                }
            } else {
                escaped.push(ch);
            }
        }
        let device = escaped.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (device.len() == 4
                && (device.starts_with("COM") || device.starts_with("LPT"))
                && matches!(device.as_bytes()[3], b'1'..=b'9'))
        {
            escaped = format!("%{:02X}{}", escaped.as_bytes()[0], &escaped[1..]);
        }
        result.push(escaped);
    }
    ensure!(!result.as_os_str().is_empty(), "Empty audio path");
    Ok(result)
}

fn prior_records(output: &Path) -> BTreeMap<(String, String), AudioRecord> {
    let path = output.join("index.json");
    if !path.exists() {
        return BTreeMap::new();
    }
    match fs::read(&path)
        .context("Cannot read audio index")
        .and_then(|b| serde_json::from_slice::<AudioIndex>(&b).context("Invalid audio index"))
    {
        Ok(index) if index.version == INDEX_VERSION => index
            .files
            .into_iter()
            .map(|r| {
                (
                    (
                        r.cache_family.to_ascii_lowercase(),
                        r.source_path.to_ascii_lowercase(),
                    ),
                    r,
                )
            })
            .collect(),
        Ok(_) => BTreeMap::new(),
        Err(error) => {
            eprintln!("  [audio] {error:#}; rebuilding exports");
            BTreeMap::new()
        }
    }
}

fn can_reuse(record: &AudioRecord, sources: &[Source], output: &Path) -> bool {
    if record.status == Status::Failed || record.sources != sources {
        return false;
    }
    if !record.playable {
        // Retry native exports on each run: installing/updating the client may
        // provide newly supported codec headers without changing an asset TOC.
        return false;
    }
    let Some((path, bytes)) = record.output_path.as_deref().zip(record.output_bytes) else {
        return false;
    };
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return false;
    }
    fs::metadata(output.join(path)).is_ok_and(|m| m.is_file() && m.len() == bytes)
}

fn read_asset(
    asset: &Asset,
    caches: &[CacheSet],
    oodle: &Oodle,
) -> (BTreeMap<Part, Vec<u8>>, Vec<String>) {
    let read = |part| -> Result<Option<Vec<u8>>> {
        asset
            .parts
            .get(&part)
            .map(|location| {
                let set = &caches[location.cache];
                let entry = &set.cache.entries[location.entry];
                set.cache
                    .get(oodle, entry)
                    .with_context(|| format!("{} TOC record {}", set.name, entry.index))
            })
            .transpose()
    };
    let mut pieces = BTreeMap::new();
    let mut errors = Vec::new();
    for part in [Part::Header, Part::Body, Part::Stream] {
        match read(part) {
            Ok(Some(bytes)) => {
                pieces.insert(part, bytes);
            }
            Ok(None) => {}
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    (pieces, errors)
}

fn save_native(
    asset: &Asset,
    pieces: &BTreeMap<Part, Vec<u8>>,
    output: &Path,
) -> Result<Vec<NativeFile>> {
    let base = PathBuf::from("native")
        .join(relative_path(&asset.family)?)
        .join(relative_path(&asset.path)?);
    let mut files = Vec::new();
    for (part, bytes) in pieces {
        let label = match part {
            Part::Header => "H",
            Part::Body => "B",
            Part::Stream => "F",
        };
        let mut relative = base.clone();
        let name = relative
            .file_name()
            .context("Missing native audio filename")?
            .to_string_lossy();
        relative.set_file_name(format!("{name}.{label}.bin"));
        let path = output.join(&relative);
        fs::create_dir_all(path.parent().context("Missing native audio directory")?)
            .with_context(|| format!("Cannot create native audio folder for {}", path.display()))?;
        fs::write(&path, bytes)
            .with_context(|| format!("Cannot write native audio {}", path.display()))?;
        files.push(NativeFile {
            part: label.into(),
            path: relative.to_string_lossy().replace('\\', "/"),
            bytes: bytes.len() as u64,
        });
    }
    Ok(files)
}

pub fn run_extract(args: ExtractArgs) -> Result<()> {
    run_extract_inner(args, None)
}

/// Export an exact selection while retaining unrelated records in index.json.
/// Empty/stale selections never become an unfiltered full-game export.
pub fn run_extract_selected(mut args: ExtractArgs, selected: &[AudioAssetKey]) -> Result<()> {
    ensure!(!selected.is_empty(), "No audio assets selected");
    let mut keys = BTreeSet::new();
    for asset in selected {
        relative_path(&asset.family)?;
        relative_path(&asset.path)?;
        keys.insert(asset_key(&asset.family, &asset.path));
    }
    args.path_filter = None;
    args.limit = 0;
    args.list_paths = false;
    run_extract_inner(args, Some(&keys))
}

fn select_assets(assets: &mut Vec<Asset>, selected: &BTreeSet<(String, String)>) -> Result<()> {
    assets.retain(|asset| selected.contains(&asset_key(&asset.family, &asset.path)));
    let found: BTreeSet<_> = assets
        .iter()
        .map(|asset| asset_key(&asset.family, &asset.path))
        .collect();
    let missing: Vec<_> = selected.difference(&found).collect();
    ensure!(
        missing.is_empty(),
        "{} selected audio asset(s) are no longer in the cache (first: {})",
        missing.len(),
        missing
            .first()
            .map(|(family, path)| format!("{family}/{path}"))
            .unwrap_or_default()
    );
    Ok(())
}

fn run_extract_inner(
    args: ExtractArgs,
    selected: Option<&BTreeSet<(String, String)>>,
) -> Result<()> {
    let cache_path = args
        .cache_path
        .clone()
        .or_else(detect_soulframe_cache)
        .context(
        "Soulframe cache not found; use --cache-path or set SOULFRAME_ROOT to Downloaded/Public",
    )?;
    ensure!(
        cache_path.is_dir(),
        "Not a cache directory: {}",
        cache_path.display()
    );
    println!("Soulframe audio: scanning {}", cache_path.display());
    let (caches, mut assets) = discover(&cache_path)?;
    if let Some(selected) = selected {
        select_assets(&mut assets, selected)?;
    } else {
        // Proprietary player-instrument scores are available in the selective
        // browser as native data; retain legacy recorded-audio CLI defaults.
        assets.retain(|asset| !is_midi(&asset.path));
    }
    if let Some(filter) = args.path_filter.as_deref() {
        let filter = filter.to_ascii_lowercase();
        assets.retain(|a| a.path.to_ascii_lowercase().contains(&filter));
    }
    if args.limit > 0 {
        assets.truncate(args.limit);
    }
    println!("{} matching audio assets", assets.len());
    if args.list_paths {
        for asset in assets {
            println!("{}/{}", asset.family, asset.path);
        }
        return Ok(());
    }
    ensure!(
        !assets.is_empty(),
        "No matching audio assets in the installed cache"
    );
    fs::create_dir_all(&args.output).with_context(|| {
        format!(
            "Cannot create audio output folder {}",
            args.output.display()
        )
    })?;
    let mut previous = prior_records(&args.output);
    let game_exe = args.game_exe.clone().unwrap_or_else(|| {
        cache_path
            .parent()
            .unwrap_or(&cache_path)
            .join("Soulframe.x64.exe")
    });
    let vorbis = match vorbis::VorbisHeaders::from_exe(&game_exe) {
        Ok(headers) => Some(headers),
        Err(error) => {
            eprintln!("  [audio] Vorbis headers unavailable ({error:#}); affected assets will be preserved as native data. Use --game-exe with the matching client.");
            None
        }
    };
    let mut oodle = None;
    let mut index = AudioIndex {
        version: INDEX_VERSION,
        selected: assets.len(),
        matched: assets.len(),
        extracted: 0,
        unchanged: 0,
        native: 0,
        failed: 0,
        files: Vec::with_capacity(assets.len()),
    };
    for (position, asset) in assets.iter().enumerate() {
        let sources = source_records(asset, &caches);
        let key = (
            asset.family.to_ascii_lowercase(),
            asset.path.to_ascii_lowercase(),
        );
        let old = previous.remove(&key);
        if !args.force {
            if let Some(mut record) = old.filter(|r| can_reuse(r, &sources, &args.output)) {
                record.status = Status::Unchanged;
                index.files.push(record);
                index.unchanged += 1;
                continue;
            }
        }
        if oodle.is_none() {
            oodle =
                Some(Oodle::new().context("Audio extraction requires the game's Oodle library")?);
        }
        let mut record = AudioRecord {
            cache_family: asset.family.clone(),
            source_path: asset.path.clone(),
            category: category(&asset.path).to_owned(),
            sources,
            status: Status::Failed,
            playable: false,
            native_files: Vec::new(),
            output_path: None,
            output_bytes: None,
            codec: None,
            sample_rate: None,
            channels: None,
            duration_seconds: None,
            error: None,
        };
        let result = (|| -> Result<()> {
            let (pieces, read_errors) = read_asset(asset, &caches, oodle.as_ref().unwrap());
            if !read_errors.is_empty() {
                record.native_files = save_native(asset, &pieces, &args.output)?;
                bail!(
                    "Some cache pieces could not be read; readable pieces preserved: {}",
                    read_errors.join("; ")
                );
            }
            let audio = match format::reconstruct_with_vorbis(
                pieces.get(&Part::Header).map(Vec::as_slice),
                pieces.get(&Part::Body).map(Vec::as_slice),
                pieces.get(&Part::Stream).map(Vec::as_slice),
                vorbis.as_ref(),
            ) {
                Ok(audio) => audio,
                Err(error) => {
                    record.native_files = save_native(asset, &pieces, &args.output)?;
                    record.status = Status::Native;
                    record.error = Some(format!("Native cache data preserved: {error:#}"));
                    return Ok(());
                }
            };
            let mut relative = relative_path(&asset.family)?.join(relative_path(&asset.path)?);
            relative.set_extension(audio.extension);
            let path = args.output.join(&relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Cannot create audio output folder {}", parent.display())
                })?;
            }
            fs::write(&path, &audio.bytes)
                .with_context(|| format!("Cannot write {}", path.display()))?;
            record.status = Status::Extracted;
            record.playable = true;
            record.output_path = Some(relative.to_string_lossy().replace('\\', "/"));
            record.output_bytes = Some(audio.bytes.len() as u64);
            record.codec = Some(audio.codec.to_owned());
            record.sample_rate = audio.sample_rate;
            record.channels = audio.channels;
            record.duration_seconds = audio.duration_seconds;
            Ok(())
        })();
        match result {
            Ok(()) => {
                index.extracted += 1;
                if record.status == Status::Native {
                    index.native += 1;
                    eprintln!(
                        "  [audio] native data saved for {}/{}: {}",
                        asset.family,
                        asset.path,
                        record.error.as_deref().unwrap_or("")
                    );
                }
            }
            Err(error) => {
                record.error = Some(format!("{error:#}"));
                index.failed += 1;
                eprintln!("  [audio] {}/{}: {error:#}", asset.family, asset.path);
                if error.chain().any(|cause| cause.is::<std::io::Error>()) {
                    return Err(error.context(format!(
                        "Audio extraction stopped while writing to {}",
                        args.output.display()
                    )));
                }
            }
        }
        index.files.push(record);
        if (position + 1) % 250 == 0 || position + 1 == assets.len() {
            println!(
                "  audio {}/{}: {} exported ({} native), {} unchanged, {} failed",
                position + 1,
                assets.len(),
                index.extracted,
                index.native,
                index.unchanged,
                index.failed
            );
        }
    }
    // A scoped extraction updates its records without losing the existing
    // full inventory. A full extraction replaces it to drop obsolete assets.
    if selected.is_some() || args.path_filter.is_some() || args.limit > 0 {
        index.files.extend(previous.into_values());
        index.files.sort_by(|a, b| {
            (&a.cache_family, &a.source_path).cmp(&(&b.cache_family, &b.source_path))
        });
        index.matched = index.files.len();
    }
    let index_path = args.output.join("index.json");
    fs::write(&index_path, serde_json::to_vec_pretty(&index)?)
        .with_context(|| format!("Cannot write audio index {}", index_path.display()))?;
    println!(
        "Audio complete: {} exported ({} native), {} unchanged, {} failed; {}",
        index.extracted,
        index.native,
        index.unchanged,
        index.failed,
        index_path.display()
    );
    if index.failed > 0 {
        bail!(
            "{} audio assets failed; see {} for individual errors",
            index.failed,
            index_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "soulframe-audio-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(index: usize, timestamp: u64, path: &str) -> TocEntry {
        TocEntry {
            index,
            cache_offset: 0,
            timestamp,
            compressed_size: 8,
            decompressed_size: 8,
            reserved: 0,
            parent_index: 0,
            name: path.to_owned(),
            full_path: path.to_owned(),
            is_dir: false,
        }
    }

    fn cache_fixture(root: &Path, name: &str, paths: &[&str]) {
        let mut toc = vec![0x4e, 0xc6, 0x67, 0x18, 0, 0, 0, 0];
        for path in paths {
            assert!(path.len() < 64);
            let mut record = vec![0u8; 96];
            record[16..20].copy_from_slice(&4u32.to_le_bytes());
            record[20..24].copy_from_slice(&4u32.to_le_bytes());
            record[32..32 + path.len()].copy_from_slice(path.as_bytes());
            toc.extend(record);
        }
        fs::write(root.join(format!("{name}.toc")), toc).unwrap();
        fs::write(root.join(format!("{name}.cache")), [0; 4]).unwrap();
    }

    #[test]
    fn catalog_keeps_locales_and_categories_without_loading_audio() {
        let dir = TestDir::new();
        cache_fixture(
            &dir.0,
            "H.Misc",
            &["SF/Music/test.wav", "SF/Sounds/UI/click.wav"],
        );
        cache_fixture(&dir.0, "F.Misc", &["SF/Music/test.wav"]);
        cache_fixture(&dir.0, "H.Misc_fr", &["SF/Music/test.wav"]);
        cache_fixture(
            &dir.0,
            "H.Midi",
            &["SF/Music/PlayerInstrument/Midi/test.mid"],
        );
        let catalog = list_assets(&dir.0).unwrap();
        assert_eq!(catalog.len(), 4);
        let song = catalog
            .iter()
            .find(|a| a.key.family == "Misc" && a.key.path == "SF/Music/test.wav")
            .unwrap();
        assert_eq!(song.category, "music");
        assert_eq!(song.compressed_bytes, 8);
        assert_eq!(song.decompressed_bytes, 8);
        assert!(catalog.iter().any(|a| a.key.family == "Misc_fr"));
        assert!(catalog
            .iter()
            .any(|a| a.key.path.ends_with("test.mid") && a.category == "music"));
        assert!(!dir.0.join("output").exists());
    }

    #[test]
    fn exact_audio_selection_preserves_other_names_locales_and_index_records() {
        let dir = TestDir::new();
        let cache = dir.0.join("cache");
        let output = dir.0.join("output");
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(&output).unwrap();
        cache_fixture(&cache, "H.Misc", &["test.wav", "test-long.wav"]);
        cache_fixture(&cache, "H.Misc_fr", &["test.wav"]);
        let (caches, assets) = discover(&cache).unwrap();
        let records: Vec<_> = assets
            .iter()
            .enumerate()
            .map(|(index, asset)| {
                let file = format!("prior-{index}.ogg");
                fs::write(output.join(&file), [1, 2, 3]).unwrap();
                AudioRecord {
                    cache_family: asset.family.clone(),
                    source_path: asset.path.clone(),
                    category: category(&asset.path).into(),
                    sources: source_records(asset, &caches),
                    status: Status::Extracted,
                    playable: true,
                    native_files: vec![],
                    output_path: Some(file),
                    output_bytes: Some(3),
                    codec: None,
                    sample_rate: None,
                    channels: None,
                    duration_seconds: None,
                    error: None,
                }
            })
            .collect();
        let prior = AudioIndex {
            version: INDEX_VERSION,
            selected: 3,
            matched: 3,
            extracted: 3,
            unchanged: 0,
            native: 0,
            failed: 0,
            files: records,
        };
        fs::write(
            output.join("index.json"),
            serde_json::to_vec(&prior).unwrap(),
        )
        .unwrap();
        // Reuse makes this a complete selected-export test with no native codec
        // dependency. Only the exact English filename may become Unchanged.
        run_extract_selected(
            ExtractArgs {
                cache_path: Some(cache),
                output: output.clone(),
                game_exe: Some(dir.0.join("absent.exe")),
                path_filter: Some("ignored legacy filter".into()),
                limit: 1,
                ..Default::default()
            },
            &[AudioAssetKey {
                family: "misc".into(),
                path: "TEST.WAV".into(),
            }],
        )
        .unwrap();
        let index: AudioIndex =
            serde_json::from_slice(&fs::read(output.join("index.json")).unwrap()).unwrap();
        assert_eq!(index.selected, 1);
        assert_eq!(index.unchanged, 1);
        assert_eq!(index.matched, 3);
        assert_eq!(index.files.len(), 3);
        assert_eq!(
            index
                .files
                .iter()
                .filter(|r| r.status == Status::Unchanged)
                .count(),
            1
        );
        let changed = index
            .files
            .iter()
            .find(|r| r.status == Status::Unchanged)
            .unwrap();
        assert_eq!(
            (&*changed.cache_family, &*changed.source_path),
            ("Misc", "test.wav")
        );
        for record in &index.files {
            assert_eq!(
                fs::read(output.join(record.output_path.as_ref().unwrap())).unwrap(),
                [1, 2, 3]
            );
        }
    }

    #[test]
    fn empty_stale_and_unsafe_audio_selection_never_creates_output() {
        let dir = TestDir::new();
        cache_fixture(&dir.0, "H.Misc", &["test.wav"]);
        let args = || ExtractArgs {
            cache_path: Some(dir.0.clone()),
            output: dir.0.join("output"),
            ..Default::default()
        };
        assert!(run_extract_selected(args(), &[])
            .unwrap_err()
            .to_string()
            .contains("No audio assets selected"));
        assert!(run_extract_selected(
            args(),
            &[AudioAssetKey {
                family: "Misc_fr".into(),
                path: "test.wav".into()
            }]
        )
        .unwrap_err()
        .to_string()
        .contains("no longer in the cache"));
        for (family, path) in [
            ("../escape", "test.wav"),
            ("Misc", "../escape.wav"),
            ("Misc", "/absolute.wav"),
            ("Misc", "C:/escape.wav"),
        ] {
            assert!(run_extract_selected(
                args(),
                &[AudioAssetKey {
                    family: family.into(),
                    path: path.into()
                }]
            )
            .is_err());
        }
        assert!(!dir.0.join("output").exists());
    }

    #[test]
    fn newest_toc_record_wins_and_localized_cache_families_stay_distinct() {
        let mut assets = BTreeMap::new();
        let entries = vec![
            entry(1, 10, "SF/Sounds/Test.wav"),
            entry(2, 30, "SF/Sounds/Test.wav"),
            entry(3, 20, "SF/Sounds/Test.wav"),
            entry(4, 30, "SF/Sounds/Test.wav"),
        ];
        add_cache_assets(&mut assets, 0, "H.Misc", &entries);
        add_cache_assets(&mut assets, 1, "F.Misc", &entries);
        add_cache_assets(&mut assets, 2, "H.Misc_xx", &entries);
        assert_eq!(assets.len(), 2);
        let standard = &assets[&("misc".into(), "sf/sounds/test.wav".into())];
        assert_eq!(standard.parts[&Part::Header].entry, 3);
        assert_eq!(standard.parts[&Part::Stream].cache, 1);
    }

    #[test]
    fn archive_paths_cannot_escape_output_and_windows_names_are_encoded() {
        for unsafe_path in [
            "../escape.wav",
            "SF/../../escape",
            "C:/escape.wav",
            "/absolute.wav",
            "SF/./test",
        ] {
            assert!(relative_path(unsafe_path).is_err(), "{unsafe_path}");
        }
        assert_eq!(
            relative_path("SF/Sounds/CON.wav").unwrap(),
            PathBuf::from("SF/Sounds/%43ON.wav")
        );
        assert_eq!(
            relative_path("SF/a?b%.wav").unwrap(),
            PathBuf::from("SF/a%3Fb%25.wav")
        );
        assert_eq!(
            relative_path("SF\\Music\\Song.wav").unwrap(),
            PathBuf::from("SF/Music/Song.wav")
        );
    }

    #[test]
    fn audio_selection_includes_effects_and_impulse_responses_outside_music() {
        for path in [
            "SF/Music/Score.WAV",
            "SF/Sounds/Dialog/Test.wav",
            "SF/Levels/Reverb/Impulse.wav",
            "sound.ogg",
            "SF/Music/PlayerInstrument/Midi/test.mid",
        ] {
            assert!(is_audio_file(path));
        }
        assert!(!is_audio_file("SF/Music/Score.bankmeta"));
        assert_eq!(category("SF/Music/Score.wav"), "music");
        assert_eq!(category("SF/Sounds/Dialog/Test_en.wav"), "dialogue");
        assert_eq!(category("SF/Sounds/Ambience/Forest.wav"), "ambience");
        assert_eq!(category("SF/Sounds/UI/Click.wav"), "sound_effects");
    }

    #[test]
    fn list_paths_needs_neither_oodle_nor_game_exe_and_does_not_create_output() {
        let dir = TestDir::new();
        let mut toc = vec![0x4e, 0xc6, 0x67, 0x18, 0, 0, 0, 0];
        let mut record = vec![0u8; 96];
        record[16..20].copy_from_slice(&4u32.to_le_bytes());
        record[20..24].copy_from_slice(&4u32.to_le_bytes());
        record[32..41].copy_from_slice(b"test.wav\0");
        toc.extend_from_slice(&record);
        fs::write(dir.0.join("H.Misc.toc"), toc).unwrap();
        fs::write(dir.0.join("H.Misc.cache"), [0; 4]).unwrap();
        let output = dir.0.join("output");
        run_extract(ExtractArgs {
            cache_path: Some(dir.0.clone()),
            output: output.clone(),
            list_paths: true,
            ..Default::default()
        })
        .unwrap();
        assert!(!output.exists());
    }

    #[test]
    fn incremental_export_checks_source_revision_output_size_and_escaped_path() {
        let dir = TestDir::new();
        fs::write(dir.0.join("%43ON.ogg"), [1, 2, 3]).unwrap();
        let sources = vec![Source {
            cache: "F.Misc".into(),
            toc_index: 1,
            timestamp: 10,
            offset: 0,
            compressed_bytes: 3,
            decompressed_bytes: 3,
        }];
        let mut record = AudioRecord {
            cache_family: "Misc".into(),
            source_path: "CON.wav".into(),
            category: "sound_effects".into(),
            sources: sources.clone(),
            status: Status::Extracted,
            playable: true,
            native_files: vec![],
            output_path: Some("%43ON.ogg".into()),
            output_bytes: Some(3),
            codec: Some("opus".into()),
            sample_rate: Some(48_000),
            channels: Some(1),
            duration_seconds: Some(0.02),
            error: None,
        };
        assert!(can_reuse(&record, &sources, &dir.0));
        let mut updated = sources.clone();
        updated[0].timestamp += 1;
        assert!(!can_reuse(&record, &updated, &dir.0));
        record.output_bytes = Some(4);
        assert!(!can_reuse(&record, &sources, &dir.0));
        record.output_bytes = Some(3);
        record.output_path = Some("../%43ON.ogg".into());
        assert!(!can_reuse(&record, &sources, &dir.0));
    }

    #[test]
    fn native_output_failure_keeps_io_cause_and_destination_path() {
        let dir = TestDir::new();
        fs::write(dir.0.join("native"), b"existing file").unwrap();
        let asset = Asset {
            family: "Misc".into(),
            path: "SF/Sounds/Test.wav".into(),
            parts: BTreeMap::new(),
        };
        let pieces = BTreeMap::from([(Part::Header, vec![1, 2, 3])]);
        let error = save_native(&asset, &pieces, &dir.0).unwrap_err();
        assert!(format!("{error:#}").contains("Test.wav.H.bin"));
        assert!(error.chain().any(|cause| cause.is::<std::io::Error>()));
        assert_eq!(fs::read(dir.0.join("native")).unwrap(), b"existing file");
    }
}
