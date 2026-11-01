//! CLI subcommands for `soulframe-images`. Owns the cache walking, H/B
//! pairing, output-format selection and disk write logic.

use anyhow::{anyhow, ensure, Context, Result};
use clap::Subcommand;
use soulframe_cache::{detect_soulframe_cache, LazyCache, Oodle};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::{
    dds_deswizzle, is_image_file, texture_export_dds, texture_export_selected_dds, ImageType,
};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Extract image / texture files from the local Soulframe cache.
    Extract {
        /// Override Soulframe cache directory.
        #[arg(long)]
        cache_path: Option<PathBuf>,
        /// Output directory (subdirectories preserve the in-cache structure unless `--flat`).
        #[arg(short, long, default_value = "output")]
        output: PathBuf,
        /// Filter by image extension (e.g. `png`, `jpg`, `dds`).
        #[arg(long)]
        filter: Option<String>,
        /// Filter by path prefix (e.g. `SF/Interface/`).
        #[arg(long)]
        path_filter: Option<String>,
        /// Cap the number of images extracted (0 = unlimited).
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// List all discovered cache sets and exit.
        #[arg(long)]
        list_caches: bool,
        /// List matching paths (after filters) and exit, without extracting.
        #[arg(long)]
        list_paths: bool,
        /// Include interface SWF assets as well as recognized image formats.
        #[arg(long, default_value_t = false)]
        list_all: bool,
        /// Treat `--path-filter` as substring match anywhere in the path.
        #[arg(long, default_value_t = false)]
        path_filter_substring: bool,
        /// Write all files into a single flat directory.
        #[arg(long, default_value_t = false)]
        flat: bool,
        /// Apply Morton deswizzle to `Texture` cache data (never applied to `LightMap`).
        #[arg(long, default_value_t = false)]
        deswizzle_textures: bool,
        /// Halve DDS header dimensions until they match the body size (helps PNG conversion).
        /// Always on when `--output-format png` is selected.
        #[arg(long, default_value_t = false)]
        fix_dims: bool,
        /// Output format: `dds` (raw reconstruction) or `png` (decoded RGBA).
        #[arg(long, default_value = "dds")]
        output_format: String,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Extract {
            cache_path,
            output,
            filter,
            path_filter,
            limit,
            list_caches,
            list_paths,
            list_all,
            path_filter_substring,
            flat,
            deswizzle_textures,
            fix_dims,
            output_format,
        } => run_extract(ExtractArgs {
            cache_path,
            output,
            filter,
            path_filter,
            limit,
            list_caches,
            list_paths,
            list_all,
            path_filter_substring,
            flat,
            deswizzle_textures,
            fix_dims,
            output_format,
        }),
    }
}

#[derive(Debug)]
pub struct ExtractArgs {
    pub cache_path: Option<PathBuf>,
    pub output: PathBuf,
    pub filter: Option<String>,
    pub path_filter: Option<String>,
    pub limit: usize,
    pub list_caches: bool,
    pub list_paths: bool,
    pub list_all: bool,
    pub path_filter_substring: bool,
    pub flat: bool,
    pub deswizzle_textures: bool,
    pub fix_dims: bool,
    pub output_format: String,
}

/// One logical image, with its newest record from each contributing cache set.
/// These sizes describe cache data; the exported image size can differ.
#[derive(Clone, Debug)]
pub struct ImageAssetInfo {
    pub path: String,
    pub compressed_bytes: u64,
    pub decompressed_bytes: u64,
    pub cache_sets: Vec<String>,
}

fn is_native_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".hdr") || lower.ends_with(".exr")
}

fn path_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

type ImageLocations = HashMap<String, Vec<(String, usize)>>;

fn image_locations(caches: &[(String, LazyCache)], include_swfs: bool) -> ImageLocations {
    let mut images: HashMap<String, (String, Vec<(String, usize)>)> = HashMap::new();
    for (cache_name, cache) in caches {
        let mut newest = HashMap::new();
        for (index, entry) in cache.entries.iter().enumerate() {
            // This is a media extractor, including when --list-all is used by
            // the interface pipeline. Never turn it into an arbitrary cache
            // exporter for scripts or other non-image resources.
            if entry.is_dir
                || !(is_image_file(&entry.full_path)
                    || (include_swfs && entry.full_path.to_ascii_lowercase().ends_with(".swf")))
            {
                continue;
            }
            let key = path_key(&entry.full_path);
            let rank = (entry.timestamp, entry.index);
            if newest
                .get(&key)
                .is_some_and(|&(old_rank, _)| old_rank > rank)
            {
                continue;
            }
            newest.insert(key, (rank, index));
        }
        for (key, (_, index)) in newest {
            let image = images.entry(key).or_insert_with(|| {
                (
                    cache.entries[index].full_path.replace('\\', "/"),
                    Vec::new(),
                )
            });
            image.1.push((cache_name.clone(), index + 1));
        }
    }
    images.into_values().collect()
}

