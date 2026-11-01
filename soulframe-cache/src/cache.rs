//! `*.cache` file loading and per-entry decompression.

use anyhow::{anyhow, Context, Result};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::oodle::Oodle;
use crate::toc::{toc_decode, TocEntry};

/// A `<base_name>.toc` parsed into memory alongside its `<base_name>.cache`
/// left open on disk.
///
/// Cache bodies reach several GB each (`F.Texture.cache` is ~8 GB), so holding
/// them in `Vec<u8>` costs more memory than the machine has once more than one
/// set is open. [`LazyCache`] keeps only the TOC resident and reads each
/// entry's bytes on demand.
pub struct LazyCache {
    /// Decoded TOC entries, in stream order.
    pub entries: Vec<TocEntry>,
    file: File,
    len: u64,
}

impl LazyCache {
    /// Open a `<base_name>.toc` + `<base_name>.cache` pair, reading only the TOC.
    ///
    /// Returns `Ok(None)` if the `.toc` file does not exist or is empty
    /// (typical for cache pairs absent from a given installation).
    pub fn open(cache_path: &Path, base_name: &str) -> Result<Option<Self>> {
        let toc_path = cache_path.join(format!("{}.toc", base_name));
        if !toc_path.exists() {
            return Ok(None);
        }

        let toc_bin = fs::read(&toc_path)
            .with_context(|| format!("Failed to read TOC file {}", toc_path.display()))?;
        if toc_bin.is_empty() {
            return Ok(None);
        }
        let entries = toc_decode(&toc_bin)?;

        let cache_file_path = cache_path.join(format!("{}.cache", base_name));
        let file = File::open(&cache_file_path)
            .with_context(|| format!("Failed to open cache file {}", cache_file_path.display()))?;
        let len = file
            .metadata()
            .with_context(|| format!("Failed to stat {}", cache_file_path.display()))?
            .len();

        Ok(Some(Self { entries, file, len }))
    }

    /// Total size of the backing `.cache` file, for reporting only.
    pub fn cache_len(&self) -> u64 {
        self.len
    }

    /// Decompress one entry, reading only that entry's bytes from disk.
    ///
    /// Equivalent to [`cache_get_toc_entry`] against a fully loaded cache
    /// buffer, but touches `compressed_size` bytes instead of the whole file.
    pub fn get(&self, oodle: &Oodle, entry: &TocEntry) -> Result<Vec<u8>> {
        if entry.cache_offset >= self.len {
            return Err(anyhow!("Cache offset beyond data"));
        }

        // Both framings are self-delimiting within `compressed_size` bytes: an
        // uncompressed entry stores compressed_size == decompressed_size, and
        // SHCC block headers are counted inside compressed_size.
        let want = entry.compressed_size as u64;
        let avail = self.len - entry.cache_offset;
        let mut buf = vec![0u8; want.min(avail) as usize];
        read_exact_at(&self.file, &mut buf, entry.cache_offset)?;

        // The buffer starts at the entry, so re-base the offset onto it.
        let mut local = entry.clone();
        local.cache_offset = 0;
        cache_get_toc_entry(oodle, &buf, &local)
    }
}

/// Read `buf.len()` bytes at `offset` without disturbing the file cursor, so
/// callers can share one `&File` across entries.
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;

    let mut done = 0usize;
    while done < buf.len() {
        #[cfg(windows)]
        let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
        #[cfg(unix)]
        let n = file.read_at(&mut buf[done..], offset + done as u64)?;

        if n == 0 {
            return Err(anyhow!(
                "Short read: wanted {} bytes at offset {}, got {}",
                buf.len(),
                offset,
                done
            ));
        }
        done += n;
    }
    Ok(())
}

/// Load a `<base_name>.toc` + `<base_name>.cache` pair from disk.
///
/// Returns `Ok(None)` if the `.toc` file does not exist (typical for cache
/// pairs that aren't present in a given installation), or `Ok(Some(...))`
/// with the parsed entries and the raw cache bytes.
pub fn load_local_cache(
    cache_path: &Path,
    base_name: &str,
) -> Result<Option<(Vec<TocEntry>, Vec<u8>)>> {
    let toc_path = cache_path.join(format!("{}.toc", base_name));
    let cache_file_path = cache_path.join(format!("{}.cache", base_name));

    if !toc_path.exists() {
        return Ok(None);
    }

    let toc_bin = fs::read(&toc_path)
        .with_context(|| format!("Failed to read TOC file {}", toc_path.display()))?;

    let cache_bin = fs::read(&cache_file_path)
        .with_context(|| format!("Failed to read cache file {}", cache_file_path.display()))?;

    if toc_bin.is_empty() {
        return Ok(None);
    }

    let entries = toc_decode(&toc_bin)?;
    Ok(Some((entries, cache_bin)))
}

