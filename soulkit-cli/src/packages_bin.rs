//! Port of `generate_all.py::parse_packages_bin`, `build_crafting_recipes`,
//! and `workdir/extract_manifest_rewards.py`.
//!
//! Generates the three intermediate JSONs the items pipeline depends on
//! (`packages_bin_attrs.json`, `crafting_recipes.json`,
//! `manifest_rewards_raw.json`) directly from the local Soulframe cache,
//! eliminating the need for the Python `soulframe-research` pipeline.

use anyhow::{anyhow, ensure, Context, Result};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use soulframe_cache::{
    cache_get_toc_entry, detect_soulframe_cache, load_local_cache, toc_find_path, Oodle, TocEntry,
};
use soulframe_languages::Zstd;
use std::collections::BTreeMap;
use std::path::Path;

// ─── Public output types ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PkgEntry {
    pub path: String,
    pub parent: String,
    pub attributes: String,
}

pub type PkgAttrs = BTreeMap<String, PkgEntry>;

// ─── Attribute-shift correction ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttributeRotation {
    None,
    Previous,
    Next,
}

/// Some Packages.bin builds emit the plaintext frame stream one text-bearing
/// entity out of phase with the entity stream. Detect that from path/attribute
/// self-consistency and rotate only when there is strong evidence. This avoids
/// cyclically shifting already-aligned builds.
fn align_text_attributes(ordered: &mut [(String, PkgEntry)]) -> AttributeRotation {
    let n = ordered.len();
    if n < 2 {
        return AttributeRotation::None;
    }

    let current = attribute_alignment_score(ordered, AttributeRotation::None);
    let previous = attribute_alignment_score(ordered, AttributeRotation::Previous);
    let next = attribute_alignment_score(ordered, AttributeRotation::Next);

    let (best_rotation, best_score) = [
        (AttributeRotation::None, current),
        (AttributeRotation::Previous, previous),
        (AttributeRotation::Next, next),
    ]
    .into_iter()
    .max_by_key(|(_, score)| *score)
    .unwrap();

    // Require both a meaningful absolute delta and a 10% relative improvement.
    // Small score changes can happen naturally in adjacent similar entries.
    let min_delta = if n < 32 { 0 } else { (current / 10).max(128) };
    if best_rotation == AttributeRotation::None || best_score <= current + min_delta {
        return AttributeRotation::None;
    }

    apply_attribute_rotation(ordered, best_rotation);
    best_rotation
}

fn apply_attribute_rotation(ordered: &mut [(String, PkgEntry)], rotation: AttributeRotation) {
    if rotation == AttributeRotation::None {
        return;
    }

    let n = ordered.len();
    let attrs: Vec<String> = ordered
        .iter()
        .map(|(_, entry)| entry.attributes.clone())
        .collect();
    for (i, (_, entry)) in ordered.iter_mut().enumerate() {
        let src = match rotation {
            AttributeRotation::None => i,
            AttributeRotation::Previous => (i + n - 1) % n,
            AttributeRotation::Next => (i + 1) % n,
        };
        entry.attributes = attrs[src].clone();
    }
}

fn attribute_alignment_score(ordered: &[(String, PkgEntry)], rotation: AttributeRotation) -> usize {
    let n = ordered.len();
    let mut score = 0usize;

    for (i, (path, _)) in ordered.iter().enumerate() {
        let src = match rotation {
            AttributeRotation::None => i,
            AttributeRotation::Previous => (i + n - 1) % n,
            AttributeRotation::Next => (i + 1) % n,
        };
        let attrs = ordered[src].1.attributes.as_str();
        score += score_path_attrs_alignment(path, attrs);
    }

    score
}

fn score_path_attrs_alignment(path: &str, attrs: &str) -> usize {
    let Some(leaf) = path.rsplit('/').next() else {
        return 0;
    };
    if leaf.len() < 4 {
        return 0;
    }

    let mut score = 0usize;
    let leaf_l = leaf.to_ascii_lowercase();
    let attrs_l = attrs.to_ascii_lowercase();

    if attrs_l.contains(&leaf_l) {
        score += 3;
    }
    if let Some(type_name) = capture_attr_path(attrs, "TypeName=") {
        if type_name.rsplit('/').next() == Some(leaf) {
            score += 5;
        }
    }
    if let Some(result_item) = capture_attr_path(attrs, "ResultItem=") {
        if result_item.rsplit('/').next() == Some(leaf) {
            score += 5;
        }
    }

    let stem = item_name_stem(&leaf_l);
    if stem.len() >= 4 {
        if let Some(localize_tag) = capture_attr_path(attrs, "LocalizeTag=") {
            if localize_tag.to_ascii_lowercase().contains(stem) {
                score += 2;
            }
        }
        if let Some(icon) = capture_attr_path(attrs, "Icon=") {
            if icon.to_ascii_lowercase().contains(stem) {
                score += 1;
            }
        }
    }

    score
}