fn select_images(images: &mut ImageLocations, selected: &HashSet<String>) -> Result<()> {
    images.retain(|path, _| selected.contains(&path_key(path)));
    let found: HashSet<_> = images.keys().map(|path| path_key(path)).collect();
    let mut missing: Vec<_> = selected.difference(&found).collect();
    missing.sort();
    ensure!(
        missing.is_empty(),
        "{} selected image asset(s) are no longer in the cache (first: {})",
        missing.len(),
        missing.first().map(|p| p.as_str()).unwrap_or("")
    );
    for path in images.keys() {
        safe_relative_path(path)?;
    }
    Ok(())
}

/// Inspect TOCs across every cache group without loading/decompressing payloads.
pub fn list_assets(cache_path: &Path) -> Result<Vec<ImageAssetInfo>> {
    let mut caches = Vec::new();
    for name in discover_cache_sets(cache_path)? {
        if let Some(cache) = LazyCache::open(cache_path, &name)? {
            caches.push((name, cache));
        }
    }
    let by_name: HashMap<_, _> = caches.iter().map(|(name, cache)| (name, cache)).collect();
    let mut result = Vec::new();
    for (path, locations) in image_locations(&caches, false) {
        // An unsafe archive path cannot be exported. Do not offer it as a
        // selectable asset that could later escape the chosen output folder.
        safe_relative_path(&path).with_context(|| format!("Unsafe image cache entry {path}"))?;
        let mut info = ImageAssetInfo {
            path,
            compressed_bytes: 0,
            decompressed_bytes: 0,
            cache_sets: Vec::with_capacity(locations.len()),
        };
        for (name, index) in locations {
            let entry = &by_name[&name].entries[index - 1];
            info.compressed_bytes += u64::from(entry.compressed_size);
            info.decompressed_bytes += u64::from(entry.decompressed_size);
            info.cache_sets.push(name);
        }
        info.cache_sets.sort();
        result.push(info);
    }
    result.sort_by(|a, b| path_key(&a.path).cmp(&path_key(&b.path)));
    Ok(result)
}

/// Extract only the exact logical paths selected in the cache browser.
/// Empty or stale selections are errors, never an implicit full export.
pub fn run_extract_selected(mut args: ExtractArgs, paths: &[String]) -> Result<()> {
    ensure!(!paths.is_empty(), "No image assets selected");
    let mut selected = HashSet::new();
    for path in paths {
        safe_relative_path(path)?;
        selected.insert(path_key(path));
    }
    args.filter = None;
    args.path_filter = None;
    args.limit = 0;
    args.list_caches = false;
    args.list_paths = false;
    args.list_all = false;
    run_extract_inner(args, Some(&selected))
}

pub fn run_extract(args: ExtractArgs) -> Result<()> {
    run_extract_inner(args, None)
}

