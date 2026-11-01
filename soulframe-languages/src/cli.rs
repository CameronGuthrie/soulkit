//! CLI subcommands for `soulframe-languages`, factored out so other crates
//! (the `soulkit-cli` umbrella binary) can embed them as a nested subcommand.

use anyhow::{anyhow, Context, Result};
use clap::Subcommand;
use rand::Rng as _;
use serde_json::json;
use soulframe_cache::{
    cache_get_toc_entry, detect_soulframe_cache, toc_decode, toc_find_path, Oodle,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::{
    b64m_encode, languages_unpack, shcc_unpack, CacheManifest, ShccData, Zstd, TYPE_BIN,
    TYPE_MANIFEST,
};

pub const DEFAULT_LOCALES: &str = "en,fr,de,es,it,pt,ru,pl,tr,ja,ko,zh";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Download Languages.bin for the given locales from the Soulframe CDN.
    Download {
        #[arg(short, long, default_value = DEFAULT_LOCALES)]
        locales: String,
    },
    /// Convert downloaded Languages.bin_H files into per-locale JSON.
    Extract {
        #[arg(short, long, default_value = DEFAULT_LOCALES)]
        locales: String,
    },
    /// Pull Languages.bin from the local Soulframe H.Misc cache instead of the CDN.
    ExtractFromCache {
        #[arg(short, long, default_value = DEFAULT_LOCALES)]
        locales: String,
        /// Override Soulframe cache directory (defaults to %LOCALAPPDATA%\Soulframe\Downloaded\Public\Cache.Windows).
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Output directory for extracted Languages_{locale}.bin files.
        #[arg(long, default_value = "extracted-from-cache")]
        output: PathBuf,
    },
}

pub fn parse_locales(s: &str) -> Vec<String> {
    s.split(',').map(|s| s.trim().to_string()).collect()
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Download { locales } => run_download(&parse_locales(&locales)),
        Cmd::Extract { locales } => run_extract(&parse_locales(&locales)),
        Cmd::ExtractFromCache {
            locales,
            cache_dir,
            output,
        } => run_extract_from_cache(&parse_locales(&locales), cache_dir.as_deref(), &output),
    }
}

// ─── download ────────────────────────────────────────────────────────────

fn make_http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .http1_only()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .user_agent("SoulframeLauncher/1.0")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("creating language download HTTP client")
}

fn download_shcc_file(
    client: &reqwest::blocking::Client,
    path: &str,
    file_type: u8,
    hash: &str,
    suffix: &str,
    oodle: &Oodle,
) -> Result<ShccData> {
    let type_hex = format!("{:X}", file_type);
    let req_path = format!("/0{}{}!{}_{}", suffix, path, type_hex, hash);

    let is_h_cache_placeholder = path == "/H.Cache.bin" && hash == "---------------------w";

    let urls: Vec<String> = if is_h_cache_placeholder {
        let random_id = format!("{:08X}", rand::thread_rng().gen::<u32>());
        vec![format!(
            "https://origin.soulframe.com/origin/{}{}",
            random_id, req_path
        )]
    } else {
        vec![
            format!("http://content.soulframe.com{}", req_path),
            format!("https://origin.soulframe.com{}", req_path),
        ]
    };

    for url in &urls {
        println!("  Downloading {}", url);
        match client.get(url).send() {
            Ok(resp) if resp.status().is_success() => {
                let bin = resp.bytes()?.to_vec();
                println!("  ✓ {} bytes", bin.len());
                let bin = if !bin.starts_with(b"SHCC") {
                    oodle
                        .decompress(&bin, bin.len() * 10)
                        .with_context(|| "decompressing non-SHCC CDN payload")?
                } else {
                    bin
                };
                let data = shcc_unpack(&bin, oodle)?;
                return Ok(data);
            }
            Ok(resp) => println!("  HTTP {}", resp.status().as_u16()),
            Err(e) => println!("  Error: {}", e),
        }
    }
    Err(anyhow!("All download attempts failed for {}", path))
}

