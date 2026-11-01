//! Read-only media catalog and exact, manifest-based extraction requests.

use crate::tasks::{Job, RunPlan, RunSettings, TaskId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const WORKER_COMMAND: &str = "cache-extract";
const MAX_ASSETS: usize = 2_000_000;
const MAX_MANIFEST_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PAGE: usize = 500;
const STALE: &str = "The game cache changed. Refresh the cache browser before continuing.";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    All,
    Images,
    Audio,
    Models,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub id: String,
    pub path: String,
    pub source_path: String,
    pub name: String,
    pub kind: MediaKind,
    pub family: Option<String>,
    pub category: Option<String>,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
    pub cache_sets: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheLookup {
    pub cache_path: PathBuf,
    pub entry: CacheEntry,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SelectionSummary {
    pub count: usize,
    pub images: usize,
    pub audio: usize,
    pub models: usize,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BrowseRequest {
    pub generation: Option<u64>,
    pub folder: String,
    pub query: String,
    pub kind: MediaKind,
    pub offset: usize,
    pub limit: usize,
    pub refresh: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheRow {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub is_dir: bool,
    pub count: usize,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
    pub selected: bool,
    pub partial: bool,
    pub category: Option<String>,
    pub cache_sets: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowseResult {
    pub generation: u64,
    pub cache_path: PathBuf,
    pub folder: String,
    pub query: String,
    pub kind: MediaKind,
    pub total: usize,
    pub total_assets: usize,
    pub matching_assets: usize,
    pub offset: usize,
    pub limit: usize,
    pub entries: Vec<CacheRow>,
    pub selection: SelectionSummary,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionAction {
    Add,
    Remove,
    Replace,
    Clear,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SelectionRequest {
    pub generation: u64,
    pub action: SelectionAction,
    pub ids: Option<Vec<String>>,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub kind: MediaKind,
}

struct Catalog {
    generation: u64,
    settings_key: String,
    cache_path: PathBuf,
    entries: Vec<CacheEntry>,
    by_id: HashMap<String, usize>,
    summary: SelectionSummary,
}

#[derive(Default)]
struct BrowserState {
    catalog: Option<Arc<Catalog>>,
    selected: Arc<HashSet<String>>,
    summary: SelectionSummary,
}

/// Copying a request is constant time even when every asset is selected.
pub struct ExtractionSnapshot {
    catalog: Arc<Catalog>,
    selected: Arc<HashSet<String>>,
    all: bool,
    pub summary: SelectionSummary,
}

impl ExtractionSnapshot {
    pub fn cache_path(&self) -> &Path {
        &self.catalog.cache_path
    }

    pub fn manifest(&self, output: &Path, format: &str) -> Result<CacheManifest, String> {
        manifest_from_entries(
            &self.catalog.cache_path,
            output,
            self.catalog
                .entries
                .iter()
                .filter(|e| self.all || self.selected.contains(&e.id)),
            self.summary.count,
            format,
        )
    }
}

#[derive(Default)]
pub struct CacheBrowserService {
    state: Mutex<BrowserState>,
    revision: AtomicU64,
}

fn settings_key(settings: &RunSettings) -> String {
    format!(
        "{}|{}",
        settings.workspace.display(),
        settings
            .cache_path
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default()
    )
    .to_lowercase()
}

/// A cache name is never allowed to become an absolute or parent output path.
fn safe_relative(value: &str) -> Result<String, String> {
    let value = value.replace('\\', "/");
    if value.is_empty()
        || value.len() > 4096
        || value.starts_with('/')
        || value.contains(':')
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("The cache contains an unsafe asset path.".into());
    }
    Ok(value)
}

fn folder_path(value: &str) -> Result<String, String> {
    if value.is_empty() {
        Ok(String::new())
    } else {
        safe_relative(value.trim_end_matches('/'))
    }
}

fn in_folder(path: &str, folder: &str) -> bool {
    folder.is_empty()
        || (path
            .get(..folder.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(folder))
            && path.as_bytes().get(folder.len()) == Some(&b'/'))
}

fn matches(entry: &CacheEntry, folder: &str, query: &str, kind: MediaKind) -> bool {
    (kind == MediaKind::All || entry.kind == kind)
        && in_folder(&entry.path, folder)
        && (query.is_empty() || entry.path.to_lowercase().contains(query))
}

fn summarize<'a>(entries: impl Iterator<Item = &'a CacheEntry>) -> SelectionSummary {
    let mut result = SelectionSummary::default();
    for entry in entries {
        result.count += 1;
        result.images += usize::from(entry.kind == MediaKind::Images);
        result.audio += usize::from(entry.kind == MediaKind::Audio);
        result.models += usize::from(entry.kind == MediaKind::Models);
        result.compressed_bytes = result
            .compressed_bytes
            .saturating_add(entry.compressed_bytes);
        result.decompressed_bytes = result
            .decompressed_bytes
            .saturating_add(entry.decompressed_bytes);
    }
    result
}

fn asset_row(entry: &CacheEntry, selected: bool) -> CacheRow {
    CacheRow {
        id: entry.id.clone(),
        path: entry.path.clone(),
        name: entry.name.clone(),
        kind: match entry.kind {
            MediaKind::Images => "images",
            MediaKind::Models => "models",
            _ => "audio",
        }
        .into(),
        is_dir: false,
        count: 1,
        compressed_bytes: entry.compressed_bytes,
        decompressed_bytes: entry.decompressed_bytes,
        selected,
        partial: false,
        category: entry.category.clone(),
        cache_sets: entry.cache_sets.clone(),
    }
}

impl CacheBrowserService {
    pub fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut state) = self.state.lock() {
            state.catalog = None;
            state.selected = Arc::default();
            state.summary = SelectionSummary::default();
        }
    }

    pub fn cancel_scan(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn current(&self, settings: &RunSettings, generation: u64) -> Result<Arc<Catalog>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "The cache browser is unavailable.")?;
        state
            .catalog
            .as_ref()
            .filter(|catalog| {
                catalog.generation == generation && catalog.settings_key == settings_key(settings)
            })
            .cloned()
            .ok_or_else(|| STALE.into())
    }

    fn catalog(&self, settings: &RunSettings, refresh: bool) -> Result<Arc<Catalog>, String> {
        let key = settings_key(settings);
        if !refresh {
            let state = self
                .state
                .lock()
                .map_err(|_| "The cache browser is unavailable.")?;
            if let Some(catalog) = state.catalog.as_ref().filter(|c| c.settings_key == key) {
                return Ok(catalog.clone());
            }
        }
        let ticket = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let report = crate::preflight::inspect_cache(settings);
        if !report.found {
            return Err(report.error.unwrap_or_else(|| {
                "Soulframe cache not found. Choose its folder in Settings.".into()
            }));
        }
        let cache_path = report
            .path
            .ok_or("The cache folder could not be resolved.")?;
        let images = soulframe_images::cli::list_assets(&cache_path)
            .map_err(|e| format!("Cannot catalog images: {e:#}"))?;
        if self.revision.load(Ordering::Acquire) != ticket {
            return Err("Cache scan cancelled. Refresh to try again.".into());
        }
        let audio = soulframe_audio::list_assets(&cache_path)
            .map_err(|e| format!("Cannot catalog audio: {e:#}"))?;
        let models = soulframe_models::cli::list_assets(&cache_path)
            .map_err(|e| format!("Cannot catalog models: {e:#}"))?;
        if images.len() + audio.len() + models.len() > MAX_ASSETS {
            return Err("The game cache exceeds the browser's two million asset limit.".into());
        }
        let mut entries = Vec::with_capacity(images.len() + audio.len() + models.len());
        for image in images {
            let source_path = safe_relative(&image.path)?;
            let path = format!("images/{source_path}");
            let category = source_path
                .rsplit('.')
                .next()
                .filter(|extension| {
                    extension.eq_ignore_ascii_case("hdr") || extension.eq_ignore_ascii_case("exr")
                })
                .map(|_| "Native texture".into());
            entries.push(CacheEntry {
                id: format!("image:{source_path}"),
                name: source_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&source_path)
                    .into(),
                path,
                source_path,
                kind: MediaKind::Images,
                family: None,
                category,
                compressed_bytes: image.compressed_bytes,
                decompressed_bytes: image.decompressed_bytes,
                cache_sets: image.cache_sets,
            });
        }
        for audio in audio {
            let source_path = safe_relative(&audio.key.path)?;
            let family = safe_relative(&audio.key.family)?;
            let path = format!("audio/{family}/{source_path}");
            entries.push(CacheEntry {
                id: format!("audio:{family}/{source_path}"),
                name: source_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&source_path)
                    .into(),
                path,
                source_path,
                kind: MediaKind::Audio,
                family: Some(family),
                category: Some(audio.category),
                compressed_bytes: audio.compressed_bytes,
                decompressed_bytes: audio.decompressed_bytes,
                cache_sets: Vec::new(),
            });
        }
        for model in models {
            let source_path = safe_relative(&model.path)?;
            entries.push(CacheEntry {
                id: format!("model:{source_path}"),
                name: source_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&source_path)
                    .into(),
                path: format!("models/{source_path}"),
                source_path,
                kind: MediaKind::Models,
                family: None,
                category: Some("Static mesh".into()),
                compressed_bytes: model.compressed_bytes,
                decompressed_bytes: model.decompressed_bytes,
                cache_sets: Vec::new(),
            });
        }
        entries.sort_by_cached_key(|entry| entry.path.to_lowercase());
        let by_id = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect::<HashMap<_, _>>();
        if by_id.len() != entries.len() {
            return Err("The cache contains duplicate media identities. Refresh after the game update finishes.".into());
        }
        let catalog = Arc::new(Catalog {
            generation: ticket,
            settings_key: key,
            cache_path,
            summary: summarize(entries.iter()),
            entries,
            by_id,
        });
        let mut state = self
            .state
            .lock()
            .map_err(|_| "The cache browser is unavailable.")?;
        if self.revision.load(Ordering::Acquire) != ticket {
            return Err("Cache scan cancelled. Refresh to try again.".into());
        }
        state.catalog = Some(catalog.clone());
        state.selected = Arc::default();
        state.summary = SelectionSummary::default();
        Ok(catalog)
    }

    pub fn browse(
        &self,
        settings: &RunSettings,
        request: BrowseRequest,
    ) -> Result<BrowseResult, String> {
        let catalog = self.catalog(settings, request.refresh)?;
        if !request.refresh && request.generation.is_some_and(|g| g != catalog.generation) {
            return Err(STALE.into());
        }
        let folder = folder_path(&request.folder)?;
        let query = request.query.trim().to_lowercase();
        if query.len() > 1024 {
            return Err("Use a search shorter than 1,024 characters.".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "The cache browser is unavailable.")?;
        if state.catalog.as_ref().map(|c| c.generation) != Some(catalog.generation) {
            return Err(STALE.into());
        }
        // Keep only immediate folder aggregates in memory. Asset rows are
        // materialized after pagination, even for a recursive whole-game search.
        let mut rows: BTreeMap<String, (CacheRow, usize)> = BTreeMap::new();
        let mut matching_assets = 0;
        let mut files = 0;
        for entry in catalog
            .entries
            .iter()
            .filter(|e| matches(e, &folder, &query, request.kind))
        {
            matching_assets += 1;
            let relative = if folder.is_empty() {
                entry.path.as_str()
            } else {
                &entry.path[folder.len() + 1..]
            };
            let selected = state.selected.contains(&entry.id);
            // A search presents exact assets recursively; ordinary browsing presents folders.
            if query.is_empty() && relative.contains('/') {
                let name = relative.split('/').next().unwrap();
                let path = if folder.is_empty() {
                    name.to_string()
                } else {
                    format!("{folder}/{name}")
                };
                let (row, selected_count) = rows
                    .entry(format!("0:{}", path.to_lowercase()))
                    .or_insert_with(|| {
                        (
                            CacheRow {
                                id: format!("folder:{path}"),
                                path,
                                name: name.into(),
                                kind: "folder".into(),
                                is_dir: true,
                                count: 0,
                                compressed_bytes: 0,
                                decompressed_bytes: 0,
                                selected: false,
                                partial: false,
                                category: None,
                                cache_sets: Vec::new(),
                            },
                            0,
                        )
                    });
                row.count += 1;
                row.compressed_bytes = row.compressed_bytes.saturating_add(entry.compressed_bytes);
                row.decompressed_bytes = row
                    .decompressed_bytes
                    .saturating_add(entry.decompressed_bytes);
                *selected_count += usize::from(selected);
            } else {
                files += 1;
            }
        }
        let folder_count = rows.len();
        let total = folder_count + files;
        let limit = if request.limit == 0 {
            100
        } else {
            request.limit.min(MAX_PAGE)
        };
        let offset = request.offset.min(total);
        let mut entries: Vec<_> = rows
            .into_values()
            .skip(offset)
            .take(limit)
            .map(|(mut row, count)| {
                if row.is_dir {
                    row.selected = count == row.count;
                    row.partial = count > 0 && count < row.count;
                }
                row
            })
            .collect();
        if entries.len() < limit {
            let skip_files = offset.saturating_sub(folder_count);
            entries.extend(
                catalog
                    .entries
                    .iter()
                    .filter(|e| matches(e, &folder, &query, request.kind))
                    .filter(|entry| {
                        let relative = if folder.is_empty() {
                            entry.path.as_str()
                        } else {
                            &entry.path[folder.len() + 1..]
                        };
                        !query.is_empty() || !relative.contains('/')
                    })
                    .skip(skip_files)
                    .take(limit - entries.len())
                    .map(|entry| asset_row(entry, state.selected.contains(&entry.id))),
            );
        }
        let selection = state.summary.clone();
        Ok(BrowseResult {
            generation: catalog.generation,
            cache_path: catalog.cache_path.clone(),
            folder,
            query: request.query,
            kind: request.kind,
            total,
            total_assets: catalog.entries.len(),
            matching_assets,
            offset,
            limit,
            entries,
            selection,
        })
    }

    pub fn select(
        &self,
        settings: &RunSettings,
        request: SelectionRequest,
    ) -> Result<SelectionSummary, String> {
        let catalog = self.current(settings, request.generation)?;
        let folder = folder_path(&request.folder)?;
        let query = request.query.trim().to_lowercase();
        if query.len() > 1024 {
            return Err("Use a search shorter than 1,024 characters.".into());
        }
        let mut targets = HashSet::new();
        if !matches!(request.action, SelectionAction::Clear) {
            if let Some(ids) = request.ids {
                if ids.len() > MAX_ASSETS {
                    return Err("Too many asset IDs in one selection request.".into());
                }
                for id in ids {
                    if let Some(folder) = id.strip_prefix("folder:") {
                        let folder = folder_path(folder)?;
                        let before = targets.len();
                        targets.extend(
                            catalog
                                .entries
                                .iter()
                                .filter(|e| matches(e, &folder, "", request.kind))
                                .map(|e| e.id.clone()),
                        );
                        if targets.len() == before
                            && !catalog.entries.iter().any(|e| in_folder(&e.path, &folder))
                        {
                            return Err(
                                "That cache folder no longer exists. Refresh the browser.".into()
                            );
                        }
                    } else if catalog.by_id.contains_key(&id) {
                        targets.insert(id);
                    } else {
                        return Err(
                            "That asset is no longer in this cache catalog. Refresh the browser."
                                .into(),
                        );
                    }
                }
            } else {
                targets.extend(
                    catalog
                        .entries
                        .iter()
                        .filter(|e| matches(e, &folder, &query, request.kind))
                        .map(|e| e.id.clone()),
                );
            }
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "The cache browser is unavailable.")?;
        if state.catalog.as_ref().map(|c| c.generation) != Some(request.generation) {
            return Err(STALE.into());
        }
        let selected = Arc::make_mut(&mut state.selected);
        match request.action {
            SelectionAction::Add => selected.extend(targets),
            SelectionAction::Remove => selected.retain(|id| !targets.contains(id)),
            SelectionAction::Replace => *selected = targets,
            SelectionAction::Clear => selected.clear(),
        }
        let summary = summarize(catalog.entries.iter().filter(|e| selected.contains(&e.id)));
        state.summary = summary.clone();
        Ok(summary)
    }

    pub fn lookup(
        &self,
        settings: &RunSettings,
        generation: u64,
        id: &str,
    ) -> Result<CacheLookup, String> {
        let catalog = self.current(settings, generation)?;
        let index = catalog
            .by_id
            .get(id)
            .ok_or("That asset is not in this cache catalog.")?;
        Ok(CacheLookup {
            cache_path: catalog.cache_path.clone(),
            entry: catalog.entries[*index].clone(),
        })
    }

    #[cfg(test)]
    pub fn selected(
        &self,
        settings: &RunSettings,
        generation: u64,
        all: bool,
    ) -> Result<(PathBuf, Vec<CacheEntry>), String> {
        let catalog = self.current(settings, generation)?;
        let state = self
            .state
            .lock()
            .map_err(|_| "The cache browser is unavailable.")?;
        if state.catalog.as_ref().map(|c| c.generation) != Some(generation) {
            return Err(STALE.into());
        }
        let entries: Vec<_> = catalog
            .entries
            .iter()
            .filter(|e| all || state.selected.contains(&e.id))
            .cloned()
            .collect();
        if entries.is_empty() {
            return Err("Select at least one image or audio asset to extract.".into());
        }
        Ok((catalog.cache_path.clone(), entries))
    }

    pub fn snapshot(
        &self,
        settings: &RunSettings,
        generation: u64,
        all: bool,
    ) -> Result<ExtractionSnapshot, String> {
        let catalog = self.current(settings, generation)?;
        let state = self
            .state
            .lock()
            .map_err(|_| "The cache browser is unavailable.")?;
        if state.catalog.as_ref().map(|c| c.generation) != Some(generation) {
            return Err(STALE.into());
        }
        let summary = if all {
            catalog.summary.clone()
        } else {
            state.summary.clone()
        };
        if summary.count == 0 {
            return Err("Select at least one image or audio asset to extract.".into());
        }
        Ok(ExtractionSnapshot {
            catalog,
            selected: state.selected.clone(),
            all,
            summary,
        })
    }

    pub fn prepare_extraction(
        &self,
        settings: &RunSettings,
        generation: u64,
        all: bool,
    ) -> Result<(RunSettings, ExtractionSnapshot), String> {
        // Catalog generations belong to the saved settings. Runtime path
        // resolution can add a Windows extended prefix or pin an auto-detected
        // cache; neither operation means the user's catalog changed.
        let selection = self.snapshot(settings, generation, all)?;
        let mut resolved = crate::tasks::resolved(settings)?;
        resolved.cache_path = Some(selection.cache_path().to_path_buf());
        Ok((resolved, selection))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAudioKey {
    pub family: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheManifest {
    pub version: u32,
    pub cache_path: PathBuf,
    pub output: PathBuf,
    pub image_format: String,
    pub images: Vec<String>,
    pub audio: Vec<ManifestAudioKey>,
    #[serde(default)]
    pub models: Vec<String>,
}

pub fn manifest_for_entries(
    cache_path: &Path,
    output: &Path,
    entries: &[CacheEntry],
    format: &str,
) -> Result<CacheManifest, String> {
    manifest_from_entries(cache_path, output, entries.iter(), entries.len(), format)
}

fn manifest_from_entries<'a>(
    cache_path: &Path,
    output: &Path,
    entries: impl Iterator<Item = &'a CacheEntry>,
    count: usize,
    format: &str,
) -> Result<CacheManifest, String> {
    if !matches!(format, "png" | "dds") {
        return Err("Choose PNG or DDS image output.".into());
    }
    if count == 0 || count > MAX_ASSETS {
        return Err("Select between one and two million media assets.".into());
    }
    let mut manifest = CacheManifest {
        version: 1,
        cache_path: cache_path.to_path_buf(),
        output: output.to_path_buf(),
        image_format: format.into(),
        images: Vec::new(),
        audio: Vec::new(),
        models: Vec::new(),
    };
    let mut estimated_bytes = 1024u64;
    for entry in entries {
        let path = safe_relative(&entry.source_path)?;
        estimated_bytes = estimated_bytes.saturating_add(
            128 + path.len() as u64 * 2 + entry.family.as_ref().map_or(0, |f| f.len() as u64 * 2),
        );
        if estimated_bytes > MAX_MANIFEST_BYTES {
            return Err("The media selection manifest exceeds 128 MiB.".into());
        }
        match entry.kind {
            MediaKind::Images => manifest.images.push(path),
            MediaKind::Audio => manifest.audio.push(ManifestAudioKey {
                family: safe_relative(
                    entry
                        .family
                        .as_deref()
                        .ok_or("Audio asset has no cache family.")?,
                )?,
                path,
            }),
            MediaKind::Models => manifest.models.push(path),
            MediaKind::All => {
                return Err("A media entry must be an image, audio or model asset.".into())
            }
        }
    }
    Ok(manifest)
}

pub fn write_manifest_at(path: &Path, manifest: &CacheManifest) -> Result<(), String> {
    let bytes = serde_json::to_vec(manifest)
        .map_err(|e| format!("Cannot encode the media selection: {e}"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("The media selection manifest exceeds 128 MiB.".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create selection manifest {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .map_err(|e| format!("Cannot write selection manifest {}: {e}", path.display()))
}

/// The caller owns exactly this newly-created directory, never an arbitrary work tree.
#[derive(Clone)]
pub struct OwnedManifest {
    pub directory: PathBuf,
    pub path: PathBuf,
    created: Arc<AtomicBool>,
}

impl OwnedManifest {
    pub fn planned(workspace: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = workspace.join(".soulkit-work").join(format!(
            "media-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self {
            path: directory.join("selection.json"),
            directory,
            created: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn create(&self, workspace: &Path, output: &Path) -> Result<(), String> {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|e| format!("Cannot resolve the workspace: {e}"))?;
        let root = workspace.join(".soulkit-work");
        std::fs::create_dir_all(&root).map_err(|e| {
            format!(
                "Cannot create media selection work folder {}: {e}",
                root.display()
            )
        })?;
        let root = std::fs::canonicalize(root)
            .map_err(|e| format!("Cannot resolve the media selection work folder: {e}"))?;
        if !root.starts_with(&workspace) {
            return Err("The extraction work folder points outside the workspace.".into());
        }
        // Work manifests never belong in the export tree.
        let output = crate::config::absolute_path(&workspace, output)?;
        if root.starts_with(&output) {
            return Err("Choose an output folder that does not contain .soulkit-work.".into());
        }
        let parent = self
            .directory
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .ok_or("Cannot resolve selection work folder.")?;
        if parent != root {
            return Err("The selection manifest is outside its owned work folder.".into());
        }
        std::fs::create_dir(&self.directory).map_err(|e| {
            format!(
                "Cannot create selection work folder {}: {e}",
                self.directory.display()
            )
        })?;
        self.created.store(true, Ordering::Release);
        Ok(())
    }

    pub fn cleanup(&self) -> Result<(), String> {
        if !self.created.load(Ordering::Acquire) {
            return Ok(());
        }
        // No recursive deletion: this directory is ours and only the manifest
        // is written here. Unknown files are deliberately preserved.
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot remove selection manifest: {e}")),
        }
        match std::fs::remove_dir(&self.directory) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Cannot remove selection work folder: {e}")),
        }
    }

    pub fn exists(&self) -> bool {
        self.created.load(Ordering::Acquire)
    }
}

pub fn extraction_plan(
    path: &Path,
    images: bool,
    audio: bool,
    models: bool,
    count: usize,
) -> RunPlan {
    let modules: Vec<_> = [
        (images, TaskId::Images),
        (audio, TaskId::Audio),
        (models, TaskId::Models),
    ]
    .into_iter()
    .filter_map(|(yes, task)| yes.then_some(task))
    .collect();
    RunPlan {
        requested: modules.clone(),
        modules,
        dependencies: Vec::new(),
        jobs: vec![Job {
            id: "cache-selection".into(),
            label: format!("Extract {count} selected cache assets"),
            args: worker_args(path),
        }],
    }
}

pub fn worker_args(path: &Path) -> Vec<String> {
    vec![WORKER_COMMAND.into(), path.to_string_lossy().into_owned()]
}

pub fn run_manifest(path: &Path) -> Result<(), String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Cannot read media selection {}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_MANIFEST_BYTES {
        return Err("The media selection manifest exceeds 128 MiB.".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("The media selection manifest exceeds 128 MiB.".into());
    }
    let mut manifest: CacheManifest = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid media selection manifest: {e}"))?;
    if manifest.version != 1 || !matches!(manifest.image_format.as_str(), "png" | "dds") {
        return Err("Unsupported media selection manifest version or image format.".into());
    }
    let count = manifest.images.len() + manifest.audio.len() + manifest.models.len();
    if count == 0 || count > MAX_ASSETS {
        return Err("The media selection must contain between one and two million assets.".into());
    }
    if !manifest.cache_path.is_absolute() || !manifest.output.is_absolute() {
        return Err("Media selection folders must be absolute paths.".into());
    }
    for image in &manifest.images {
        safe_relative(image)?;
    }
    for audio in &manifest.audio {
        safe_relative(&audio.family)?;
        safe_relative(&audio.path)?;
    }
    for model in &manifest.models {
        safe_relative(model)?;
    }
    let stages = usize::from(!manifest.images.is_empty())
        + usize::from(!manifest.audio.is_empty())
        + usize::from(!manifest.models.is_empty());
    let mut stage = 0;
    if !manifest.images.is_empty() {
        stage += 1;
        println!(
            "# soulkit-cli run-all - stage {stage}/{stages}: selected images ({})",
            manifest.images.len()
        );
        soulframe_images::cli::run_extract_selected(
            soulframe_images::cli::ExtractArgs {
                cache_path: Some(manifest.cache_path.clone()),
                output: manifest.output.join("images"),
                filter: None,
                path_filter: None,
                limit: 0,
                list_caches: false,
                list_paths: false,
                list_all: false,
                path_filter_substring: false,
                flat: false,
                // Current PC cache textures use linear blocks. Applying the
                // legacy console swizzle corrupts interface artwork.
                deswizzle_textures: false,
                fix_dims: true,
                output_format: manifest.image_format.clone(),
            },
            &manifest.images,
        )
        .map_err(|e| format!("Selected image extraction failed: {e:#}"))?;
    }
    if !manifest.audio.is_empty() {
        stage += 1;
        println!(
            "# soulkit-cli run-all - stage {stage}/{stages}: selected audio ({})",
            manifest.audio.len()
        );
        let keys: Vec<_> = std::mem::take(&mut manifest.audio)
            .into_iter()
            .map(|key| soulframe_audio::AudioAssetKey {
                family: key.family,
                path: key.path,
            })
            .collect();
        soulframe_audio::run_extract_selected(
            soulframe_audio::ExtractArgs {
                cache_path: Some(manifest.cache_path.clone()),
                output: manifest.output.join("audio"),
                ..Default::default()
            },
            &keys,
        )
        .map_err(|e| format!("Selected audio extraction failed: {e:#}"))?;
    }
    if !manifest.models.is_empty() {
        stage += 1;
        println!(
            "# soulkit-cli run-all - stage {stage}/{stages}: selected models ({})",
            manifest.models.len()
        );
        soulframe_models::cli::run_extract_selected(
            soulframe_models::cli::ExtractArgs {
                cache_path: Some(manifest.cache_path),
                output: manifest.output.join("models"),
                ..Default::default()
            },
            &manifest.models,
        )
        .map_err(|e| format!("Selected model extraction failed: {e:#}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (CacheBrowserService, RunSettings) {
        let workspace = std::env::current_dir().unwrap();
        let settings = RunSettings {
            output: workspace.join("output"),
            cache_path: Some(workspace.join("Cache.Windows")),
            workspace: workspace.clone(),
            soulkit: PathBuf::new(),
            dll_directory: None,
            locales: "en".into(),
            keep_intermediates: false,
        };
        let mut entries = Vec::new();
        for (kind, family, source) in [
            (MediaKind::Images, None, "SF/Interface/icon.png"),
            (MediaKind::Images, None, "SF/Interface/portrait.png"),
            (MediaKind::Images, None, "SF/World/tile.dds"),
            (MediaKind::Audio, Some("en"), "SF/Sounds/greeting.wav"),
            (MediaKind::Audio, Some("fr"), "SF/Sounds/greeting.wav"),
            (MediaKind::Models, None, "SF/Objects/Rock.fbx"),
        ] {
            let (id, path) = match (kind, family) {
                (_, Some(family)) => (
                    format!("audio:{family}/{source}"),
                    format!("audio/{family}/{source}"),
                ),
                (MediaKind::Models, _) => (format!("model:{source}"), format!("models/{source}")),
                _ => (format!("image:{source}"), format!("images/{source}")),
            };
            entries.push(CacheEntry {
                id,
                path,
                source_path: source.into(),
                name: source.rsplit('/').next().unwrap().into(),
                kind,
                family: family.map(str::to_owned),
                category: None,
                compressed_bytes: 100,
                decompressed_bytes: 200,
                cache_sets: Vec::new(),
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let by_id = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect();
        let catalog = Catalog {
            generation: 1,
            settings_key: settings_key(&settings),
            cache_path: settings.cache_path.clone().unwrap(),
            summary: summarize(entries.iter()),
            entries,
            by_id,
        };
        (
            CacheBrowserService {
                state: Mutex::new(BrowserState {
                    catalog: Some(Arc::new(catalog)),
                    selected: Arc::default(),
                    summary: SelectionSummary::default(),
                }),
                revision: AtomicU64::new(1),
            },
            settings,
        )
    }

    fn selection(
        action: SelectionAction,
        ids: Option<Vec<String>>,
        folder: &str,
        query: &str,
    ) -> SelectionRequest {
        SelectionRequest {
            generation: 1,
            action,
            ids,
            folder: folder.into(),
            query: query.into(),
            kind: MediaKind::All,
        }
    }

    #[test]
    fn model_filter_selection_and_plan_keep_images_and_audio_out() {
        let (service, settings) = fixture();
        let page = service
            .browse(
                &settings,
                BrowseRequest {
                    query: "Rock".into(),
                    kind: MediaKind::Models,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].kind, "models");
        let selected = service
            .select(
                &settings,
                selection(
                    SelectionAction::Replace,
                    Some(vec![page.entries[0].id.clone()]),
                    "",
                    "",
                ),
            )
            .unwrap();
        assert_eq!(
            (
                selected.count,
                selected.images,
                selected.audio,
                selected.models
            ),
            (1, 0, 0, 1)
        );
        let snapshot = service.snapshot(&settings, 1, false).unwrap();
        for image_format in ["png", "dds"] {
            let manifest = snapshot.manifest(&settings.output, image_format).unwrap();
            assert_eq!(manifest.models, ["SF/Objects/Rock.fbx"]);
            assert!(manifest.images.is_empty());
            assert!(manifest.audio.is_empty());
        }
        let plan = extraction_plan(Path::new("manifest.json"), false, false, true, 1);
        assert_eq!(plan.requested, [TaskId::Models]);
        assert_eq!(plan.modules, [TaskId::Models]);
        assert!(plan.dependencies.is_empty());
        assert_eq!(plan.jobs[0].args, ["cache-extract", "manifest.json"]);
    }

    #[test]
    fn old_image_and_audio_manifests_do_not_require_a_models_field() {
        let manifest: CacheManifest = serde_json::from_value(serde_json::json!({
            "version": 1,
            "cache_path": "cache",
            "output": "output",
            "image_format": "png",
            "images": ["SF/Interface/icon.png"],
            "audio": []
        }))
        .unwrap();
        assert!(manifest.models.is_empty());
        assert_eq!(manifest.images, ["SF/Interface/icon.png"]);
    }

    #[test]
    fn root_browsing_is_virtual_and_pagination_does_not_limit_selection() {
        let (service, settings) = fixture();
        let page = service
            .browse(
                &settings,
                BrowseRequest {
                    limit: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.matching_assets, 6);
        assert_eq!(page.entries.len(), 1);
        assert!(page.entries[0].is_dir);
        let selected = service
            .select(&settings, selection(SelectionAction::Add, None, "", ""))
            .unwrap();
        assert_eq!(
            (
                selected.count,
                selected.images,
                selected.audio,
                selected.models
            ),
            (6, 3, 2, 1)
        );
        assert_eq!(selected.compressed_bytes, 600);
        let (path, entries) = service.selected(&settings, 1, false).unwrap();
        assert_eq!(path, settings.cache_path.unwrap());
        assert_eq!(entries.len(), 6);
    }

    #[test]
    fn recursive_search_selects_all_matches_across_installed_audio_families() {
        let (service, settings) = fixture();
        let page = service
            .browse(
                &settings,
                BrowseRequest {
                    query: "GREETING".into(),
                    limit: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            (page.total, page.matching_assets, page.entries.len()),
            (2, 2, 1)
        );
        assert!(!page.entries[0].is_dir);
        let selected = service
            .select(
                &settings,
                selection(SelectionAction::Add, None, "audio", "greeting"),
            )
            .unwrap();
        assert_eq!(selected.audio, 2);
        assert_eq!(selected.images, 0);
    }

    #[test]
    fn folders_report_partial_selection_and_folder_actions_are_recursive() {
        let (service, settings) = fixture();
        service
            .select(
                &settings,
                selection(
                    SelectionAction::Add,
                    Some(vec!["image:SF/Interface/icon.png".into()]),
                    "",
                    "",
                ),
            )
            .unwrap();
        let page = service
            .browse(
                &settings,
                BrowseRequest {
                    folder: "images/SF".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let folder = page.entries.iter().find(|e| e.name == "Interface").unwrap();
        assert!(folder.partial);
        assert!(!folder.selected);
        assert_eq!(folder.count, 2);
        let summary = service
            .select(
                &settings,
                selection(SelectionAction::Add, Some(vec![folder.id.clone()]), "", ""),
            )
            .unwrap();
        assert_eq!(summary.count, 2);
        let page = service
            .browse(
                &settings,
                BrowseRequest {
                    folder: "images/SF".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            page.entries
                .iter()
                .find(|e| e.name == "Interface")
                .unwrap()
                .selected
        );
        let removed = service
            .select(
                &settings,
                selection(
                    SelectionAction::Remove,
                    Some(vec![folder.id.clone()]),
                    "",
                    "",
                ),
            )
            .unwrap();
        assert_eq!(removed.count, 0);
    }

    #[test]
    fn invalid_selection_is_atomic_and_stale_settings_cannot_extract() {
        let (service, mut settings) = fixture();
        assert!(service
            .select(
                &settings,
                selection(
                    SelectionAction::Add,
                    Some(vec!["image:SF/Interface/icon.png".into(), "missing".into()]),
                    "",
                    ""
                )
            )
            .is_err());
        assert!(service.selected(&settings, 1, false).is_err());
        settings.cache_path = Some(settings.workspace.join("other-cache"));
        assert!(service
            .selected(&settings, 1, true)
            .unwrap_err()
            .contains("changed"));
        assert!(service
            .lookup(&settings, 1, "image:SF/Interface/icon.png")
            .is_err());
        service.invalidate();
        assert!(service.state.lock().unwrap().selected.is_empty());
    }

    #[test]
    fn manifest_keeps_exact_keys_and_command_line_stays_constant_size() {
        let (service, settings) = fixture();
        let (cache, entries) = service.selected(&settings, 1, true).unwrap();
        let manifest = manifest_for_entries(&cache, &settings.output, &entries, "png").unwrap();
        assert_eq!(manifest.images.len(), 3);
        assert_eq!(manifest.audio.len(), 2);
        assert_eq!(manifest.models, ["SF/Objects/Rock.fbx"]);
        assert_ne!(manifest.audio[0].family, manifest.audio[1].family);
        let plan = extraction_plan(Path::new("manifest.json"), true, true, true, 6);
        assert_eq!(
            plan.modules,
            [TaskId::Images, TaskId::Audio, TaskId::Models]
        );
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.jobs[0].args, ["cache-extract", "manifest.json"]);
        assert!(manifest_for_entries(&cache, &settings.output, &[], "png").is_err());
        assert!(manifest_for_entries(&cache, &settings.output, &entries, "jpeg").is_err());
    }

    #[test]
    fn extraction_snapshot_keeps_the_selection_at_start_while_the_browser_changes() {
        let (service, settings) = fixture();
        service
            .select(
                &settings,
                selection(
                    SelectionAction::Add,
                    Some(vec!["image:SF/Interface/icon.png".into()]),
                    "",
                    "",
                ),
            )
            .unwrap();
        let snapshot = service.snapshot(&settings, 1, false).unwrap();
        service
            .select(
                &settings,
                selection(SelectionAction::Replace, None, "audio", ""),
            )
            .unwrap();
        let manifest = snapshot.manifest(&settings.output, "png").unwrap();
        assert_eq!(manifest.images, ["SF/Interface/icon.png"]);
        assert!(manifest.audio.is_empty());
        assert_eq!(snapshot.summary.count, 1);
        assert_eq!(
            service.snapshot(&settings, 1, false).unwrap().summary.audio,
            2
        );
        service.invalidate();
        assert!(service.snapshot(&settings, 1, true).is_err());
    }

    #[test]
    fn extraction_validates_saved_settings_before_resolving_runtime_path_variants() {
        for relative_workspace in [false, true] {
            for cache_kind in 0..3 {
                let (service, mut settings) = fixture();
                let catalog_cache = settings.cache_path.clone().unwrap();
                if relative_workspace {
                    settings.workspace = settings.workspace.join(".");
                }
                settings.cache_path = match cache_kind {
                    0 => None, // Installation was auto-detected during the scan.
                    1 => Some(PathBuf::from("Cache.Windows")),
                    _ => Some(catalog_cache.clone()),
                };
                {
                    let mut state = service.state.lock().unwrap();
                    Arc::get_mut(state.catalog.as_mut().unwrap())
                        .unwrap()
                        .settings_key = settings_key(&settings);
                }
                // Changing only the destination keeps catalog IDs and selections valid.
                settings.output = settings.workspace.join("another-output");
                let (resolved, snapshot) = service.prepare_extraction(&settings, 1, true).unwrap();
                assert_eq!(
                    resolved.workspace,
                    std::fs::canonicalize(&settings.workspace).unwrap()
                );
                assert_eq!(resolved.cache_path, Some(catalog_cache));
                assert_eq!(snapshot.summary.count, 6);
                let manifest = snapshot.manifest(&resolved.output, "png").unwrap();
                assert_eq!(
                    (
                        manifest.images.len(),
                        manifest.audio.len(),
                        manifest.models.len()
                    ),
                    (3, 2, 1)
                );
            }
        }
    }

    #[test]
    fn asset_paths_cannot_escape_output_and_workers_reject_bad_manifests_before_payloads() {
        for path in [
            "../secret",
            "a/../secret",
            "C:/secret",
            "/secret",
            "a//b",
            "a\\..\\b",
            "a/./b",
        ] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        let root = std::env::temp_dir().join(format!(
            "soulkit-manifest-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let manifest_path = root.join("bad.json");
        let mut manifest = CacheManifest {
            version: 1,
            cache_path: root.clone(),
            output: root.join("output"),
            image_format: "png".into(),
            images: vec!["../escape.png".into()],
            audio: vec![],
            models: vec!["SF/../../escape.fbx".into()],
        };
        write_manifest_at(&manifest_path, &manifest).unwrap();
        assert!(run_manifest(&manifest_path).unwrap_err().contains("unsafe"));
        assert!(!root.join("output").exists());
        std::fs::remove_file(&manifest_path).unwrap();
        // Validate a model-only traversal as well, so the earlier invalid
        // image path cannot conceal a missing model path check.
        manifest.images.clear();
        write_manifest_at(&manifest_path, &manifest).unwrap();
        assert!(run_manifest(&manifest_path).unwrap_err().contains("unsafe"));
        assert!(!root.join("output").exists());
        std::fs::remove_file(manifest_path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn owned_work_cleanup_removes_only_its_manifest_and_preserves_unknown_files() {
        let root = std::env::temp_dir().join(format!(
            "soulkit-owned-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let owned = OwnedManifest::planned(&root);
        assert!(!owned.directory.exists());
        owned.cleanup().unwrap();
        owned.create(&root, &root.join("output")).unwrap();
        std::fs::write(&owned.path, "{}").unwrap();
        let unknown = owned.directory.join("keep.txt");
        std::fs::write(&unknown, "keep").unwrap();
        assert!(owned.cleanup().is_err());
        assert!(unknown.exists());
        assert!(!owned.path.exists());
        std::fs::remove_file(unknown).unwrap();
        owned.cleanup().unwrap();
        std::fs::remove_dir(root.join(".soulkit-work")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