fn item_name_stem(leaf_l: &str) -> &str {
    for suffix in [
        "blueprint",
        "resourceitem",
        "resource",
        "storeitem",
        "miscitem",
        "meleeweapon",
        "rangedweapon",
        "weapon",
        "item",
    ] {
        if let Some(stem) = leaf_l.strip_suffix(suffix) {
            return stem;
        }
    }
    leaf_l
}

fn capture_attr_path<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let start = attrs.find(key)? + key.len();
    let tail = &attrs[start..];
    let end = tail
        .find(|c: char| c.is_whitespace() || c == ',' || c == '}')
        .unwrap_or(tail.len());
    let value = tail[..end].trim_matches('"');
    value.starts_with('/').then_some(value)
}

// ─── Low-level helpers ────────────────────────────────────────────────────

fn read_u32(data: &[u8], pos: usize) -> Result<(u32, usize)> {
    if pos + 4 > data.len() {
        return Err(anyhow!("read_u32: out of bounds at {pos}"));
    }
    let v = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
    Ok((v, pos + 4))
}

fn read_ascii(data: &[u8], pos: usize, len: usize) -> Result<(String, usize)> {
    if pos + len > data.len() {
        return Err(anyhow!("read_ascii: out of bounds at {pos}+{len}"));
    }
    let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
    Ok((s, pos + len))
}

fn read_uleb128(data: &[u8], pos: usize) -> Result<(usize, usize)> {
    let mut result: usize = 0;
    let mut shift = 0u32;
    let mut p = pos;
    loop {
        if p >= data.len() {
            return Err(anyhow!("read_uleb128: truncated at {p}"));
        }
        let b = data[p];
        p += 1;
        result |= ((b & 0x7F) as usize) << shift;
        if (b & 0x80) == 0 {
            return Ok((result, p));
        }
        shift += 7;
        if shift > 63 {
            return Err(anyhow!("read_uleb128: overflow"));
        }
    }
}

struct PackageTextBuffers<'a> {
    flags: &'a [u8],
    sizes: &'a [u8],
    compressed: &'a [u8],
    end: usize,
}

/// v46 inserts variable metadata between its length-prefixed buffers. A small
/// integer in that metadata is not sufficient evidence of a buffer boundary.
/// Cross-check the dictionary, every frame length, and the entity flag count.
fn find_v46_text_buffers(raw: &[u8], start: usize) -> Result<PackageTextBuffers<'_>> {
    const DICTIONARY_MAGIC: [u8; 4] = [0x37, 0xA4, 0x30, 0xEC];
    let search = raw
        .get(start..)
        .context("Packages.bin buffers start out of bounds")?;
    for (offset, magic) in search.windows(4).enumerate() {
        if magic != DICTIONARY_MAGIC {
            continue;
        }
        let zstart = start + offset;
        if zstart < start + 8 {
            continue;
        }
        let (zl, _) = read_u32(raw, zstart - 4)?;
        let Some(zend) = zstart.checked_add(zl as usize) else {
            continue;
        };
        let Some(compressed) = raw.get(zstart..zend) else {
            continue;
        };
        let Ok((entity_count, _)) = read_u32(raw, zend) else {
            continue;
        };
        if entity_count == 0 || entity_count as usize > raw.len() {
            continue;
        }
        for size_prefix in start..zstart - 8 {
            let (sl, size_start) = read_u32(raw, size_prefix)?;
            if sl < 4 {
                continue;
            }
            let Some(size_end) = size_start.checked_add(sl as usize) else {
                continue;
            };
            if size_end > zstart - 4 {
                continue;
            }
            let sizes = &raw[size_start..size_end];
            let (dict_size, _) = read_u32(sizes, 0)?;
            if dict_size < 4 || dict_size as usize > compressed.len() {
                continue;
            }
            let mut frame_bytes = dict_size as usize;
            let mut frame_count = 0usize;
            let mut size_pos = 4;
            while size_pos < sizes.len() && frame_bytes <= compressed.len() {
                let Ok((length, next)) = read_uleb128(sizes, size_pos) else {
                    break;
                };
                let Some(next_bytes) = frame_bytes.checked_add(length) else {
                    break;
                };
                frame_bytes = next_bytes;
                frame_count += 1;
                size_pos = next;
            }
            if size_pos != sizes.len()
                || frame_bytes != compressed.len()
                || frame_count > entity_count as usize
            {
                continue;
            }
            let flag_len = (entity_count as usize + frame_count).div_ceil(8);
            let Some(last_flag_prefix) = size_prefix.checked_sub(flag_len + 4) else {
                continue;
            };
            for flag_prefix in start..=last_flag_prefix {
                let (fl, flag_start) = read_u32(raw, flag_prefix)?;
                if fl as usize != flag_len {
                    continue;
                }
                let flags = &raw[flag_start..flag_start + flag_len];
                let mut bit = 0usize;
                let mut text_count = 0usize;
                for _ in 0..entity_count {
                    let Some(byte) = flags.get(bit / 8) else {
                        break;
                    };
                    let has_text = (byte >> (bit % 8)) & 1;
                    bit += 1 + has_text as usize;
                    text_count += has_text as usize;
                }
                if text_count == frame_count
                    && bit == entity_count as usize + frame_count
                    && bit <= flags.len() * 8
                {
                    return Ok(PackageTextBuffers {
                        flags,
                        sizes,
                        compressed,
                        end: zend,
                    });
                }
            }
        }
    }
    Err(anyhow!(
        "Could not locate validated text buffers in v46 Packages.bin"
    ))
}