fn download_languages_via_manifest(
    client: &reqwest::blocking::Client,
    meta_manifest: &mut CacheManifest,
    locale: &str,
    oodle: &Oodle,
) -> Result<Vec<u8>> {
    let localized_manifest_path = format!("/B.Cache.Windows_{}.bin", locale);
    let manifest_hash = meta_manifest
        .get_hash(&localized_manifest_path)
        .ok_or_else(|| {
            anyhow!(
                "{} not found in H.Cache.bin manifest",
                localized_manifest_path
            )
        })?;
    let manifest_hash_b64 = b64m_encode(&manifest_hash);
    println!(
        "  Step 1: {} hash = {}",
        localized_manifest_path, manifest_hash_b64
    );

    println!("  Step 2: Downloading {}...", localized_manifest_path);
    let locale_manifest_data = download_shcc_file(
        client,
        &localized_manifest_path,
        TYPE_MANIFEST,
        &manifest_hash_b64,
        "",
        oodle,
    )?;

    let mut locale_manifest = CacheManifest::new(locale_manifest_data.h);

    let lang_hash = locale_manifest.get_hash("/Languages.bin").ok_or_else(|| {
        anyhow!(
            "/Languages.bin not found in {} manifest",
            localized_manifest_path
        )
    })?;
    let lang_hash_b64 = b64m_encode(&lang_hash);
    println!("  Step 3: /Languages.bin hash = {}", lang_hash_b64);

    let suffix = format!("_{}", locale);
    println!("  Step 4: Downloading /Languages.bin...");
    let lang_data = download_shcc_file(
        client,
        "/Languages.bin",
        TYPE_BIN,
        &lang_hash_b64,
        &suffix,
        oodle,
    )?;
    Ok(lang_data.h)
}

/// Download language packs using the standalone command's working-directory defaults.
pub fn run_download(locales: &[String]) -> Result<()> {
    let root = std::env::current_dir().context("locating language download directory")?;
    run_download_to(locales, &root.join("downloaded-data"))
}

fn validate_locale(locale: &str) -> Result<()> {
    let mut chars = locale.chars();
    let valid = (1..=32).contains(&locale.len())
        && chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if !valid {
        return Err(anyhow!(
            "Invalid language locale {locale:?}; use a simple language code"
        ));
    }
    Ok(())
}

fn validate_locales(locales: &[String]) -> Result<()> {
    if locales.is_empty() {
        return Err(anyhow!("At least one language locale is required"));
    }
    for locale in locales {
        validate_locale(locale)?;
    }
    Ok(())
}

fn download_path(root: &std::path::Path, locale: &str) -> PathBuf {
    root.join(format!("0_{locale}")).join("Languages.bin_H")
}

fn extracted_locale_path(root: &std::path::Path, locale: &str) -> PathBuf {
    root.join("0")
        .join("Languages")
        .join(format!("{locale}.json"))
}

/// Download all requested language packs under an explicit scratch directory.
/// A failure for any requested locale is reported after the remaining locales.
pub fn run_download_to(locales: &[String], download_root: &std::path::Path) -> Result<()> {
    validate_locales(locales)?;
    println!("=== Soulframe Language Downloader ===\n");

    println!("Initializing Oodle decompression...");
    let oodle = Oodle::new()?;
    println!("  ✓ Oodle ready\n");

    let client = make_http_client()?;
    println!("Downloading H.Cache.bin (meta manifest)...");
    let h_cache_data = download_shcc_file(
        &client,
        "/H.Cache.bin",
        TYPE_MANIFEST,
        "---------------------w",
        "",
        &oodle,
    )?;
    let mut meta_manifest = CacheManifest::new(h_cache_data.h);
    let count = meta_manifest.parse_all();
    println!("  ✓ Meta manifest loaded: {} entries\n", count);

    let mut succeeded = 0;
    let mut failures = Vec::new();
    for locale in locales {
        println!("========================================");
        println!("  Locale: {locale}");
        println!("========================================");
        let result = (|| -> Result<()> {
            let languages_bin =
                download_languages_via_manifest(&client, &mut meta_manifest, locale, &oodle)?;
            println!("  Languages.bin: {} bytes", languages_bin.len());
            let path = download_path(download_root, locale);
            fs::create_dir_all(path.parent().context("missing language download parent")?)
                .with_context(|| format!("creating language download directory for {locale}"))?;
            fs::write(&path, &languages_bin)
                .with_context(|| format!("writing downloaded language pack {}", path.display()))?;
            println!("  ✓ Saved to {}\n", path.display());
            Ok(())
        })();
        match result {
            Ok(()) => succeeded += 1,
            Err(error) => {
                eprintln!("  ✗ {locale}: {error:#}\n");
                failures.push(format!("{locale}: {error:#}"));
            }
        }
    }
    println!("Done: {succeeded} succeeded, {} failed", failures.len());
    println!("Files saved to {}", download_root.display());
    if !failures.is_empty() {
        return Err(anyhow!(
            "Language download failed for {} of {} requested locales: {}",
            failures.len(),
            locales.len(),
            failures.join("; ")
        ));
    }
    Ok(())
}

