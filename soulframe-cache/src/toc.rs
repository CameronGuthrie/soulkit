//! TOC (Table of Contents) parsing for Soulframe `*.toc` files.

use anyhow::{anyhow, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read};

/// A single entry in a `*.toc` file.
///
/// Each `*.toc` describes a forest of directories and files; the corresponding
/// `*.cache` file holds the (often compressed) contents pointed to by each
/// non-directory entry's [`cache_offset`](TocEntry::cache_offset).
#[derive(Debug, Clone)]
pub struct TocEntry {
    /// 1-based position of this entry in the TOC stream.
    pub index: usize,
    /// Byte offset into the matching `*.cache` file, or `u64::MAX` for directories.
    pub cache_offset: u64,
    pub timestamp: u64,
    pub compressed_size: u32,
    pub decompressed_size: u32,
    pub reserved: u32,
    /// Index into the directories-only sub-list (NOT the raw entry list).
    /// `0` is the synthetic root directory.
    pub parent_index: u32,
    pub name: String,
    /// Pre-resolved full path, e.g. `"SF/Interface/Graphics/Aspects/Universal/Charge.png"`.
    pub full_path: String,
    /// `true` when [`cache_offset`](TocEntry::cache_offset) is `u64::MAX`.
    pub is_dir: bool,
}

/// Decode a `*.toc` binary blob into a flat list of entries with resolved paths.
///
/// # Format
///
/// The TOC starts with the 4-byte magic `4E C6 67 18`, then 4 bytes of
/// padding, then a stream of fixed-size entries. Each entry is:
///
/// | offset | size | field             |
/// |-------:|-----:|-------------------|
/// |      0 |    8 | `cache_offset`    |
/// |      8 |    8 | `timestamp`       |
/// |     16 |    4 | `compressed_size` |
/// |     20 |    4 | `decompressed_size`|
/// |     24 |    4 | `reserved`        |
/// |     28 |    4 | `parent_index`    |
/// |     32 |   64 | `name` (NUL-terminated UTF-8) |
///
/// # `parent_index` semantics
///
/// `parent_index` is an index into the directory tree by *directory ordinal*
/// (i.e. into the subset of entries whose `cache_offset == u64::MAX`), with
/// `0` representing the synthetic root. This matches LotusLib's
/// `DirectoryTree.cpp`. Treating it as an index into the raw entry list — as
/// a naïve reading of the format suggests — silently drops children of any
/// directory whose directory-ordinal differs from its raw entry index, which
/// is the common case for deeper or later-added folders.
pub fn toc_decode(bin: &[u8]) -> Result<Vec<TocEntry>> {
    const MAGIC: [u8; 4] = [0x4E, 0xC6, 0x67, 0x18];
    const ENTRY_SIZE: usize = 8 + 8 + 4 + 4 + 4 + 4 + 64;
    const HEADER_SIZE: u64 = 8;

    if bin.len() < HEADER_SIZE as usize || bin[..4] != MAGIC {
        return Err(anyhow!("Invalid TOC: unexpected header"));
    }

    let mut cursor = Cursor::new(bin);
    cursor.set_position(HEADER_SIZE);

    // dir_paths[0] is the root (""). Subsequent entries are appended in the
    // order directory entries appear in the TOC, matching LotusLib's
    // realDirCount counter.
    let mut dir_paths: Vec<String> = vec![String::new()];
    let mut entries: Vec<TocEntry> = Vec::new();
    let mut raw_index: usize = 1;

    while (cursor.position() as usize + ENTRY_SIZE) <= bin.len() {
        let cache_offset = cursor.read_u64::<LittleEndian>()?;
        let timestamp = cursor.read_u64::<LittleEndian>()?;
        let compressed_size = cursor.read_u32::<LittleEndian>()?;
        let decompressed_size = cursor.read_u32::<LittleEndian>()?;
        let reserved = cursor.read_u32::<LittleEndian>()?;
        let parent_index = cursor.read_u32::<LittleEndian>()?;

        let mut name_field = [0u8; 64];
        cursor.read_exact(&mut name_field)?;

        let zero_pos = name_field
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| anyhow!("Invalid TOC: missing null-terminated name"))?;
        let name = String::from_utf8_lossy(&name_field[..zero_pos]).into_owned();

        let parent_path = dir_paths
            .get(parent_index as usize)
            .cloned()
            .unwrap_or_default();
        let full_path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", parent_path, name)
        };

        let is_dir = cache_offset == u64::MAX;
        if is_dir {
            dir_paths.push(full_path.clone());
        }

        entries.push(TocEntry {
            index: raw_index,
            cache_offset,
            timestamp,
            compressed_size,
            decompressed_size,
            reserved,
            parent_index,
            name,
            full_path,
            is_dir,
        });

        raw_index += 1;
    }

    if entries.is_empty() {
        return Err(anyhow!("Invalid TOC: no entries"));
    }

    Ok(entries)
}

/// Get the full path of a TOC entry by its 1-based raw index.
///
/// Returns the empty string for `index == 0` (root) or out-of-range indices.
pub fn toc_get_path(entries: &[TocEntry], index: usize) -> String {
    if index == 0 {
        return String::new();
    }
    entries
        .get(index - 1)
        .map(|e| e.full_path.clone())
        .unwrap_or_default()
}