fn run_extract_inner(args: ExtractArgs, selected: Option<&HashSet<String>>) -> Result<()> {
    println!("{}", "=".repeat(70));
    println!("Soulframe Image Extractor");
    println!("{}", "=".repeat(70));
    println!();

    let cache_path = match args.cache_path {
        Some(p) => {
            if !p.exists() || !p.is_dir() {
                return Err(anyhow!(
                    "Custom cache path does not exist or is not a directory: {:?}",
                    p
                ));
            }
            p
        }
        None => detect_soulframe_cache().ok_or_else(|| {
            anyhow!("Soulframe cache not found at default location. Pass --cache-path to override.")
        })?,
    };

    println!("✓ Found Soulframe cache at:\n  {:?}\n", cache_path);

    let cache_sets = discover_cache_sets(&cache_path)?;
    if cache_sets.is_empty() {
        ensure!(
            selected.is_none(),
            "No image cache files found in {}",
            cache_path.display()
        );
        println!("No cache files found.");
        return Ok(());
    }
    println!("Found {} cache set(s):", cache_sets.len());
    for cs in &cache_sets {
        println!("  • {}", cs);
    }
    println!();

    if args.list_caches {
        return Ok(());
    }

    println!("Loading cache indexes...");
    let mut all_caches: Vec<(String, LazyCache)> = Vec::new();
    for cs in &cache_sets {
        match LazyCache::open(&cache_path, cs) {
            Ok(Some(data)) => {
                println!(
                    "  ✓ {}: {} entries, {} bytes",
                    cs,
                    data.entries.len(),
                    data.cache_len()
                );
                all_caches.push((cs.clone(), data));
            }
            Ok(None) => {}
            Err(e) => println!("  ✗ {}: {}", cs, e),
        }
    }
    if all_caches.is_empty() {
        ensure!(
            selected.is_none(),
            "No readable image cache data in {}",
            cache_path.display()
        );
        println!("No cache data could be loaded.");
        return Ok(());
    }
    println!();

    println!("Scanning for image files...");
    // Borrow each entry's path through the filters and only allocate for the
    // ones that survive — most of a 100k-entry TOC is rejected here.
    let needle = args
        .filter
        .as_ref()
        .map(|ext| format!(".{}", ext.to_lowercase()));
    let mut images = image_locations(&all_caches, args.list_all);
    if let Some(selected) = selected {
        select_images(&mut images, selected)?;
    }
    images.retain(|path, _| {
        // Legacy image commands retain their supported-format inventory.
        // The cache browser additionally offers unsupported HDR/EXR originals.
        if selected.is_none() && !args.list_all && is_native_image(path) {
            return false;
        }
        if let Some(ref needle) = needle {
            if !path.to_lowercase().ends_with(needle.as_str()) {
                return false;
            }
        }
        if let Some(ref pf) = args.path_filter {
            let matched = if args.path_filter_substring {
                path.contains(pf.as_str())
            } else {
                path.starts_with(pf)
            };
            if !matched {
                return false;
            }
        }
        true
    });

    let total_images = images.len();
    let images_to_extract = if args.limit > 0 {
        args.limit.min(total_images)
    } else {
        total_images
    };

    println!("Found {} image file(s)", total_images);
    if let Some(ref f) = args.filter {
        println!("  (filtered by extension: .{})", f);
    }
    if let Some(ref pf) = args.path_filter {
        println!("  (filtered by path: {}*)", pf);
    }
    if args.limit > 0 && args.limit < total_images {
        println!("  (limiting extraction to {} images)", args.limit);
    }

    if args.list_paths {
        let mut paths: Vec<&String> = images.keys().collect();
        paths.sort();
        for p in paths {
            println!("{}", p);
        }
        return Ok(());
    }

    if images_to_extract == 0 {
        println!("No images to extract.");
        return Ok(());
    }

    let oodle =
        Oodle::new().map_err(|e| anyhow!("Failed to initialize Oodle decompression: {}", e))?;

    let mut by_ext: HashMap<String, usize> = HashMap::new();
    for path in images.keys() {
        let ext = path.rsplit('.').next().unwrap_or("unknown").to_lowercase();
        *by_ext.entry(ext).or_insert(0) += 1;
    }
    println!("\nBreakdown by extension:");
    let mut exts: Vec<_> = by_ext.iter().collect();
    exts.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (ext, count) in exts {
        println!("  .{}: {} files", ext, count);
    }

    println!("\nExtracting images...");
    fs::create_dir_all(&args.output).with_context(|| {
        format!(
            "Cannot create image output folder {}",
            args.output.display()
        )
    })?;

    let mut extracted = 0usize;
    let mut native = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut first_error = None;

    let cache_map: HashMap<String, &LazyCache> = all_caches
        .iter()
        .map(|(name, data)| (name.clone(), data))
        .collect();

    let mut image_paths: Vec<_> = images.iter().collect();
    image_paths.sort_by(|(a, _), (b, _)| path_key(a).cmp(&path_key(b)));
    for (path, cache_locations) in image_paths.into_iter().take(images_to_extract) {
        if selected.is_some() && is_native_image(path) {
            let parts = save_native_image(&oodle, cache_locations, &cache_map, &args.output, path)?;
            println!("  [images] Native data saved for {path} ({parts} cache parts); HDR/EXR decoding and preview are unavailable");
            native += 1;
            extracted += 1;
            continue;
        }
        let file_name = path.split('/').next_back().unwrap_or(path);
        let img_type = ImageType::from_extension(path);
        let selected_data = if selected.is_some() {
            match read_selected_image(
                &oodle,
                cache_locations,
                &cache_map,
                path,
                args.deswizzle_textures,
            ) {
                Ok(data) => Some(data),
                Err(error) => {
                    let parts =
                        save_native_image(&oodle, cache_locations, &cache_map, &args.output, path)?;
                    println!("  [images] Native data saved for {path} ({parts} cache parts); decoding unavailable: {error:#}");
                    native += 1;
                    extracted += 1;
                    continue;
                }
            }
        } else {
            None
        };

        let classify_cache_group = |cache_name: &str| -> &str {
            if cache_name.contains(".Texture") {
                "Texture"
            } else if cache_name.contains(".LightMap") {
                "LightMap"
            } else {
                "Other"
            }
        };

        let output_data: Option<Vec<u8>> = if let Some(data) = selected_data {
            Some(data)
        } else if img_type == ImageType::Png || img_type == ImageType::Jpg {
            // TGA/BMP source names can also be proprietary DDS-backed textures.
            // Selected exports cover every image family with H+B/F pairing.
            let header_data = pull_first(&oodle, cache_locations, &cache_map, path, |c| {
                c.starts_with('H')
            });

            let (mut body_data, mut body_group) = (None, None);
            for prefix in ['F', 'B'] {
                if body_data.is_some() {
                    break;
                }
                for (cache_name, entry_index) in cache_locations {
                    if !cache_name.starts_with(prefix) {
                        continue;
                    }
                    if let Some(cache) = cache_map.get(cache_name) {
                        if let Some(entry) = cache.entries.get(entry_index - 1).filter(|entry| {
                            !entry.is_dir && path_key(&entry.full_path) == path_key(path)
                        }) {
                            if let Ok(body) = cache.get(&oodle, entry) {
                                if !body.is_empty() {
                                    body_group = Some(classify_cache_group(cache_name).to_string());
                                    body_data = Some(body);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            let want_png = args.output_format.eq_ignore_ascii_case("png");
            let fix_dims = args.fix_dims || want_png;

            if let (Some(header), Some(body)) = (header_data, body_data) {
                match texture_export_dds(&header, &body, fix_dims) {
                    Ok(mut dds) => {
                        if args.deswizzle_textures {
                            if let Some(g) = &body_group {
                                if g == "Texture" {
                                    if let Ok(de) = dds_deswizzle(&dds) {
                                        dds = de;
                                    }
                                }
                            }
                        }
                        Some(dds)
                    }
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            // .dds and other raw files: prefer the full (F) cache payload.
            // Body (B) SWFs can be reduced streaming variants whose embedded
            // DefineBits images are only tiny placeholders.
            let mut data: Option<Vec<u8>> = None;
            let mut source_group: Option<String> = None;
            for prefix in ['F', 'B', 'H'] {
                if data.is_some() {
                    break;
                }
                for (cache_name, entry_index) in cache_locations {
                    if !cache_name.starts_with(prefix) {
                        continue;
                    }
                    if let Some(cache) = cache_map.get(cache_name) {
                        if let Some(entry) = cache.entries.get(entry_index - 1).filter(|entry| {
                            !entry.is_dir && path_key(&entry.full_path) == path_key(path)
                        }) {
                            if let Ok(bytes) = cache.get(&oodle, entry) {
                                if !bytes.is_empty() {
                                    source_group =
                                        Some(classify_cache_group(cache_name).to_string());
                                    data = Some(bytes);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            data.map(|bytes| {
                if args.deswizzle_textures && img_type == ImageType::Dds {
                    if let Some(g) = &source_group {
                        if g == "Texture" {
                            if let Ok(de) = dds_deswizzle(&bytes) {
                                return de;
                            }
                        }
                    }
                }
                bytes
            })
        };

        match output_data {
            Some(out) => match write_output_inner(
                args.flat,
                &args.output,
                path,
                &img_type,
                out,
                file_name,
                &mut extracted,
                &args.output_format,
                selected.is_some(),
            ) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("  ✗ {} - failed to write: {e:#}", file_name);
                    errors += 1;
                    // A full disk or unwritable folder affects the rest of the
                    // export too. Stop before producing thousands of failures.
                    if e.chain().any(|cause| cause.is::<std::io::Error>()) {
                        return Err(e.context(format!(
                            "Image extraction stopped while writing to {}",
                            args.output.display()
                        )));
                    }
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            },
            None => {
                eprintln!("  ⚠ {} - no usable header/body data", path);
                skipped += 1;
            }
        }
    }

    println!();
    println!("{}", "=".repeat(70));
    println!("Extraction complete:");
    println!("  {} successfully extracted", extracted);
    if native > 0 {
        println!("  {native} preserved as native cache data (decoding unavailable)");
    }
    println!("  {} skipped (no data)", skipped);
    println!("  {} errors", errors);
    println!("\nOutput directory: {:?}", args.output);
    println!("{}", "=".repeat(70));

    if let Some(error) = first_error {
        return Err(error.context(format!("Image extraction failed for {errors} file(s)")));
    }
    ensure!(selected.is_none() || skipped == 0, "{skipped} selected image asset(s) could not be read or reconstructed; no usable header/body data. Check the installed cache and native libraries.");
    Ok(())
}

fn image_cache_family(name: &str) -> &str {
    match name.split_once('.') {
        Some(("H" | "B" | "F", family)) => family,
        _ => name,
    }
}

fn read_selected_image(
    oodle: &Oodle,
    locations: &[(String, usize)],
    cache_map: &HashMap<String, &LazyCache>,
    path: &str,
    deswizzle: bool,
) -> Result<Vec<u8>> {
    let mut groups: BTreeMap<&str, Vec<&(String, usize)>> = BTreeMap::new();
    for location in locations {
        groups
            .entry(image_cache_family(&location.0))
            .or_default()
            .push(location);
    }
    let mut last_error = anyhow!("No compatible image header/body pair for {path}");
    for (family, locations) in groups {
        let read = |role: char| -> Result<Option<Vec<u8>>> {
            locations
                .iter()
                .find(|(name, _)| name.starts_with(role))
                .map(|(name, index)| {
                    let cache = cache_map
                        .get(name)
                        .with_context(|| format!("Missing image cache {name}"))?;
                    let entry = cache
                        .entries
                        .get(index - 1)
                        .with_context(|| format!("Missing image cache record {name}:{index}"))?;
                    cache
                        .get(oodle, entry)
                        .with_context(|| format!("Cannot read {path} from {name}:{index}"))
                })
                .transpose()
        };
        let header = read('H')?;
        let body = read('B')?;
        let stream = read('F')?;
        for bytes in [stream.as_deref(), body.as_deref(), header.as_deref()]
            .into_iter()
            .flatten()
        {
            if bytes.starts_with(b"DDS ") {
                return Ok(bytes.to_vec());
            }
        }
        let Some(header) = header.as_deref() else {
            continue;
        };
        match texture_export_selected_dds(header, body.as_deref(), stream.as_deref()) {
            Ok(mut dds) => {
                if deswizzle && family.starts_with("Texture") {
                    dds = dds_deswizzle(&dds)?;
                }
                return Ok(dds);
            }
            Err(error) => last_error = error.context(format!("Image cache group {family}")),
        }
    }
    Err(last_error)
}

fn save_native_image(
    oodle: &Oodle,
    locations: &[(String, usize)],
    cache_map: &HashMap<String, &LazyCache>,
    output: &Path,
    path: &str,
) -> Result<usize> {
    let mut written = 0;
    for (name, index) in locations {
        let cache = cache_map
            .get(name)
            .with_context(|| format!("Missing image cache set {name}"))?;
        let entry = cache
            .entries
            .get(index - 1)
            .with_context(|| format!("Missing image cache record {name}:{index}"))?;
        let bytes = cache
            .get(oodle, entry)
            .with_context(|| format!("Cannot read native image {path} from {name}:{index}"))?;
        let destination = output.join(native_image_path(path, name)?);
        fs::create_dir_all(
            destination
                .parent()
                .context("Missing native image directory")?,
        )
        .with_context(|| {
            format!(
                "Cannot create native image folder for {}",
                destination.display()
            )
        })?;
        fs::write(&destination, bytes)
            .with_context(|| format!("Cannot write native image {}", destination.display()))?;
        written += 1;
    }
    ensure!(
        written > 0,
        "No native image cache data available for {path}"
    );
    Ok(written)
}

fn native_image_path(path: &str, cache_name: &str) -> Result<PathBuf> {
    let mut relative = PathBuf::from("native").join(safe_relative_path(path)?);
    let cache = safe_relative_path(cache_name)?;
    ensure!(
        cache.components().count() == 1,
        "Invalid native image cache name: {cache_name}"
    );
    let filename = relative
        .file_name()
        .context("Missing native image filename")?
        .to_string_lossy();
    relative.set_file_name(format!("{filename}.{}.bin", cache.display()));
    Ok(relative)
}

/// Pull the first cache entry matching a predicate over the cache name.
fn pull_first(
    oodle: &Oodle,
    locations: &[(String, usize)],
    cache_map: &HashMap<String, &LazyCache>,
    path: &str,
    pred: impl Fn(&str) -> bool,
) -> Option<Vec<u8>> {
    for (cache_name, entry_index) in locations {
        if !pred(cache_name) {
            continue;
        }
        if let Some(cache) = cache_map.get(cache_name) {
            if let Some(entry) = cache
                .entries
                .get(entry_index - 1)
                .filter(|entry| !entry.is_dir && path_key(&entry.full_path) == path_key(path))
            {
                if let Ok(data) = cache.get(oodle, entry) {
                    if !data.is_empty() {
                        return Some(data);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
fn write_output(
    flat: bool,
    output_root: &Path,
    path: &str,
    img_type: &ImageType,
    output: Vec<u8>,
    file_name: &str,
    extracted: &mut usize,
    output_format: &str,
) -> Result<()> {
    write_output_inner(
        flat,
        output_root,
        path,
        img_type,
        output,
        file_name,
        extracted,
        output_format,
        false,
    )
}

fn write_output_inner(
    flat: bool,
    output_root: &Path,
    path: &str,
    img_type: &ImageType,
    output: Vec<u8>,
    file_name: &str,
    extracted: &mut usize,
    output_format: &str,
    preserve_source_extension: bool,
) -> Result<()> {
    safe_relative_path(path)?;
    let want_png = output_format.eq_ignore_ascii_case("png");

    let out_name = if want_png {
        png_output_name(file_name, preserve_source_extension)
    } else if preserve_source_extension {
        dds_output_name(file_name)
    } else if *img_type == ImageType::Png {
        file_name.replace(".png", ".dds")
    } else if *img_type == ImageType::Jpg {
        file_name.replace(".jpg", ".dds")
    } else {
        file_name.to_string()
    };

    let out_path = if flat {
        output_root.join(safe_relative_path(&out_name)?)
    } else {
        let relative_path = if preserve_source_extension {
            path
        } else if let Some(pos) = path.find("/Lotus/") {
            &path[pos + 1..]
        } else if let Some(pos) = path.find("/SF/") {
            &path[pos + 1..]
        } else if let Some(pos) = path.find("/EE/") {
            &path[pos + 1..]
        } else {
            path
        };

        let adjusted_path = if want_png {
            png_output_name(relative_path, preserve_source_extension)
        } else if preserve_source_extension {
            dds_output_name(relative_path)
        } else if *img_type == ImageType::Png {
            relative_path.replace(".png", ".dds")
        } else if *img_type == ImageType::Jpg {
            relative_path.replace(".jpg", ".dds")
        } else {
            relative_path.to_string()
        };

        output_root.join(safe_relative_path(&adjusted_path)?)
    };

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create image output folder {}", parent.display()))?;
    }

    if out_path.is_dir() {
        anyhow::bail!(
            "Cannot write image {}: a folder already exists at this path",
            out_path.display()
        );
    }

    if want_png {
        // Patch the DDS header to declare a single mip — the body usually only
        // contains the top mip; otherwise the PNG decoder rejects a size mismatch.
        let mut dds_buf = output;
        if dds_buf.len() >= 128 && &dds_buf[0..4] == b"DDS " {
            dds_buf[28..32].copy_from_slice(&1u32.to_le_bytes()); // dwMipMapCount = 1
            dds_buf[108..112].copy_from_slice(&0x0000_1000u32.to_le_bytes()); // dwCaps = DDSCAPS_TEXTURE
        }
        let dds = ddsfile::Dds::read(&mut Cursor::new(&dds_buf))
            .map_err(|e| anyhow!("Failed to parse DDS: {}", e))?;
        let img = image_dds::image_from_dds(&dds, 0)
            .map_err(|e| anyhow!("Failed to decode DDS: {}", e))?;
        img.save(&out_path)
            .with_context(|| format!("Cannot save PNG {}", out_path.display()))?;
    } else {
        fs::write(&out_path, output)
            .with_context(|| format!("Cannot write image {}", out_path.display()))?;
    }

    *extracted += 1;
    if *extracted % 50 == 0 {
        println!("  Extracted {} images...", *extracted);
    }
    Ok(())
}

// Selected exports retain non-PNG suffixes: game caches contain both a.jpg
// and a.png, and converting both to a.png would silently overwrite an asset.
fn png_output_name(path: &str, preserve_source_extension: bool) -> String {
    if preserve_source_extension && !path.to_ascii_lowercase().ends_with(".png") {
        format!("{path}.png")
    } else {
        let base = path.rsplit_once('.').map(|(base, _)| base).unwrap_or(path);
        format!("{base}.png")
    }
}

fn dds_output_name(path: &str) -> String {
    if path.to_ascii_lowercase().ends_with(".dds") {
        path.to_owned()
    } else {
        format!("{path}.dds")
    }
}

// Cache names are archive paths, not operating-system paths. Validate before
// joining and escape Windows filename characters without discarding assets.
/// Map a cache path to a relative output path that cannot escape the
/// output folder, escaping characters and device names Windows rejects.
pub fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let normalized = path.replace('\\', "/");
    ensure!(
        !normalized.starts_with('/') && normalized.as_bytes().get(1) != Some(&b':'),
        "Absolute output path: {path}"
    );
    let mut result = PathBuf::new();
    for component in normalized.split('/') {
        ensure!(
            !component.is_empty() && component != "." && component != "..",
            "Unsafe output path: {path}"
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
    ensure!(!result.as_os_str().is_empty(), "Empty output path");
    Ok(result)
}

fn discover_cache_sets(cache_path: &Path) -> Result<Vec<String>> {
    let mut cache_sets = HashSet::new();
    {
        let entries = fs::read_dir(cache_path).with_context(|| {
            format!("Cannot read image cache directory {}", cache_path.display())
        })?;
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                if let Some(stem) = name.strip_suffix(".toc") {
                    cache_sets.insert(stem.to_string());
                }
            }
        }
    }
    let mut result: Vec<String> = cache_sets.into_iter().collect();
    result.sort();
    Ok(result)
}

#[cfg(test)]
mod output_error_tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/image-output-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn args(cache: &Path) -> ExtractArgs {
        ExtractArgs {
            cache_path: Some(cache.to_owned()),
            output: cache.join("output"),
            filter: None,
            path_filter: None,
            limit: 0,
            list_caches: false,
            list_paths: false,
            list_all: false,
            path_filter_substring: false,
            flat: false,
            deswizzle_textures: false,
            fix_dims: false,
            output_format: "png".into(),
        }
    }

    fn cache_fixture(root: &Path, name: &str, entries: &[(&str, u64, u32)]) {
        let mut toc = vec![0x4e, 0xc6, 0x67, 0x18, 0, 0, 0, 0];
        for (path, timestamp, bytes) in entries {
            assert!(path.len() < 64);
            let mut record = vec![0u8; 96];
            record[8..16].copy_from_slice(&timestamp.to_le_bytes());
            record[16..20].copy_from_slice(&bytes.to_le_bytes());
            record[20..24].copy_from_slice(&(bytes * 2).to_le_bytes());
            record[32..32 + path.len()].copy_from_slice(path.as_bytes());
            toc.extend(record);
        }
        fs::write(root.join(format!("{name}.toc")), toc).unwrap();
        fs::write(root.join(format!("{name}.cache")), [0; 8]).unwrap();
    }

    #[test]
    fn catalog_reads_all_image_cache_groups_and_newest_records_without_decoding() {
        let root = fixture();
        cache_fixture(
            &root,
            "H.Texture",
            &[("SF/Art/a.png", 1, 3), ("SF/Art/a.png", 2, 7)],
        );
        cache_fixture(&root, "F.Texture", &[("SF/Art/a.png", 1, 4)]);
        cache_fixture(&root, "F.LightMap", &[("SF/World/tile.dds", 1, 5)]);
        cache_fixture(
            &root,
            "F.Misc",
            &[("SF/Art/a-long.png", 1, 2), ("SF/Art/info.txt", 1, 2)],
        );
        cache_fixture(
            &root,
            "H.LightMap",
            &[
                ("SF/World/0_cross.hdr", 1, 3),
                ("SF/World/0_gbdcm.exr", 1, 4),
            ],
        );
        let catalog = list_assets(&root).unwrap();
        assert_eq!(catalog.len(), 5);
        let image = catalog
            .iter()
            .find(|asset| asset.path == "SF/Art/a.png")
            .unwrap();
        assert_eq!(image.compressed_bytes, 11);
        assert_eq!(image.decompressed_bytes, 22);
        assert_eq!(image.cache_sets, ["F.Texture", "H.Texture"]);
        assert!(catalog
            .iter()
            .any(|asset| asset.path == "SF/World/tile.dds"));
        assert!(!root.join("output").exists());
    }

    #[test]
    fn list_all_preserves_images_and_swfs_without_exposing_cache_scripts() {
        let root = fixture();
        cache_fixture(
            &root,
            "B.Font",
            &[
                ("SF/Interface/picture.png", 1, 3),
                ("SF/Interface/menu.SWF", 1, 3),
                ("SF/Scripts/test.lua", 1, 3),
                ("SF/Scripts/test.luac", 1, 3),
                ("SF/Scripts/test.luau", 1, 3),
                ("SF/other.bin", 1, 3),
            ],
        );
        let caches = vec![(
            "B.Font".into(),
            LazyCache::open(&root, "B.Font").unwrap().unwrap(),
        )];
        let all = image_locations(&caches, true);
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("SF/Interface/picture.png"));
        assert!(all.contains_key("SF/Interface/menu.SWF"));
        assert_eq!(image_locations(&caches, false).len(), 1);

        // The public CLI's old raw-export route must stop before loading
        // native libraries or creating output, even with --list-all enabled.
        for extension in ["lua", "luac", "luau", "bin"] {
            let mut request = args(&root);
            request.list_all = true;
            request.filter = Some(extension.into());
            run_extract(request).unwrap();
            assert!(!root.join("output").exists());
        }
    }

    #[test]
    fn native_image_names_preserve_all_cache_parts_and_remain_inside_output() {
        assert_eq!(
            native_image_path("SF/World/0_cross.hdr", "H.LightMap").unwrap(),
            PathBuf::from("native/SF/World/0_cross.hdr.H.LightMap.bin")
        );
        assert_ne!(
            native_image_path("SF/World/0_gbdcm.exr", "B.LightMap").unwrap(),
            native_image_path("SF/World/0_gbdcm.exr", "F.LightMap").unwrap()
        );
        for (path, cache) in [
            ("../escape.hdr", "H.LightMap"),
            ("/absolute.exr", "H.LightMap"),
            ("SF/World/0.hdr", "../escape"),
            ("SF/World/0.hdr", "H/LightMap"),
        ] {
            assert!(native_image_path(path, cache).is_err());
        }
    }

    #[test]
    fn selected_image_pairing_keeps_cache_families_and_locales_distinct() {
        assert_eq!(
            image_cache_family("H.Texture"),
            image_cache_family("F.Texture")
        );
        assert_ne!(
            image_cache_family("H.Texture"),
            image_cache_family("B.LightMap")
        );
        assert_ne!(
            image_cache_family("H.Texture"),
            image_cache_family("B.Texture_fr")
        );
    }

    #[test]
    fn exact_image_selection_does_not_match_similar_names_or_folders() {
        let mut images = HashMap::from([
            ("SF/Art/a.png".into(), vec![]),
            ("SF/Art/a-long.png".into(), vec![]),
            ("SF/Art/a.png/other.png".into(), vec![]),
        ]);
        select_images(&mut images, &HashSet::from([path_key("sf\\art\\A.PNG")])).unwrap();
        assert_eq!(
            images.keys().collect::<Vec<_>>(),
            [&"SF/Art/a.png".to_owned()]
        );
        assert!(select_images(
            &mut images,
            &HashSet::from([path_key("SF/Art/missing.png")])
        )
        .is_err());
    }

    #[test]
    fn empty_stale_and_unsafe_selections_never_create_output() {
        let root = fixture();
        cache_fixture(&root, "F.Texture", &[("test.dds", 1, 4)]);
        assert!(run_extract_selected(args(&root), &[])
            .unwrap_err()
            .to_string()
            .contains("No image assets selected"));
        assert!(run_extract_selected(args(&root), &["missing.dds".into()])
            .unwrap_err()
            .to_string()
            .contains("no longer in the cache"));
        for path in [
            "../escape.dds",
            "/absolute.dds",
            "C:/escape.dds",
            "SF/../../escape.dds",
            "SF\\..\\escape.dds",
        ] {
            assert!(
                run_extract_selected(args(&root), &[path.into()]).is_err(),
                "{path}"
            );
        }
        assert!(!root.join("output").exists());
    }

    #[test]
    fn image_archive_paths_are_contained_and_reserved_names_are_encoded() {
        let root = fixture();
        let mut count = 0;
        for path in [
            "../escape.dds",
            "SF/../../escape.dds",
            "/absolute.dds",
            "C:/escape.dds",
        ] {
            assert!(write_output(
                false,
                &root,
                path,
                &ImageType::Dds,
                vec![1],
                "escape.dds",
                &mut count,
                "dds"
            )
            .is_err());
        }
        write_output(
            false,
            &root,
            "SF/CON.dds",
            &ImageType::Dds,
            vec![1, 2],
            "CON.dds",
            &mut count,
            "dds",
        )
        .unwrap();
        assert_eq!(fs::read(root.join("SF/%43ON.dds")).unwrap(), [1, 2]);
        assert_eq!(count, 1);
    }

    #[test]
    fn selected_png_exports_do_not_overwrite_assets_with_the_same_basename() {
        let root = fixture();
        let mut dds = crate::dds_header_pack(&crate::TextureInfo {
            metadata: vec![],
            miplevels: 1,
            format: 0,
            width: 1,
            height: 1,
            data_size: 4,
        });
        dds.extend([255, 0, 0, 255]);
        let mut count = 0;
        for path in ["SF/Art/a.png", "SF/Art/a.jpg", "SF/Art/a.dds"] {
            write_output_inner(
                false,
                &root,
                path,
                &ImageType::from_extension(path),
                dds.clone(),
                path.rsplit('/').next().unwrap(),
                &mut count,
                "png",
                true,
            )
            .unwrap();
        }
        assert_eq!(count, 3);
        for path in ["SF/Art/a.png", "SF/Art/a.jpg.png", "SF/Art/a.dds.png"] {
            assert!(root.join(path).is_file(), "{path}");
        }
        assert_eq!(png_output_name("SF/Art/a.jpg", false), "SF/Art/a.png");
        for path in [
            "SF/Art/b.png",
            "SF/Art/b.jpg",
            "SF/Art/b.dds",
            "SF/Art/b.tga",
        ] {
            write_output_inner(
                false,
                &root,
                path,
                &ImageType::from_extension(path),
                dds.clone(),
                path.rsplit('/').next().unwrap(),
                &mut count,
                "dds",
                true,
            )
            .unwrap();
        }
        for path in [
            "SF/Art/b.png.dds",
            "SF/Art/b.jpg.dds",
            "SF/Art/b.dds",
            "SF/Art/b.tga.dds",
        ] {
            assert!(root.join(path).is_file(), "{path}");
            assert_eq!(fs::read(root.join(path)).unwrap(), dds);
        }
    }

    #[test]
    fn blocked_parent_reports_destination_and_preserves_existing_file() {
        let root = fixture();
        let blocked = root.join("blocked");
        fs::write(&blocked, b"keep this existing file").unwrap();
        let mut count = 0;
        let error = write_output(
            false,
            &blocked,
            "SF/test.dds",
            &ImageType::Dds,
            vec![1, 2, 3],
            "test.dds",
            &mut count,
            "dds",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(&blocked.display().to_string()));
        assert!(error.chain().any(|cause| cause.is::<std::io::Error>()));
        assert_eq!(fs::read(&blocked).unwrap(), b"keep this existing file");
        assert_eq!(count, 0);
    }

    #[test]
    fn output_directory_collision_is_reported_without_deleting_contents() {
        let root = fixture();
        let collision = root.join("test.dds");
        fs::create_dir(&collision).unwrap();
        fs::write(collision.join("existing.txt"), b"keep").unwrap();
        let mut count = 0;
        let error = write_output(
            true,
            &root,
            "SF/test.dds",
            &ImageType::Dds,
            vec![1, 2, 3],
            "test.dds",
            &mut count,
            "dds",
        )
        .unwrap_err();
        assert!(error.to_string().contains("a folder already exists"));
        assert_eq!(fs::read(collision.join("existing.txt")).unwrap(), b"keep");
        assert_eq!(count, 0);
    }
}