// ─── extract ─────────────────────────────────────────────────────────────

/// Publish the same generated JSON to its scratch and final destinations.
/// Both writes must succeed before the locale is considered extracted.
fn publish_locale_json(
    locale: &str,
    json_content: &str,
    extract_root: &std::path::Path,
    output_root: &std::path::Path,
) -> Result<()> {
    validate_locale(locale)?;
    let extracted = extracted_locale_path(extract_root, locale);
    fs::create_dir_all(
        extracted
            .parent()
            .context("missing extracted language parent")?,
    )
    .with_context(|| format!("creating language extraction directory for {locale}"))?;
    fs::write(&extracted, json_content)
        .with_context(|| format!("writing extracted language JSON {}", extracted.display()))?;
    fs::create_dir_all(output_root).with_context(|| {
        format!(
            "creating language output directory {}",
            output_root.display()
        )
    })?;
    let published = output_root.join(format!("Languages_{locale}.json"));
    fs::write(&published, json_content)
        .with_context(|| format!("publishing language JSON {}", published.display()))?;
    println!("  ✓ {} -> {}", extracted.display(), published.display());
    Ok(())
}

fn publish_alias(
    locale: &str,
    extract_root: &std::path::Path,
    output_root: &std::path::Path,
) -> Result<()> {
    validate_locale(locale)?;
    let source = output_root.join(format!("Languages_{locale}.json"));
    let content = fs::read(&source).with_context(|| {
        format!(
            "reading published language alias source {}",
            source.display()
        )
    })?;
    for alias in [
        extract_root
            .join("0")
            .join("Languages")
            .join("Languages.json"),
        output_root.join("Languages.json"),
    ] {
        fs::write(&alias, &content)
            .with_context(|| format!("publishing language alias {}", alias.display()))?;
    }
    println!("Alias written: Languages.json -> {locale}.json");
    Ok(())
}

fn write_locale_json(
    locale: &str,
    bin: &[u8],
    zstd: &Zstd,
    extract_root: &std::path::Path,
    output_root: &std::path::Path,
) -> Result<usize> {
    let entries = languages_unpack(bin, zstd)?;
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();

    let mut ordered = BTreeMap::new();
    ordered.insert("__order".to_string(), json!(keys));
    for key in &keys {
        if let Some(value) = entries.get(*key) {
            ordered.insert((*key).clone(), json!(value));
        }
    }
    let json_content = serde_json::to_string_pretty(&ordered)?;
    publish_locale_json(locale, &json_content, extract_root, output_root)?;
    println!("  ✓ {} strings", keys.len());
    Ok(keys.len())
}

/// Extract downloaded packs using the standalone command's working-directory defaults.
pub fn run_extract(locales: &[String]) -> Result<()> {
    let root = std::env::current_dir().context("locating language extraction directory")?;
    run_extract_to(
        locales,
        &root.join("downloaded-data"),
        &root.join("extracted-data"),
        &root.join("output").join("Languages"),
    )
}

