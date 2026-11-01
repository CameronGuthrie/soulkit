use crate::{decode_static, is_static_name, to_glb, TYPE_STATIC};
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use soulframe_cache::{detect_soulframe_cache, parse_common_header, LazyCache, Oodle, TocEntry};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Export installed static meshes as glTF binary (.glb).
    Extract {
        /// Override the installed Cache.Windows directory.
        #[arg(long, alias = "cache-dir")]
        cache_path: Option<PathBuf>,
        /// Destination folder for .glb files.
        #[arg(short, long, default_value = "output/models")]
        output: PathBuf,
        /// Only paths containing this text (case insensitive).
        #[arg(long)]
        path_filter: Option<String>,
        /// Maximum number of meshes to export (0 = unlimited).
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// List matching paths without extracting.
        #[arg(long)]
        list_paths: bool,
    },
    /// Decode meshes without writing files and report coverage per header type.
    Scan {
        #[arg(long, alias = "cache-dir")]
        cache_path: Option<PathBuf>,
        #[arg(long)]
        path_filter: Option<String>,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Extract {
            cache_path,
            output,
            path_filter,
            limit,
            list_paths,
        } => run_extract(ExtractArgs {
            cache_path,
            output,
            path_filter,
            limit,
            list_paths,
        }),
        Cmd::Scan {
            cache_path,
            path_filter,
        } => run_scan(cache_path, path_filter),
    }
}

#[derive(Debug, Default)]
pub struct ExtractArgs {
    pub cache_path: Option<PathBuf>,
    pub output: PathBuf,
    pub path_filter: Option<String>,
    pub limit: usize,
    pub list_paths: bool,
}

#[derive(Debug, Clone)]
pub struct ModelAssetInfo {
    pub path: String,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
}

/// The H, B and F mesh caches opened together.
struct Caches {
    h: LazyCache,
    b: LazyCache,
    f: Option<LazyCache>,
}

fn cache_dir(cache_path: Option<PathBuf>) -> Result<PathBuf> {
    cache_path
        .or_else(detect_soulframe_cache)
        .context("Soulframe cache directory not found; pass --cache-path")
}

fn open(cache_path: &Path) -> Result<Caches> {
    let missing = |name: &str| format!("{name} not found in {}", cache_path.display());
    Ok(Caches {
        h: LazyCache::open(cache_path, "H.Misc")?.with_context(|| missing("H.Misc"))?,
        b: LazyCache::open(cache_path, "B.Misc")?.with_context(|| missing("B.Misc"))?,
        f: LazyCache::open(cache_path, "F.Misc")?,
    })
}

fn by_path(cache: &LazyCache) -> HashMap<&str, &TocEntry> {
    cache
        .entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| (e.full_path.as_str(), e))
        .collect()
}

/// Static mesh candidates by name, with a body in B.Misc.
fn candidates(caches: &Caches) -> Vec<&TocEntry> {
    let b = by_path(&caches.b);
    caches
        .h
        .entries
        .iter()
        .filter(|e| {
            !e.is_dir && is_static_name(&e.full_path) && b.contains_key(e.full_path.as_str())
        })
        .collect()
}

/// Static meshes in the installed cache, for the Soulkit cache browser.
/// Reads only TOCs, so it needs no Oodle or other native code.
/// A cache without mesh TOCs lists no models instead of failing the browser.
pub fn list_assets(cache_path: &Path) -> Result<Vec<ModelAssetInfo>> {
    let (Some(h), Some(b)) = (
        LazyCache::open(cache_path, "H.Misc")?,
        LazyCache::open(cache_path, "B.Misc")?,
    ) else {
        return Ok(Vec::new());
    };
    let caches = Caches {
        h,
        b,
        f: LazyCache::open(cache_path, "F.Misc")?,
    };
    let (b, f) = (by_path(&caches.b), caches.f.as_ref().map(by_path));
    let mut result = Vec::new();
    for h in candidates(&caches) {
        soulframe_images::cli::safe_relative_path(&h.full_path)
            .with_context(|| format!("Unsafe mesh cache entry {}", h.full_path))?;
        let path = h.full_path.as_str();
        let parts = [
            Some(h),
            b.get(path).copied(),
            f.as_ref().and_then(|f| f.get(path).copied()),
        ];
        let parts = parts.iter().flatten();
        result.push(ModelAssetInfo {
            path: h.full_path.clone(),
            compressed_bytes: parts.clone().map(|e| u64::from(e.compressed_size)).sum(),
            decompressed_bytes: parts.map(|e| u64::from(e.decompressed_size)).sum(),
        });
    }
    result.sort_by_cached_key(|a| a.path.to_lowercase());
    Ok(result)
}

fn output_name(path: &str) -> Result<PathBuf> {
    let relative = soulframe_images::cli::safe_relative_path(path)?;
    Ok(relative.with_extension("glb"))
}

/// Decode one mesh to GLB bytes.
fn export_one(
    caches: &Caches,
    oodle: &Oodle,
    b: &HashMap<&str, &TocEntry>,
    f: Option<&HashMap<&str, &TocEntry>>,
    h: &TocEntry,
) -> Result<(Vec<u8>, crate::Mesh)> {
    let path = h.full_path.as_str();
    let h_bytes = caches.h.get(oodle, h)?;
    let header = parse_common_header(&h_bytes)?;
    if header.type_id != TYPE_STATIC {
        bail!("Unsupported mesh type {}", header.type_id);
    }
    let b_entry = b.get(path).context("Missing B.Misc body")?;
    let b_bytes = caches.b.get(oodle, b_entry)?;
    let f_bytes = match (caches.f.as_ref(), f.and_then(|f| f.get(path))) {
        (Some(cache), Some(entry)) => Some(cache.get(oodle, entry)?),
        _ => None,
    };
    let mesh = decode_static(path, &header, &h_bytes, &b_bytes, f_bytes.as_deref())?;
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    Ok((to_glb(&mesh, name)?, mesh))
}