fn find_v46_group_start(
    raw: &[u8],
    start: usize,
    package_count: u32,
    entity_count: u32,
) -> Result<usize> {
    fn validate(raw: &[u8], mut pos: usize, groups: u32, expected: u32) -> Result<()> {
        let mut children = 0u32;
        for _ in 0..groups {
            let (len, path_start) = read_u32(raw, pos)?;
            ensure!((1..1024).contains(&len), "invalid group path length");
            let path_end = path_start + len as usize;
            let path = raw
                .get(path_start..path_end)
                .context("truncated group path")?;
            ensure!(
                path.starts_with(b"/") && path.ends_with(b"/"),
                "invalid group path"
            );
            let (count, next) = read_u32(raw, path_end + 1)?;
            children = children
                .checked_add(count)
                .context("entity count overflow")?;
            ensure!(children <= expected, "too many grouped entities");
            pos = next;
            for _ in 0..count {
                let (len, name_start) = read_u32(raw, pos)?;
                ensure!(len <= 2048, "invalid entity name length");
                let (parent_len, parent_start) = read_u32(raw, name_start + len as usize + 3)?;
                ensure!(parent_len <= 4096, "invalid parent path length");
                pos = parent_start + parent_len as usize;
                ensure!(pos <= raw.len(), "truncated entity path");
            }
        }
        ensure!(children == expected, "grouped entity count mismatch");
        Ok(())
    }

    let end = start.saturating_add(256).min(raw.len().saturating_sub(4));
    for candidate in start..end {
        let (len, path_start) = read_u32(raw, candidate)?;
        if (1..1024).contains(&len)
            && raw.get(path_start) == Some(&b'/')
            && validate(raw, candidate, package_count, entity_count).is_ok()
        {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "Could not locate complete v46 entity groups in Packages.bin"
    ))
}

// ─── Cache extraction wrapper ─────────────────────────────────────────────

/// Extract a file from a cache pair by exact full path, with fallback to
/// trailing-name match (Python parity for `Packages.bin`, whose internal
/// entry path is just `"Packages.bin"`).
fn extract_file_by_path(
    oodle: &Oodle,
    cache_dir: &Path,
    prefix: &str,
    target_path: &str,
) -> Result<Option<Vec<u8>>> {
    let Some((entries, cache)) = load_local_cache(cache_dir, prefix)? else {
        return Ok(None);
    };
    let Some(entry) = find_file_entry(&entries, target_path) else {
        return Ok(None);
    };
    println!(
        "  {}: TOC record {}, timestamp {}, offset {}",
        entry.full_path, entry.index, entry.timestamp, entry.cache_offset
    );
    Ok(Some(cache_get_toc_entry(oodle, &cache, entry)?))
}

fn find_file_entry<'a>(entries: &'a [TocEntry], target_path: &str) -> Option<&'a TocEntry> {
    toc_find_path(entries, target_path).or_else(|| {
        let name = target_path.rsplit('/').next().unwrap_or(target_path);
        entries
            .iter()
            .filter(|e| !e.is_dir && e.name == name)
            .max_by_key(|e| (e.timestamp, e.index))
    })
}

// ─── Stage 1: Packages.bin -> packages_bin_attrs ──────────────────────────