/// Extract every requested locale and publish final JSON to an explicit directory.
/// Missing inputs and failed canonical writes are errors, so callers retain scratch
/// data when extraction or publication is incomplete.
pub fn run_extract_to(
    locales: &[String],
    download_root: &std::path::Path,
    extract_root: &std::path::Path,
    output_root: &std::path::Path,
) -> Result<()> {
    validate_locales(locales)?;
    println!("=== Extract downloaded Languages.bin -> JSON ===");
    let mut available = Vec::new();
    let mut failures = Vec::new();
    for locale in locales {
        let input = download_path(download_root, locale);
        if input.is_file() {
            available.push(locale);
        } else {
            failures.push(format!(
                "{locale}: missing downloaded language pack {}",
                input.display()
            ));
        }
    }
    if available.is_empty() {
        return Err(anyhow!(
            "No requested language packs are available: {}",
            failures.join("; ")
        ));
    }

    let zstd = Zstd::new().context("loading Zstd for language extraction")?;
    let mut succeeded = Vec::new();
    for locale in available {
        println!("[{locale}] Extracting...");
        let input = download_path(download_root, locale);
        let result = fs::read(&input)
            .with_context(|| format!("reading downloaded language pack {}", input.display()))
            .and_then(|bin| write_locale_json(locale, &bin, &zstd, extract_root, output_root));
        match result {
            Ok(_) => succeeded.push(locale.as_str()),
            Err(error) => {
                eprintln!("  ✗ {locale}: {error:#}");
                failures.push(format!("{locale}: {error:#}"));
            }
        }
    }

    // An alias can only come from a locale successfully published in this run;
    // a stale JSON left by an earlier build must not make a failed run succeed.
    if let Some(locale) = succeeded
        .iter()
        .find(|locale| **locale == "en")
        .or(succeeded.first())
    {
        if let Err(error) = publish_alias(locale, extract_root, output_root) {
            failures.push(format!("Languages.json: {error:#}"));
        }
    }
    println!(
        "Done: {} locales published to {}, {} failed",
        succeeded.len(),
        output_root.display(),
        failures.len()
    );
    if !failures.is_empty() {
        return Err(anyhow!(
            "Language extraction failed for requested outputs: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ─── extract-from-cache ──────────────────────────────────────────────────

pub fn run_extract_from_cache(
    locales: &[String],
    cache_dir: Option<&std::path::Path>,
    output: &std::path::Path,
) -> Result<()> {
    validate_locales(locales)?;
    let cache_dir = match cache_dir {
        Some(p) => p.to_path_buf(),
        None => detect_soulframe_cache().ok_or_else(|| {
            anyhow!("Could not detect Soulframe cache directory; pass --cache-dir")
        })?,
    };

    println!("=== Soulframe Cache Extractor ===");
    println!("Using cache directory: {}", cache_dir.display());

    if !cache_dir.exists() {
        return Err(anyhow!(
            "Cache directory does not exist: {}",
            cache_dir.display()
        ));
    }

    let oodle = Oodle::new()?;
    fs::create_dir_all(output)?;

    let shared_cache_path = cache_dir.join("H.Misc.cache");
    if !shared_cache_path.exists() {
        return Err(anyhow!(
            "Shared cache file not found: {}",
            shared_cache_path.display()
        ));
    }
    let cache_bytes = fs::read(&shared_cache_path)
        .with_context(|| format!("reading {}", shared_cache_path.display()))?;
    println!("Shared cache: {} bytes", cache_bytes.len());

    let mut ok_count = 0;
    let mut fail_count = 0;

    for locale in locales {
        println!("\n--- Locale: {} ---", locale);

        let toc_path = cache_dir.join(format!("H.Misc_{}.toc", locale));
        if !toc_path.exists() {
            println!("  TOC not found: {}", toc_path.display());
            fail_count += 1;
            continue;
        }
        let toc_bin =
            fs::read(&toc_path).with_context(|| format!("reading {}", toc_path.display()))?;
        let entries = toc_decode(&toc_bin)?;
        println!("  Loaded {} TOC entries", entries.len());

        let entry = match toc_find_path(&entries, "Languages.bin") {
            Some(e) => e,
            None => {
                println!("  ✗ Languages.bin not in TOC");
                fail_count += 1;
                continue;
            }
        };
        println!(
            "    cache_offset={}  compressed={}  decompressed={}",
            entry.cache_offset, entry.compressed_size, entry.decompressed_size
        );

        match cache_get_toc_entry(&oodle, &cache_bytes, entry) {
            Ok(data) => {
                let out_path = output.join(format!("Languages_{}.bin", locale));
                fs::write(&out_path, &data)?;
                println!("  ✓ Extracted to: {}", out_path.display());
                ok_count += 1;
            }
            Err(e) => {
                println!("  ✗ Failed to decompress: {}", e);
                fail_count += 1;
            }
        }
    }

    println!(
        "\n✓ {} succeeded, {} failed. Output: {}",
        ok_count,
        fail_count,
        output.display()
    );
    println!("\nNext step: feed the extracted .bin files into the `extract` subcommand.");
    if fail_count > 0 {
        return Err(anyhow!(
            "Failed to extract {fail_count} of {} requested language packs",
            locales.len()
        ));
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
                "soulframe-language-publish-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed),
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

    #[test]
    fn language_json_and_alias_are_published_to_explicit_final_directory() {
        let root = TestDir::new();
        let scratch = root.0.join("scratch/extracted");
        let final_dir = root.0.join("final/Languages");
        let content = r#"{"__order":["/SF/Example"],"/SF/Example":"Example"}"#;
        publish_locale_json("en", content, &scratch, &final_dir).unwrap();
        publish_alias("en", &scratch, &final_dir).unwrap();
        for path in [
            scratch.join("0/Languages/en.json"),
            scratch.join("0/Languages/Languages.json"),
            final_dir.join("Languages_en.json"),
            final_dir.join("Languages.json"),
        ] {
            assert_eq!(fs::read_to_string(path).unwrap(), content);
        }
        assert_eq!(
            download_path(&root.0.join("downloads"), "pt-BR"),
            root.0.join("downloads/0_pt-BR/Languages.bin_H"),
        );
    }

    #[test]
    fn final_publication_failure_is_reported_and_scratch_json_remains() {
        let root = TestDir::new();
        let scratch = root.0.join("extracted");
        let blocked_output = root.0.join("blocked-output");
        fs::write(&blocked_output, b"existing file").unwrap();
        let error = publish_locale_json("fr", "{}", &scratch, &blocked_output).unwrap_err();
        assert!(format!("{error:#}").contains("language output directory"));
        assert_eq!(
            fs::read_to_string(scratch.join("0/Languages/fr.json")).unwrap(),
            "{}"
        );
        assert_eq!(fs::read(&blocked_output).unwrap(), b"existing file");
    }

    #[test]
    fn missing_requested_locales_are_errors_without_native_libraries_or_outputs() {
        let root = TestDir::new();
        let downloads = root.0.join("downloads");
        let extracted = root.0.join("extracted");
        let final_dir = root.0.join("Languages");
        let error = run_extract_to(
            &["en".into(), "fr".into()],
            &downloads,
            &extracted,
            &final_dir,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("en: missing"));
        assert!(message.contains("fr: missing"));
        assert!(!extracted.exists() && !final_dir.exists());
        assert!(run_extract_to(&[], &downloads, &extracted, &final_dir).is_err());
    }

    #[test]
    fn unsafe_locales_fail_before_dependencies_or_files_are_touched() {
        let root = TestDir::new();
        for locale in [
            "",
            "../en",
            "en/../../escape",
            "en\\..\\escape",
            ".",
            "..",
            "C:",
            "en?",
            "en us",
        ] {
            let locales = [locale.to_owned()];
            assert!(
                run_download_to(&locales, &root.0.join("download")).is_err(),
                "{locale}"
            );
            assert!(
                run_extract_to(
                    &locales,
                    &root.0.join("download"),
                    &root.0.join("extracted"),
                    &root.0.join("output")
                )
                .is_err(),
                "{locale}"
            );
        }
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
        for locale in ["en", "pt-BR", "zh_Hans"] {
            validate_locale(locale).unwrap();
        }
    }
}