pub fn run_extract(args: ExtractArgs) -> Result<()> {
    run_extract_inner(args, None)
}

/// Export an exact selection. An empty selection never becomes a full export.
pub fn run_extract_selected(mut args: ExtractArgs, paths: &[String]) -> Result<()> {
    ensure!(!paths.is_empty(), "No meshes selected");
    for path in paths {
        output_name(path)?;
    }
    args.path_filter = None;
    args.limit = 0;
    args.list_paths = false;
    run_extract_inner(args, Some(&paths.iter().map(String::as_str).collect()))
}

fn run_extract_inner(args: ExtractArgs, selected: Option<&HashSet<&str>>) -> Result<()> {
    let cache_path = cache_dir(args.cache_path)?;
    let caches = open(&cache_path)?;
    let filter = args.path_filter.map(|f| f.to_lowercase());
    let mut todo: Vec<_> = candidates(&caches)
        .into_iter()
        .filter(|e| selected.is_none_or(|s| s.contains(e.full_path.as_str())))
        .filter(|e| {
            filter
                .as_ref()
                .is_none_or(|f| e.full_path.to_lowercase().contains(f))
        })
        .collect();
    if let Some(selected) = selected {
        let found: HashSet<_> = todo.iter().map(|e| e.full_path.as_str()).collect();
        let missing: Vec<_> = selected.difference(&found).collect();
        ensure!(
            missing.is_empty(),
            "Selected meshes are no longer in the cache: {missing:?}"
        );
    }
    if args.limit > 0 {
        todo.truncate(args.limit);
    }
    if args.list_paths {
        for e in &todo {
            println!("{}", e.full_path);
        }
        return Ok(());
    }

    let oodle = Oodle::new()?;
    let (b, f) = (by_path(&caches.b), caches.f.as_ref().map(by_path));
    let (mut written, mut skipped) = (0usize, 0usize);
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    for (i, h) in todo.iter().enumerate() {
        match export_one(&caches, &oodle, &b, f.as_ref(), h) {
            Ok((glb, _)) => {
                let out = args.output.join(output_name(&h.full_path)?);
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("Cannot create {}", parent.display()))?;
                }
                std::fs::write(&out, glb)
                    .with_context(|| format!("Cannot write {}", out.display()))?;
                written += 1;
            }
            Err(e) if e.to_string().starts_with("Unsupported mesh type") && selected.is_none() => {
                skipped += 1;
            }
            Err(e) => {
                eprintln!("  failed {}: {e:#}", h.full_path);
                *failures.entry(format!("{e:#}")).or_default() += 1;
            }
        }
        if (i + 1) % 500 == 0 {
            println!("  {}/{} processed", i + 1, todo.len());
        }
    }
    let failed: usize = failures.values().sum();
    println!(
        "Models: {written} written, {failed} failed, {skipped} other .fbx types skipped -> {}",
        args.output.display()
    );
    ensure!(
        written > 0 || todo.is_empty(),
        "No meshes could be exported"
    );
    Ok(())
}

/// Decode every candidate in memory and report results by type and error.
fn run_scan(cache_path: Option<PathBuf>, filter: Option<String>) -> Result<()> {
    let cache_path = cache_dir(cache_path)?;
    let caches = open(&cache_path)?;
    let oodle = Oodle::new()?;
    let (b, f) = (by_path(&caches.b), caches.f.as_ref().map(by_path));
    let filter = filter.map(|f| f.to_lowercase());
    let mut types: BTreeMap<u32, usize> = BTreeMap::new();
    let mut errors: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut lods: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut ok, mut with_f, mut vertices, mut triangles) = (0usize, 0usize, 0usize, 0usize);
    let mut most = (0usize, String::new());
    for h in candidates(&caches) {
        if filter
            .as_ref()
            .is_some_and(|f| !h.full_path.to_lowercase().contains(f))
        {
            continue;
        }
        let type_id = caches
            .h
            .get(&oodle, h)
            .ok()
            .and_then(|bytes| parse_common_header(&bytes).ok())
            .map_or(u32::MAX, |c| c.type_id);
        *types.entry(type_id).or_default() += 1;
        if type_id != TYPE_STATIC {
            continue;
        }
        match export_one(&caches, &oodle, &b, f.as_ref(), h) {
            Ok((_, mesh)) => {
                ok += 1;
                with_f += usize::from(
                    f.as_ref()
                        .is_some_and(|f| f.contains_key(h.full_path.as_str())),
                );
                vertices += mesh.positions.len();
                if mesh.positions.len() > most.0 {
                    most = (mesh.positions.len(), h.full_path.clone());
                }
                for part in &mesh.parts {
                    *lods.entry(part.lod).or_default() += 1;
                    triangles += part.count / 3;
                }
            }
            Err(e) => {
                let slot = errors
                    .entry(format!("{e:#}"))
                    .or_insert((0, h.full_path.clone()));
                slot.0 += 1;
            }
        }
    }
    println!("Header types among .fbx candidates: {types:?}");
    let failed: usize = errors.values().map(|(n, _)| n).sum();
    println!(
        "Static (type {TYPE_STATIC}): {ok} decoded, {failed} failed; {with_f} used F.Misc detail"
    );
    println!("Exported LOD per part: {lods:?}; {vertices} vertices, {triangles} triangles");
    println!("Largest mesh: {} vertices ({})", most.0, most.1);
    for (error, (count, example)) in &errors {
        println!("  {count:>6}  {error}  (e.g. {example})");
    }
    Ok(())
}