pub fn parse_packages_bin(oodle: &Oodle, zstd: &Zstd, cache_dir: &Path) -> Result<PkgAttrs> {
    let raw = extract_file_by_path(oodle, cache_dir, "H.Misc", "/SF/Misc/Packages.bin")?
        .ok_or_else(|| anyhow!("Could not extract Packages.bin from H.Misc cache"))?;

    println!("  Packages.bin: {} bytes", raw.len());

    let mut pos: usize = 16;
    let (_header_size, p) = read_u32(&raw, pos)?;
    pos = p;
    let (version, p) = read_u32(&raw, pos)?;
    pos = p;
    let (_flags, p) = read_u32(&raw, pos)?;
    pos = p;

    if version >= 46 {
        // v46+: variable-length metadata block; scan forward for ref_count.
        let scan_end = (pos + 4096).min(raw.len().saturating_sub(16));
        let mut found = false;
        for test_pos in pos..scan_end {
            if test_pos + 9 > raw.len() {
                break;
            }
            let candidate = u32::from_le_bytes(raw[test_pos..test_pos + 4].try_into().unwrap());
            if (10..200).contains(&candidate) {
                let nl = u32::from_le_bytes(raw[test_pos + 4..test_pos + 8].try_into().unwrap());
                if (5..200).contains(&nl) && raw[test_pos + 8] == b'/' {
                    pos = test_pos;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Err(anyhow!("Could not locate ref_count in v46 Packages.bin"));
        }
    } else {
        pos += 4; // v44: skip 4 unknown bytes
    }

    let (ref_count, p) = read_u32(&raw, pos)?;
    pos = p;
    for _ in 0..ref_count {
        let (nl, p) = read_u32(&raw, pos)?;
        pos = p + nl as usize + 2;
    }

    let (pkg_count, p) = read_u32(&raw, pos)?;
    pos = p;

    let (com_flags_buf, com_size_buf, com_z_buf, _dict_size) = if version >= 46 {
        let buffers = find_v46_text_buffers(&raw, pos)?;
        pos = buffers.end;
        let (dict_size, _) = read_u32(buffers.sizes, 0)?;
        (
            buffers.flags.to_vec(),
            buffers.sizes.to_vec(),
            buffers.compressed.to_vec(),
            dict_size,
        )
    } else {
        let (fl, p) = read_u32(&raw, pos)?;
        pos = p;
        let flags_buf = raw[pos..pos + fl as usize].to_vec();
        pos += fl as usize;
        let (sl, p) = read_u32(&raw, pos)?;
        pos = p;
        let size_buf = raw[pos..pos + sl as usize].to_vec();
        pos += sl as usize;
        let (zl, p) = read_u32(&raw, pos)?;
        pos = p;
        let z_buf = raw[pos..pos + zl as usize].to_vec();
        pos += zl as usize;
        let (dict_size, _) = read_u32(&size_buf, 0)?;
        (flags_buf, size_buf, z_buf, dict_size)
    };

    let (dict_size, _) = read_u32(&com_size_buf, 0)?;
    let dict_size = dict_size as usize;
    let zstd_dict_data = &com_z_buf[..dict_size];
    let decoder = zstd.open_magicless_dict(zstd_dict_data);

    let (entity_count, p) = read_u32(&raw, pos)?;
    pos = p;
    println!("  Entity count: {}", entity_count);

    // Bit-packed text flags: one `has_text` bit per child, plus one
    // `is_compressed` bit for each child that has text. The stream starts at
    // bit 0 with no leading sentinel — the buffer length confirms it. For this
    // build: 70173 children + 26639 text-bearing = 96812 bits, against a 12102
    // byte (96816 bit) buffer, so only byte padding remains. Starting at bit 1
    // instead shifts every flag onto the wrong entity, which silently drops
    // entities whose bit lands on 0 and hands their attributes to a neighbour.
    let mut flags_pos: usize = 0;
    let mut flags_bit: u32 = 0;
    let mut flags_byte: u8 = com_flags_buf
        .first()
        .copied()
        .ok_or_else(|| anyhow!("empty flags buffer"))?;
    let mut size_pos: usize = 4; // skip dict_size header
    let mut z_pos: usize = dict_size;

    let mut entities_ordered: Vec<(String, PkgEntry)> = Vec::new();
    let mut ok = 0u32;
    let mut fail = 0u32;

    macro_rules! read_bit {
        () => {{
            let bit = (flags_byte >> flags_bit) & 1;
            flags_bit += 1;
            if flags_bit > 7 {
                flags_pos += 1;
                if flags_pos < com_flags_buf.len() {
                    flags_byte = com_flags_buf[flags_pos];
                }
                flags_bit -= 8;
            }
            bit
        }};
    }

    if version >= 46 {
        // Metadata before the first group is variable-length. Validate the
        // complete group/child counts, and start flags at bit zero for its
        // first child, just as in the flat format.
        pos = find_v46_group_start(&raw, pos, pkg_count, entity_count)?;

        for _ in 0..pkg_count {
            if pos + 8 > raw.len() {
                break;
            }
            let (gpl, p) = read_u32(&raw, pos)?;
            pos = p;
            let gpl = gpl as usize;
            if gpl == 0 || gpl >= 1024 || pos + gpl + 5 > raw.len() {
                break;
            }
            let (group_path, p) = read_ascii(&raw, pos, gpl)?;
            pos = p;
            if !group_path.starts_with('/') {
                break;
            }
            pos += 1; // group flag byte
            let (cc, p) = read_u32(&raw, pos)?;
            pos = p;

            for _ in 0..cc {
                if pos + 8 > raw.len() {
                    break;
                }
                let (fn_l, p) = read_u32(&raw, pos)?;
                pos = p;
                let fn_l = fn_l as usize;
                if fn_l > 2048 || pos + fn_l + 7 > raw.len() {
                    break;
                }
                let (file_name, p) = read_ascii(&raw, pos, fn_l)?;
                pos = p + 3;

                let (pt_l, p) = read_u32(&raw, pos)?;
                pos = p;
                let pt_l = pt_l as usize;
                if pt_l > 4096 || pos + pt_l > raw.len() {
                    break;
                }
                let (parent_type, p) = read_ascii(&raw, pos, pt_l)?;
                pos = p;

                let full_path = format!("{group_path}{file_name}");
                let has_text = read_bit!();
                let mut text_attrs = String::new();

                if has_text == 1 {
                    let (frame_size, sp2) = read_uleb128(&com_size_buf, size_pos)?;
                    size_pos = sp2;
                    let frame_data = &com_z_buf[z_pos..z_pos + frame_size];
                    let is_compressed = read_bit!();

                    if is_compressed == 1 {
                        let (decomp_len, content_start) = read_uleb128(frame_data, 0)?;
                        let compressed = &frame_data[content_start..];
                        match decoder.decompress(compressed, decomp_len) {
                            Ok(decoded) => {
                                text_attrs = String::from_utf8_lossy(&decoded).into_owned();
                                ok += 1;
                            }
                            Err(_) => fail += 1,
                        }
                    } else {
                        text_attrs = String::from_utf8_lossy(frame_data).into_owned();
                        ok += 1;
                    }
                    z_pos += frame_size;
                }

                let parent = if parent_type.starts_with('/') {
                    parent_type
                } else {
                    format!("{group_path}{parent_type}")
                };
                if !text_attrs.is_empty() {
                    entities_ordered.push((
                        full_path.clone(),
                        PkgEntry {
                            path: full_path,
                            parent,
                            attributes: text_attrs,
                        },
                    ));
                }
            }
        }
    } else {
        // v44 flat entity format.
        for _ in 0..entity_count {
            let (pkg_nl, p) = read_u32(&raw, pos)?;
            pos = p;
            let (pkg_name, p) = read_ascii(&raw, pos, pkg_nl as usize)?;
            pos = p;
            let (fn_l, p) = read_u32(&raw, pos)?;
            pos = p;
            let (file_name, p) = read_ascii(&raw, pos, fn_l as usize)?;
            pos = p + 3;
            let (pt_l, p) = read_u32(&raw, pos)?;
            pos = p;
            let (parent_type, p) = read_ascii(&raw, pos, pt_l as usize)?;
            pos = p;

            let full_path = format!("{pkg_name}{file_name}");
            let has_text = read_bit!();
            let mut text_attrs = String::new();

            if has_text == 1 {
                let (frame_size, sp2) = read_uleb128(&com_size_buf, size_pos)?;
                size_pos = sp2;
                let frame_data = &com_z_buf[z_pos..z_pos + frame_size];
                let is_compressed = read_bit!();

                if is_compressed == 1 {
                    let (decomp_len, content_start) = read_uleb128(frame_data, 0)?;
                    let compressed = &frame_data[content_start..];
                    match decoder.decompress(compressed, decomp_len) {
                        Ok(decoded) => {
                            text_attrs = String::from_utf8_lossy(&decoded).into_owned();
                            ok += 1;
                        }
                        Err(_) => fail += 1,
                    }
                } else {
                    text_attrs = String::from_utf8_lossy(frame_data).into_owned();
                    ok += 1;
                }
                z_pos += frame_size;
            }

            let parent = if parent_type.starts_with('/') {
                parent_type
            } else {
                format!("{pkg_name}{parent_type}")
            };
            if !text_attrs.is_empty() {
                entities_ordered.push((
                    full_path.clone(),
                    PkgEntry {
                        path: full_path,
                        parent,
                        attributes: text_attrs,
                    },
                ));
            }
        }
    }

    println!("  Decompressed OK: {ok}  /  Failed: {fail}");
    println!(
        "  Entities with text attributes: {}",
        entities_ordered.len()
    );
    let rotation = align_text_attributes(&mut entities_ordered);
    if rotation != AttributeRotation::None {
        println!("  Applied Packages.bin text-attribute rotation: {rotation:?}");
    }
    let entities: PkgAttrs = entities_ordered.into_iter().collect();
    Ok(entities)
}

// ─── Stage 2: build_crafting_recipes ──────────────────────────────────────

pub fn build_crafting_recipes(pkg_attrs: &PkgAttrs) -> Vec<Value> {
    static CATEGORY_TAGS: &[(&str, &str)] = &[
        ("/Armor/", "Armor"),
        ("/Weapons/", "Weapons"),
        ("/Melee/", "Weapons"),
        ("/Consumables/", "Consumables"),
        ("/Totems/", "Totems"),
        ("/Companions/", "Companions"),
        ("/Resources/", "Resources"),
        ("/Keys/", "Keys"),
        ("/Dojo/", "Dojo"),
        ("/Skins/", "Skins"),
        ("/Sigils/", "Sigils"),
        ("/Syandanas/", "Syandanas"),
    ];

    let re_ingredient = Regex::new(
        r"ItemType=(/[\w/]+)\s*\n\s*AlternativeItemTypes=\{([^}]*)\}\s*\n\s*ItemCount=(\d+)",
    )
    .unwrap();
    let re_build_time = Regex::new(r"BuildTime=(\d+)").unwrap();
    let re_build_price = Regex::new(r"BuildPrice=(\d+)").unwrap();
    let re_skip_price = Regex::new(r"SkipBuildTimePrice=(\d+)").unwrap();
    let re_result = Regex::new(r"ResultItem=(/[\w/]+)").unwrap();
    let re_rarity = Regex::new(r"Rarity=(\w+)").unwrap();
    let re_reg_price = Regex::new(r"RegularPrice=(\d+)").unwrap();
    let re_sell_price = Regex::new(r"SellingPrice=(\d+)").unwrap();
    let re_num_req = Regex::new(r"NumRequired=(\d+)").unwrap();
    let re_consume = Regex::new(r"ConsumeOnUse=(\d+)").unwrap();
    let re_icon = Regex::new(r"Icon=(/[\w/.]+)").unwrap();
    let re_loc_tag = Regex::new(r"LocalizeTag=(/[\w/]+)").unwrap();

    let mut recipes = Vec::new();
    // BTreeMap iteration is already sorted (Python: `sorted(pkg_attrs)`).
    for (path, entry) in pkg_attrs {
        let attrs = &entry.attributes;
        if !attrs.contains("Ingredient") {
            continue;
        }
        if !attrs.contains("BuildTime") && !attrs.contains("CraftTime") {
            continue;
        }

        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let category = CATEGORY_TAGS
            .iter()
            .find(|(tag, _)| path.contains(tag))
            .map(|(_, label)| *label)
            .unwrap_or("Other");

        let mut ingredients = Vec::new();
        for caps in re_ingredient.captures_iter(attrs) {
            let item_path = caps.get(1).unwrap().as_str();
            let alt_raw = caps.get(2).unwrap().as_str().trim();
            let count: i64 = caps.get(3).unwrap().as_str().parse().unwrap_or(0);
            let alts: Vec<String> = if alt_raw.is_empty() {
                Vec::new()
            } else {
                alt_raw
                    .split('\n')
                    .map(|s| s.trim().trim_end_matches(',').to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            ingredients.push(json!({
                "item": item_path.rsplit('/').next().unwrap_or(item_path),
                "item_path": item_path,
                "count": count,
                "alternatives": if alts.is_empty() { Value::Null } else { json!(alts) },
            }));
        }

        let cap_i64 = |re: &Regex| -> Option<i64> {
            re.captures(attrs)
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse().ok())
        };
        let cap_str = |re: &Regex| -> Option<String> {
            re.captures(attrs)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
        };

        let bt_sec = cap_i64(&re_build_time);
        let bt_human = bt_sec.map(|s| format!("{}h {}m", s / 3600, (s % 3600) / 60));
        let result_item = cap_str(&re_result);
        let result_name = result_item
            .as_ref()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string());
        let consume = cap_i64(&re_consume).map(|v| v != 0);

        recipes.push(json!({
            "name": name,
            "path": path,
            "parent": entry.parent,
            "category": category,
            "rarity": cap_str(&re_rarity),
            "result_item": result_item,
            "result_item_name": result_name,
            "ingredients": ingredients,
            "build_time_seconds": bt_sec,
            "build_time_human": bt_human,
            "build_cost": cap_i64(&re_build_price),
            "rush_cost_premium": cap_i64(&re_skip_price),
            "blueprint_price": cap_i64(&re_reg_price),
            "selling_price": cap_i64(&re_sell_price),
            "num_required": cap_i64(&re_num_req),
            "consume_on_use": consume,
            "icon": cap_str(&re_icon),
            "localize_tag": cap_str(&re_loc_tag),
        }));
    }

    recipes
}

// ─── Stage 3: extract_manifest_rewards ────────────────────────────────────

/// Pull every plaintext syndicate/recipe/reward manifest out of the `H.Misc`
/// cache and return `{full_path: plaintext_content}`.
pub fn extract_manifest_rewards(
    oodle: &Oodle,
    cache_dir: &Path,
) -> Result<BTreeMap<String, String>> {
    const TARGETS: &[&str] = &[
        "championsyndicatemanifest",
        "silentrosemanifest",
        "feysyndicatemanifest",
        "recipemanifest",
        "rewardmanifest",
        "itemmanifest",
        "vendormanifest",
        "manifest",
        "containerdroptable",
        "zonespawndroptable",
    ];

    let (entries, cache) = load_local_cache(cache_dir, "H.Misc")?
        .ok_or_else(|| anyhow!("H.Misc cache pair missing"))?;

    let mut results: BTreeMap<String, String> = BTreeMap::new();

    for entry in entries.iter().filter(|e| !e.is_dir) {
        let path_low = entry.full_path.to_lowercase();
        if !TARGETS.iter().any(|t| path_low.contains(t)) {
            continue;
        }

        let raw = match cache_get_toc_entry(oodle, &cache, entry) {
            Ok(b) if b.len() >= 20 => b,
            _ => continue,
        };

        // Try CommonHeader at offset 0, then at offset 16 (with 16-byte hash prefix).
        let text = parse_common_header_text(&raw, 0).or_else(|| parse_common_header_text(&raw, 16));

        if let Some(t) = text {
            // Sanity-check first 100 chars are mostly printable.
            let sample: Vec<char> = t.chars().take(100).collect();
            let printable = sample
                .iter()
                .filter(|&&c| (32u32..127).contains(&(c as u32)) || matches!(c, '\n' | '\r' | '\t'))
                .count();
            if !sample.is_empty() && printable * 10 >= sample.len() * 6 {
                // Python: store under `/`-prefixed full path key (already the case).
                let key = if entry.full_path.starts_with('/') {
                    entry.full_path.clone()
                } else {
                    format!("/{}", entry.full_path)
                };
                results.insert(key, t);
            }
        }
    }

    println!("  Manifest entries captured: {}", results.len());
    Ok(results)
}

fn parse_common_header_text(raw: &[u8], base: usize) -> Option<String> {
    if base + 4 > raw.len() {
        return None;
    }
    let src_count = u32::from_le_bytes(raw[base..base + 4].try_into().ok()?) as usize;
    if src_count > 10 {
        return None;
    }
    let mut off = base + 4;
    for _ in 0..src_count {
        if off + 4 > raw.len() {
            return None;
        }
        let pl = u32::from_le_bytes(raw[off..off + 4].try_into().ok()?) as usize;
        off += 4;
        if off + pl > raw.len() {
            return None;
        }
        off += pl;
    }
    if off + 4 > raw.len() {
        return None;
    }
    let content_len = u32::from_le_bytes(raw[off..off + 4].try_into().ok()?) as usize;
    off += 4;
    let avail = raw.len().saturating_sub(off);
    if content_len > avail + 100 {
        return None;
    }
    let take = content_len.min(avail);
    Some(String::from_utf8_lossy(&raw[off..off + take]).into_owned())
}

// ─── Orchestrator ─────────────────────────────────────────────────────────

/// Run all three intermediate-input generators end-to-end, writing the JSONs
/// expected by the items pipeline. Always extracts from the current game cache.
pub fn generate_items_inputs(
    packages_out: &Path,
    recipes_out: &Path,
    manifests_out: &Path,
) -> Result<()> {
    let cache_dir =
        detect_soulframe_cache().ok_or_else(|| anyhow!("Soulframe cache directory not found"))?;
    println!("  cache dir: {}", cache_dir.display());

    let oodle = Oodle::new().context("Loading Oodle")?;
    let zstd = Zstd::new().context("Loading libzstd")?;

    let pkg_attrs = parse_packages_bin(&oodle, &zstd, &cache_dir)?;
    write_json(packages_out, &pkg_attrs)?;
    println!("  wrote {}", packages_out.display());

    let recipes = build_crafting_recipes(&pkg_attrs);
    println!("  Recipes: {}", recipes.len());
    write_json(recipes_out, &recipes)?;
    println!("  wrote {}", recipes_out.display());

    let manifests = extract_manifest_rewards(&oodle, &cache_dir)?;
    write_json(manifests_out, &manifests)?;
    println!("  wrote {}", manifests_out.display());

    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = serde_json::to_string_pretty(value)?;
    std::fs::write(path, s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_buffer(raw: &mut Vec<u8>, data: &[u8]) {
        raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
        raw.extend_from_slice(data);
    }

    fn v46_text_fixture() -> Vec<u8> {
        let mut raw = Vec::new();
        // Metadata contains both the old heuristic's plausible length and a
        // decoy of the correct length whose text flags do not match the frames.
        raw.extend_from_slice(&20_549u32.to_le_bytes());
        push_buffer(&mut raw, &[0xff]);
        raw.extend_from_slice(&[0xcc; 3]);
        // Three children: no text, raw text, raw text. No leading flag bit.
        push_buffer(&mut raw, &[0b0000_1010]);
        raw.extend_from_slice(&[0xcc; 5]);
        let mut sizes = 16u32.to_le_bytes().to_vec();
        sizes.extend_from_slice(&[3, 0x82, 1]); // frame lengths 3 and 130
        push_buffer(&mut raw, &sizes);
        raw.extend_from_slice(&[0xcc; 11]);
        let mut compressed = vec![0; 16 + 3 + 130];
        compressed[..4].copy_from_slice(&[0x37, 0xa4, 0x30, 0xec]);
        push_buffer(&mut raw, &compressed);
        raw.extend_from_slice(&3u32.to_le_bytes());
        raw
    }

    #[test]
    fn v46_buffers_ignore_metadata_decoys_and_use_stored_dictionary_size() {
        let raw = v46_text_fixture();
        let buffers = find_v46_text_buffers(&raw, 0).unwrap();
        assert_eq!(buffers.flags, &[0b0000_1010]);
        assert_eq!(buffers.sizes, &[16, 0, 0, 0, 3, 0x82, 1]);
        assert_eq!(buffers.compressed.len(), 149);
        assert_eq!(buffers.end + 4, raw.len());
    }

    #[test]
    fn v46_buffers_reject_inconsistent_frame_lengths_and_truncation() {
        let mut raw = v46_text_fixture();
        let size_start =
            find_v46_text_buffers(&raw, 0).unwrap().sizes.as_ptr() as usize - raw.as_ptr() as usize;
        raw[size_start + 4] = 4; // frame total no longer fills the z buffer
        assert!(find_v46_text_buffers(&raw, 0).is_err());

        let raw = v46_text_fixture();
        assert!(find_v46_text_buffers(&raw[..raw.len() - 1], 0).is_err());
        assert!(find_v46_text_buffers(&[0x37, 0xa4, 0x30], 0).is_err());
    }

    fn v46_groups_fixture() -> Vec<u8> {
        let mut raw = vec![0x5f]; // current build has one metadata byte, not 12
        for (group, child) in [("/SF/Fx/", "SFSwarm"), ("/EE/Types/", "Template")] {
            push_buffer(&mut raw, group.as_bytes());
            raw.push(1);
            raw.extend_from_slice(&1u32.to_le_bytes());
            push_buffer(&mut raw, child.as_bytes());
            raw.extend_from_slice(&[0, 2, 1]);
            raw.extend_from_slice(&0u32.to_le_bytes()); // empty parent path
        }
        raw
    }

    #[test]
    fn v46_groups_keep_the_first_child_and_validate_all_counts() {
        let raw = v46_groups_fixture();
        assert_eq!(find_v46_group_start(&raw, 0, 2, 2).unwrap(), 1);
        assert!(find_v46_group_start(&raw, 0, 2, 3).is_err());
        assert!(find_v46_group_start(&raw[..raw.len() - 1], 0, 2, 2).is_err());
    }

    #[test]
    fn packages_name_fallback_selects_current_cache_record() {
        let record = |index, timestamp, full_path: &str| TocEntry {
            index,
            cache_offset: index as u64 * 100,
            timestamp,
            compressed_size: 16,
            decompressed_size: 16,
            reserved: 0,
            parent_index: 0,
            name: "Packages.bin".to_string(),
            full_path: full_path.to_string(),
            is_dir: false,
        };
        // Packages.bin is stored at the TOC root, so the requested /SF/Misc
        // path uses the filename fallback. Obsolete copies must not win.
        let mut entries = vec![
            record(1, 0, "Packages.bin"),
            record(2, 200, "Packages.bin"),
            record(3, 0, "Packages.bin"),
        ];
        let target = "/SF/Misc/Packages.bin";
        assert_eq!(find_file_entry(&entries, target).unwrap().index, 2);

        entries.push(record(4, 200, "Packages.bin"));
        assert_eq!(find_file_entry(&entries, target).unwrap().index, 4);

        // A real exact path still takes precedence over a basename match.
        entries.push(record(5, 100, target));
        assert_eq!(find_file_entry(&entries, target).unwrap().index, 5);
    }

    fn entry(path: &str, attrs: &str) -> (String, PkgEntry) {
        (
            path.to_string(),
            PkgEntry {
                path: path.to_string(),
                parent: "/SF/Types/Test".to_string(),
                attributes: attrs.to_string(),
            },
        )
    }

    #[test]
    fn aligned_attributes_are_not_rotated() {
        let mut ordered = vec![
            entry(
                "/SF/StoreItems/Weapons/Foo/FooWeapon",
                "LocalizeTag=/SF/Language/Weapons/FooWeaponName\nTypeName=/SF/Weapons/Foo/FooWeapon",
            ),
            entry(
                "/SF/StoreItems/Weapons/Bar/BarWeapon",
                "LocalizeTag=/SF/Language/Weapons/BarWeaponName\nTypeName=/SF/Weapons/Bar/BarWeapon",
            ),
            entry(
                "/SF/StoreItems/Weapons/Baz/BazWeapon",
                "LocalizeTag=/SF/Language/Weapons/BazWeaponName\nTypeName=/SF/Weapons/Baz/BazWeapon",
            ),
        ];

        assert_eq!(align_text_attributes(&mut ordered), AttributeRotation::None);
        assert!(ordered[0].1.attributes.contains("FooWeaponName"));
        assert!(ordered[1].1.attributes.contains("BarWeaponName"));
        assert!(ordered[2].1.attributes.contains("BazWeaponName"));
    }

    #[test]
    fn shifted_attributes_are_rotated_back_into_place() {
        let mut ordered = vec![
            entry(
                "/SF/StoreItems/Weapons/Foo/FooWeapon",
                "LocalizeTag=/SF/Language/Weapons/BarWeaponName\nTypeName=/SF/Weapons/Bar/BarWeapon",
            ),
            entry(
                "/SF/StoreItems/Weapons/Bar/BarWeapon",
                "LocalizeTag=/SF/Language/Weapons/BazWeaponName\nTypeName=/SF/Weapons/Baz/BazWeapon",
            ),
            entry(
                "/SF/StoreItems/Weapons/Baz/BazWeapon",
                "LocalizeTag=/SF/Language/Weapons/FooWeaponName\nTypeName=/SF/Weapons/Foo/FooWeapon",
            ),
        ];

        assert_eq!(
            align_text_attributes(&mut ordered),
            AttributeRotation::Previous
        );
        assert!(ordered[0].1.attributes.contains("FooWeaponName"));
        assert!(ordered[1].1.attributes.contains("BarWeaponName"));
        assert!(ordered[2].1.attributes.contains("BazWeaponName"));
    }
}