/// Extract and decompress the bytes for a single TOC entry from a cache buffer.
///
/// Handles three on-disk shapes transparently:
///
/// - **Uncompressed**: when `compressed_size == decompressed_size`, the
///   `decompressed_size` bytes at `cache_offset` are returned verbatim.
/// - **SHCC-wrapped Oodle**: when the byte at `cache_offset` is `0x80`, the
///   entry is one or more SHCC-framed Oodle blocks (see
///   [`shcc_decompress_chunk_oodle`]).
/// - **Raw Oodle**: otherwise, `compressed_size` bytes are passed directly
///   to Oodle.
pub fn cache_get_toc_entry(oodle: &Oodle, cache: &[u8], entry: &TocEntry) -> Result<Vec<u8>> {
    let start = entry.cache_offset as usize;

    if entry.decompressed_size == entry.compressed_size {
        let end = start
            .checked_add(entry.decompressed_size as usize)
            .ok_or_else(|| anyhow!("Cache entry offset+size overflow"))?;
        if end > cache.len() {
            return Err(anyhow!("Cache entry extends beyond data"));
        }
        return Ok(cache[start..end].to_vec());
    }

    if start >= cache.len() {
        return Err(anyhow!("Cache offset beyond data"));
    }

    if cache[start] == 0x80 {
        let (data, _) =
            shcc_decompress_chunk_oodle(oodle, cache, start, entry.decompressed_size as usize)?;
        Ok(data)
    } else {
        let end = start
            .checked_add(entry.compressed_size as usize)
            .ok_or_else(|| anyhow!("Compressed entry offset+size overflow"))?;
        if end > cache.len() {
            return Err(anyhow!("Compressed data extends beyond cache"));
        }

        let compressed_data = &cache[start..end];
        let mut decompressed =
            oodle.decompress(compressed_data, entry.decompressed_size as usize)?;

        if decompressed.len() < entry.decompressed_size as usize {
            decompressed.resize(entry.decompressed_size as usize, 0);
        }

        Ok(decompressed)
    }
}

/// Decompress an SHCC-framed sequence of Oodle blocks.
///
/// The SHCC framing wraps one or more Oodle-compressed blocks. Each block is
/// preceded by an 8-byte info header (first byte `0x80`, low 4 bits of last
/// byte `0x01`), followed by a `0x8C` marker byte, then the compressed
/// payload of length encoded in bits of the header.
/// Blocks whose encoded and decoded lengths are equal contain literal bytes
/// instead, with no Oodle marker. A single entry can contain both kinds.
///
/// Returns the concatenated decompressed bytes and the byte position in
/// `bin` immediately after the last consumed block.
pub fn shcc_decompress_chunk_oodle(
    oodle: &Oodle,
    bin: &[u8],
    i: usize,
    decompressed_size: usize,
) -> Result<(Vec<u8>, usize)> {
    shcc_decompress_chunk(bin, i, decompressed_size, |data, size| {
        oodle.decompress(data, size)
    })
}