/// Find the newest non-directory TOC entry by its full path.
///
/// Updates retain obsolete records (often with a zero timestamp). Prefer the
/// newest timestamp, then the later TOC record when timestamps are equal.
/// Keep all records in `toc_decode` so raw entry indices remain valid.
pub fn toc_find_path<'a>(entries: &'a [TocEntry], path: &str) -> Option<&'a TocEntry> {
    entries
        .iter()
        .filter(|e| !e.is_dir && e.full_path == path)
        .max_by_key(|e| (e.timestamp, e.index))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal in-memory TOC containing one directory and one file.
    fn build_minimal_toc() -> Vec<u8> {
        let mut buf = Vec::new();
        // Magic + 4 padding bytes.
        buf.extend_from_slice(&[0x4E, 0xC6, 0x67, 0x18, 0, 0, 0, 0]);

        // Entry 1: directory "SF" under root (parent_index = 0).
        push_entry(&mut buf, u64::MAX, 0, 0, 0, 0, 0, "SF");
        // Entry 2: file "Hello.bin" under "SF". The directory-ordinal of "SF"
        // is 1 (root is 0), so parent_index = 1.
        push_entry(&mut buf, 0, 0, 16, 16, 0, 1, "Hello.bin");

        buf
    }

    fn push_entry(
        buf: &mut Vec<u8>,
        cache_offset: u64,
        timestamp: u64,
        compressed_size: u32,
        decompressed_size: u32,
        reserved: u32,
        parent_index: u32,
        name: &str,
    ) {
        buf.extend_from_slice(&cache_offset.to_le_bytes());
        buf.extend_from_slice(&timestamp.to_le_bytes());
        buf.extend_from_slice(&compressed_size.to_le_bytes());
        buf.extend_from_slice(&decompressed_size.to_le_bytes());
        buf.extend_from_slice(&reserved.to_le_bytes());
        buf.extend_from_slice(&parent_index.to_le_bytes());

        let mut name_field = [0u8; 64];
        let name_bytes = name.as_bytes();
        assert!(name_bytes.len() < 64, "test name too long");
        name_field[..name_bytes.len()].copy_from_slice(name_bytes);
        buf.extend_from_slice(&name_field);
    }

    #[test]
    fn rejects_empty_input() {
        assert!(toc_decode(&[]).is_err());
    }

    #[test]
    fn rejects_bad_magic() {
        let bad = vec![0; 256];
        assert!(toc_decode(&bad).is_err());
    }

    #[test]
    fn decodes_dir_and_file_with_paths() {
        let toc = build_minimal_toc();
        let entries = toc_decode(&toc).unwrap();
        assert_eq!(entries.len(), 2);

        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "SF");
        assert_eq!(entries[0].full_path, "SF");

        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].name, "Hello.bin");
        assert_eq!(entries[1].full_path, "SF/Hello.bin");
    }

    #[test]
    fn toc_find_path_returns_file_not_dir() {
        let toc = build_minimal_toc();
        let entries = toc_decode(&toc).unwrap();

        assert!(toc_find_path(&entries, "SF/Hello.bin").is_some());
        // Directories are filtered out.
        assert!(toc_find_path(&entries, "SF").is_none());
    }

    #[test]
    fn toc_find_path_prefers_current_record_over_obsolete_duplicates() {
        let mut toc = build_minimal_toc();
        push_entry(&mut toc, 100, 200, 16, 16, 0, 1, "Hello.bin");
        // An obsolete record can also occur after the current record.
        push_entry(&mut toc, 200, 0, 16, 16, 0, 1, "Hello.bin");
        let entries = toc_decode(&toc).unwrap();

        assert_eq!(
            toc_find_path(&entries, "SF/Hello.bin")
                .unwrap()
                .cache_offset,
            100
        );
        assert_eq!(entries.len(), 4);
        assert_eq!(toc_get_path(&entries, 4), "SF/Hello.bin");
    }

    #[test]
    fn toc_find_path_breaks_timestamp_ties_by_record_index() {
        for timestamp in [0, 200] {
            let mut toc = build_minimal_toc();
            push_entry(&mut toc, 100, timestamp, 16, 16, 0, 1, "Hello.bin");
            push_entry(&mut toc, 200, timestamp, 16, 16, 0, 1, "Hello.bin");
            let mut entries = toc_decode(&toc).unwrap();
            entries.reverse();
            assert_eq!(
                toc_find_path(&entries, "SF/Hello.bin")
                    .unwrap()
                    .cache_offset,
                200
            );
        }
    }

    #[test]
    fn toc_get_path_handles_root_and_oob() {
        let toc = build_minimal_toc();
        let entries = toc_decode(&toc).unwrap();
        assert_eq!(toc_get_path(&entries, 0), "");
        assert_eq!(toc_get_path(&entries, 1), "SF");
        assert_eq!(toc_get_path(&entries, 2), "SF/Hello.bin");
        assert_eq!(toc_get_path(&entries, 99), "");
    }
}