fn shcc_decompress_chunk(
    bin: &[u8],
    mut i: usize,
    decompressed_size: usize,
    mut decompress: impl FnMut(&[u8], usize) -> Result<Vec<u8>>,
) -> Result<(Vec<u8>, usize)> {
    let mut decompressed = Vec::with_capacity(decompressed_size);

    while decompressed.len() < decompressed_size {
        if i + 8 > bin.len() {
            break;
        }

        let block_info = &bin[i..i + 8];
        i += 8;

        if block_info[0] != 0x80 {
            return Err(anyhow!(
                "Expected SHCC byte 1 to be 0x80, got 0x{:02X}",
                block_info[0]
            ));
        }

        if (block_info[7] & 0x0F) != 0x01 {
            return Err(anyhow!(
                "Expected SHCC byte 8 & 0x0F to be 0x01, got 0x{:02X}",
                block_info[7] & 0x0F
            ));
        }

        let num1 = ((block_info[0] as u32) << 24)
            | ((block_info[1] as u32) << 16)
            | ((block_info[2] as u32) << 8)
            | (block_info[3] as u32);
        let num2 = ((block_info[4] as u32) << 24)
            | ((block_info[5] as u32) << 16)
            | ((block_info[6] as u32) << 8)
            | (block_info[7] as u32);

        let block_compressed_size = ((num1 >> 2) & 0xFFFFFF) as usize;
        let block_decompressed_size = ((num2 >> 5) & 0xFFFFFF) as usize;

        if i + block_compressed_size > bin.len() {
            return Err(anyhow!("SHCC block extends beyond data"));
        }

        let compressed_block = &bin[i..i + block_compressed_size];
        let mut part = if block_compressed_size == block_decompressed_size {
            // SHCC can mix Oodle blocks with stored blocks. Stored tails have
            // no Oodle marker, e.g. MothIdleLoopA's final 83,336 audio bytes.
            compressed_block.to_vec()
        } else {
            if compressed_block.first() != Some(&0x8C) {
                return Err(anyhow!("Expected SHCC 0x8C marker"));
            }
            decompress(compressed_block, block_decompressed_size)?
        };

        if part.len() < block_decompressed_size {
            part.resize(block_decompressed_size, 0);
        }

        decompressed.extend_from_slice(&part);
        i += block_compressed_size;
    }

    if decompressed.len() < decompressed_size {
        decompressed.resize(decompressed_size, 0);
    } else if decompressed.len() > decompressed_size {
        decompressed.truncate(decompressed_size);
    }

    Ok((decompressed, i))
}

/// Locate the default Soulframe cache directory on this machine.
///
/// `SOULFRAME_ROOT` overrides the install location (point it at the
/// `Downloaded\Public` folder, e.g. a Steam library). Otherwise this is
/// `%LOCALAPPDATA%\Soulframe\Downloaded\Public\Cache.Windows`.
/// Returns `None` if the directory does not exist.
pub fn detect_soulframe_cache() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("SOULFRAME_ROOT") {
        let cache_path = PathBuf::from(root).join("Cache.Windows");
        return cache_path.is_dir().then_some(cache_path);
    }

    let local_appdata = std::env::var("LOCALAPPDATA").ok()?;
    let cache_path = PathBuf::from(local_appdata)
        .join("Soulframe")
        .join("Downloaded")
        .join("Public")
        .join("Cache.Windows");

    if cache_path.is_dir() {
        Some(cache_path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn shcc_accepts_stored_tail_after_compressed_block_and_keeps_boundaries() {
        fn block(out: &mut Vec<u8>, payload: &[u8], size: usize) {
            out.extend_from_slice(&(0x8000_0000u32 | ((payload.len() as u32) << 2)).to_be_bytes());
            out.extend_from_slice(&(((size as u32) << 5) | 1).to_be_bytes());
            out.extend_from_slice(payload);
        }
        let mut raw = vec![99; 3];
        block(&mut raw, &[0x8c, 7], 4);
        block(&mut raw, &[1, 2, 3], 3);
        raw.extend_from_slice(&[0xaa, 0xbb]); // the next cache record
        let mut calls = 0;
        let (decoded, end) = shcc_decompress_chunk(&raw, 3, 7, |data, size| {
            calls += 1;
            assert_eq!(data, &[0x8c, 7]);
            assert_eq!(size, 4);
            Ok(vec![5; 4])
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(decoded, &[5, 5, 5, 5, 1, 2, 3]);
        assert_eq!(&raw[end..], &[0xaa, 0xbb]);
        assert!(shcc_decompress_chunk(&raw[..end - 1], 3, 7, |_, _| Ok(vec![5; 4])).is_err());
    }

    /// [`LazyCache::get`] reads `compressed_size` bytes at `cache_offset` and
    /// then decodes them as if they began at offset 0. That window must match
    /// exactly what a fully loaded cache buffer would have sliced — if it
    /// drifts, every entry decompresses from the wrong bytes.
    #[test]
    fn read_exact_at_matches_whole_buffer_slice() {
        let whole: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let dir = std::env::temp_dir().join(format!("sfcache-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.cache");
        File::create(&path).unwrap().write_all(&whole).unwrap();

        let file = File::open(&path).unwrap();
        for (offset, len) in [(0u64, 16usize), (100, 300), (4000, 96)] {
            let mut buf = vec![0u8; len];
            read_exact_at(&file, &mut buf, offset).unwrap();
            let start = offset as usize;
            assert_eq!(buf, whole[start..start + len], "offset {offset} len {len}");
        }

        // Running past EOF must fail loudly rather than yield zero padding.
        let mut over = vec![0u8; 64];
        assert!(read_exact_at(&file, &mut over, 4090).is_err());

        fs::remove_dir_all(&dir).ok();
    }
}
