//! Soulframe language pack download/extract pipeline.
//!
//! Ported from `soulframe-language-downloader-rust`. Uses the shared
//! [`soulframe_cache::Oodle`] loader; everything else here is CDN-format
//! specific (SHCC wire framing) or ZSTD dictionary decompression that the
//! Rust `zstd` crate doesn't expose ergonomically.

pub mod cli;

use anyhow::{anyhow, Context, Result};
use base64::prelude::*;
use libloading::{Library, Symbol};
use md5::{Digest, Md5};
use soulframe_cache::Oodle;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};

pub const TYPE_MANIFEST: u8 = 0xE;
pub const TYPE_BIN: u8 = 0x2C;

pub fn get_download_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("downloaded-data")
        .join(format!("0{}{}", suffix, path))
}

pub fn get_extract_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("extracted-data")
        .join(format!("0{}{}", suffix, path))
}

/// URL-safe base64 without padding (Soulframe's "b64m" variant: `/` → `-`).
pub fn b64m_encode(data: &[u8]) -> String {
    BASE64_STANDARD_NO_PAD.encode(data).replace('/', "-")
}

pub fn b64m_decode(data: &str) -> Result<Vec<u8>> {
    let normalized = data.replace('-', "/");
    BASE64_STANDARD_NO_PAD
        .decode(normalized)
        .map_err(|e| anyhow!("Base64 decode error: {}", e))
}

// ─── SHCC wire-format unpack ──────────────────────────────────────────────
//
// This is the CDN delivery wrapper, distinct from the local cache format that
// `soulframe-cache` handles. An SHCC blob holds an `H` chunk and an optional
// `B` chunk; each chunk is either uncompressed (type 0) or Oodle (type 2).

#[derive(Debug, Clone)]
pub struct ShccData {
    pub h: Vec<u8>,
    pub b: Option<Vec<u8>>,
    pub b_raw: Option<Vec<u8>>,
}

fn shcc_decompress_oodle_blocks(
    bin: &[u8],
    start: usize,
    decompressed_size: usize,
    oodle: &Oodle,
) -> Result<(Vec<u8>, usize)> {
    // Reuse the shared shcc framing helper from soulframe-cache.
    soulframe_cache::shcc_decompress_chunk_oodle(oodle, bin, start, decompressed_size)
}

pub fn shcc_decompress_chunk(bin: &[u8], start: usize, oodle: &Oodle) -> Result<(Vec<u8>, usize)> {
    if start + 9 > bin.len() {
        return Err(anyhow!("Not enough data for SHCC chunk header"));
    }

    let chunk_type = bin[start];
    let decompressed_size = u32::from_le_bytes([
        bin[start + 1],
        bin[start + 2],
        bin[start + 3],
        bin[start + 4],
    ]) as usize;
    let compressed_size = u32::from_le_bytes([
        bin[start + 5],
        bin[start + 6],
        bin[start + 7],
        bin[start + 8],
    ]) as usize;

    let mut i = start + 9;

    match chunk_type {
        0 => {
            if compressed_size != decompressed_size {
                return Err(anyhow!("Compressed size mismatch for uncompressed chunk"));
            }
            if i + compressed_size > bin.len() {
                return Err(anyhow!("Not enough data for uncompressed chunk"));
            }
            let data = bin[i..i + compressed_size].to_vec();
            i += decompressed_size;
            Ok((data, i))
        }
        2 => shcc_decompress_oodle_blocks(bin, i, decompressed_size, oodle),
        _ => Err(anyhow!("Unknown chunk type: {}", chunk_type)),
    }
}

pub fn shcc_unpack(bin: &[u8], oodle: &Oodle) -> Result<ShccData> {
    if bin.len() < 8 {
        return Err(anyhow!("SHCC data too short"));
    }

    let mut i = 8; // Skip initial 8-byte SHCC header.

    let (h_data, new_i) = shcc_decompress_chunk(bin, i, oodle)?;
    i = new_i;

    let mut b_data = None;
    let mut b_raw = None;
    if i < bin.len() {
        let b_start = i;
        if let Ok((b, _)) = shcc_decompress_chunk(bin, i, oodle) {
            b_data = Some(b);
            if b_start + 9 < bin.len() && bin.len() >= 15 {
                b_raw = Some(bin[b_start + 9..bin.len() - 15].to_vec());
            }
        }
    }

    Ok(ShccData {
        h: h_data,
        b: b_data,
        b_raw,
    })
}

/// SHCC payload hash used by some downstream callers (matches Python ref impl).
pub fn shcc_hash(data: &ShccData) -> Vec<u8> {
    let mut hasher = Md5::new();
    hasher.update(b"SHCC\x1F\x00\x00\x00");
    if data.h.len() >= 17 {
        hasher.update(&data.h[16..]);
    }
    if let Some(ref b_raw) = data.b_raw {
        hasher.update(b_raw);
    }
    hasher.finalize().to_vec()
}

/// Variable-length LE u32 decoder used by both manifest and language formats.
pub fn unpack_u32_dyn_le(bin: &[u8], start: usize) -> Result<(u32, usize)> {
    let mut value = 0u32;
    let mut i = start;
    let mut shift = 0u32;

    while shift < 28 {
        if i >= bin.len() {
            return Err(anyhow!("Unexpected end of data in dynamic u32"));
        }
        let byte = bin[i];
        i += 1;
        value |= ((byte & 0x7f) as u32) << shift;
        if (byte & 0x80) == 0 {
            return Ok((value, i));
        }
        shift += 7;
    }

    if i >= bin.len() {
        return Err(anyhow!("Unexpected end of data in dynamic u32 final byte"));
    }
    let byte = bin[i];
    i += 1;
    if byte > 0xF {
        return Err(anyhow!("Invalid final byte in dynamic u32: {}", byte));
    }
    value |= (byte as u32) << shift;
    Ok((value, i))
}

// ─── Cache manifest (H.Cache.bin, B.Cache.Windows_XX.bin) ────────────────

/// Parser for the binary manifest format used by `H.Cache.bin` and
/// `B.Cache.Windows*.bin`.
///
/// H data layout after SHCC unpacking:
/// - 20-byte header
/// - Repeating groups: 4-byte entry count, then N entries
/// - Each entry: 4-byte path length + path string + 16-byte MD5 + 4-byte unknown
pub struct CacheManifest {
    bin: Vec<u8>,
    i: usize,
    remaining_entries: u32,
    hashes: HashMap<String, Vec<u8>>,
    paths: Vec<String>,
}

impl CacheManifest {
    pub fn new(h_data: Vec<u8>) -> Self {
        Self {
            bin: h_data,
            i: 20,
            remaining_entries: 0,
            hashes: HashMap::new(),
            paths: Vec::new(),
        }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    fn seek(&mut self, stop_at: Option<&str>) -> Option<Vec<u8>> {
        while self.i < self.bin.len() {
            while self.remaining_entries == 0 {
                if self.i + 4 > self.bin.len() {
                    return None;
                }
                self.remaining_entries = u32::from_le_bytes([
                    self.bin[self.i],
                    self.bin[self.i + 1],
                    self.bin[self.i + 2],
                    self.bin[self.i + 3],
                ]);
                self.i += 4;
            }

            self.remaining_entries -= 1;

            if self.i + 4 > self.bin.len() {
                return None;
            }
            let path_len = u32::from_le_bytes([
                self.bin[self.i],
                self.bin[self.i + 1],
                self.bin[self.i + 2],
                self.bin[self.i + 3],
            ]) as usize;
            self.i += 4;

            if self.i + path_len + 20 > self.bin.len() {
                return None;
            }
            let path = String::from_utf8_lossy(&self.bin[self.i..self.i + path_len]).to_string();
            self.i += path_len;

            let hash = self.bin[self.i..self.i + 16].to_vec();
            self.i += 20; // 16 hash + 4 unknown

            self.paths.push(path.clone());
            self.hashes.insert(path.clone(), hash.clone());

            if let Some(target) = stop_at {
                if path == target {
                    return Some(hash);
                }
            }
        }
        None
    }

    pub fn get_hash(&mut self, path: &str) -> Option<Vec<u8>> {
        if let Some(h) = self.hashes.get(path) {
            return Some(h.clone());
        }
        self.seek(Some(path))
    }

    pub fn parse_all(&mut self) -> usize {
        self.seek(None);
        self.paths.len()
    }
}

// ─── ZSTD with dictionary support ────────────────────────────────────────
//
// Loaded dynamically from `lib/libzstd.dll` because the `zstd` crate doesn't
// expose `ZSTD_decompress_usingDDict` in a way that's convenient for our
// dynamic-dict-per-file usage pattern.

pub struct Zstd {
    #[allow(dead_code)]
    lib: Library,
    create_ddict: Symbol<'static, unsafe extern "C" fn(*const c_char, usize) -> usize>,
    create_dctx: Symbol<'static, unsafe extern "C" fn() -> usize>,
    dctx_set_parameter: Symbol<'static, unsafe extern "C" fn(usize, c_int, c_int) -> usize>,
    decompress_using_ddict: Symbol<
        'static,
        unsafe extern "C" fn(usize, *mut c_void, usize, *const c_char, usize, usize) -> usize,
    >,
    free_dctx: Symbol<'static, unsafe extern "C" fn(usize) -> usize>,
    free_ddict: Symbol<'static, unsafe extern "C" fn(usize) -> usize>,
}

impl Zstd {
    /// Use `SOULFRAME_ZSTD_LIB` when set; otherwise search `lib/` and the
    /// directory beside the executable, then the same locations at the cwd.
    pub fn new() -> Result<Self> {
        let override_path = std::env::var_os("SOULFRAME_ZSTD_LIB");
        Self::from_override(override_path.as_deref().map(Path::new))
    }

    fn from_override(override_path: Option<&Path>) -> Result<Self> {
        let lib_name = if cfg!(windows) {
            "libzstd.dll"
        } else {
            "libzstd.so"
        };

        let mut tried: Vec<PathBuf> = Vec::new();

        if let Some(path) = override_path {
            return Self::with_path(path);
        }
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
        {
            tried.push(exe_dir.join("lib").join(lib_name));
            tried.push(exe_dir.join(lib_name));
        }
        if let Ok(cwd) = std::env::current_dir() {
            tried.push(cwd.join("lib").join(lib_name));
            tried.push(cwd.join(lib_name));
        }

        let mut last_err: Option<libloading::Error> = None;
        for path in &tried {
            if !path.exists() {
                continue;
            }
            unsafe {
                match Library::new(path) {
                    Ok(lib) => return Self::from_library(lib),
                    Err(e) => last_err = Some(e),
                }
            }
        }

        Err(anyhow!(
            "Failed to load ZSTD library. Tried: {}. Last error: {:?}",
            tried
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            last_err
        ))
    }

    /// Load exactly this path, without falling back to another installed copy.
    pub fn with_path(path: &Path) -> Result<Self> {
        unsafe {
            let lib = Library::new(path)
                .with_context(|| format!("Failed to load ZSTD library from {}", path.display()))?;
            Self::from_library(lib)
        }
    }

    unsafe fn from_library(lib: Library) -> Result<Self> {
        let create_ddict: Symbol<unsafe extern "C" fn(*const c_char, usize) -> usize> =
            lib.get(b"ZSTD_createDDict\0").context("ZSTD_createDDict")?;
        let create_dctx: Symbol<unsafe extern "C" fn() -> usize> =
            lib.get(b"ZSTD_createDCtx\0").context("ZSTD_createDCtx")?;
        let dctx_set_parameter: Symbol<unsafe extern "C" fn(usize, c_int, c_int) -> usize> = lib
            .get(b"ZSTD_DCtx_setParameter\0")
            .context("ZSTD_DCtx_setParameter")?;
        let decompress_using_ddict: Symbol<
            unsafe extern "C" fn(usize, *mut c_void, usize, *const c_char, usize, usize) -> usize,
        > = lib
            .get(b"ZSTD_decompress_usingDDict\0")
            .context("ZSTD_decompress_usingDDict")?;
        let free_dctx: Symbol<unsafe extern "C" fn(usize) -> usize> =
            lib.get(b"ZSTD_freeDCtx\0").context("ZSTD_freeDCtx")?;
        let free_ddict: Symbol<unsafe extern "C" fn(usize) -> usize> =
            lib.get(b"ZSTD_freeDDict\0").context("ZSTD_freeDDict")?;

        // Extend lifetimes to 'static — safe because we keep `lib` alive in the struct.
        let create_ddict: Symbol<'static, _> = std::mem::transmute(create_ddict);
        let create_dctx: Symbol<'static, _> = std::mem::transmute(create_dctx);
        let dctx_set_parameter: Symbol<'static, _> = std::mem::transmute(dctx_set_parameter);
        let decompress_using_ddict: Symbol<'static, _> =
            std::mem::transmute(decompress_using_ddict);
        let free_dctx: Symbol<'static, _> = std::mem::transmute(free_dctx);
        let free_ddict: Symbol<'static, _> = std::mem::transmute(free_ddict);

        Ok(Self {
            lib,
            create_ddict,
            create_dctx,
            dctx_set_parameter,
            decompress_using_ddict,
            free_dctx,
            free_ddict,
        })
    }

    pub(crate) fn create_ddict(&self, dict: &[u8]) -> usize {
        unsafe { (self.create_ddict)(dict.as_ptr() as *const c_char, dict.len()) }
    }
    pub(crate) fn create_dctx(&self) -> usize {
        unsafe { (self.create_dctx)() }
    }
    pub(crate) fn dctx_ref_multi_ddicts(&self, ctx: usize) {
        // ZSTD_d_refMultipleDDicts = 1000
        unsafe {
            (self.dctx_set_parameter)(ctx, 1000, 1);
        }
    }
    pub(crate) fn decompress(
        &self,
        ctx: usize,
        out: &mut [u8],
        compressed: &[u8],
        dict: usize,
    ) -> usize {
        unsafe {
            (self.decompress_using_ddict)(
                ctx,
                out.as_mut_ptr() as *mut c_void,
                out.len(),
                compressed.as_ptr() as *const c_char,
                compressed.len(),
                dict,
            )
        }
    }
    pub(crate) fn free_dctx(&self, ctx: usize) {
        unsafe {
            (self.free_dctx)(ctx);
        }
    }
    pub(crate) fn free_ddict(&self, dict: usize) {
        unsafe {
            (self.free_ddict)(dict);
        }
    }

    /// Open a magicless-format decoder bound to a zstd dictionary.
    ///
    /// The returned handle keeps the dictionary alive and frees it on drop.
    /// Suitable for the `Packages.bin` per-entity frame format which uses
    /// a single shared dictionary and `ZSTD_f_zstd1_magicless` framing.
    pub fn open_magicless_dict(&self, dict: &[u8]) -> MagiclessDictDecoder<'_> {
        let ddict = unsafe { (self.create_ddict)(dict.as_ptr() as *const c_char, dict.len()) };
        let dctx = unsafe { (self.create_dctx)() };
        // ZSTD_d_format (param 1000) = ZSTD_f_zstd1_magicless (1).
        unsafe { (self.dctx_set_parameter)(dctx, 1000, 1) };
        MagiclessDictDecoder {
            zstd: self,
            dctx,
            ddict,
        }
    }
}

/// RAII handle around a magicless-mode `ZSTD_DCtx` + bound `ZSTD_DDict`.
pub struct MagiclessDictDecoder<'z> {
    zstd: &'z Zstd,
    dctx: usize,
    ddict: usize,
}

impl<'z> MagiclessDictDecoder<'z> {
    /// Decompress one frame, allocating an output buffer of `expected_len`.
    ///
    /// Returns the decoded bytes (truncated to the actual decompressed size).
    pub fn decompress(&self, compressed: &[u8], expected_len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; expected_len];
        let rc = unsafe {
            (self.zstd.decompress_using_ddict)(
                self.dctx,
                out.as_mut_ptr() as *mut c_void,
                out.len(),
                compressed.as_ptr() as *const c_char,
                compressed.len(),
                self.ddict,
            )
        };
        // ZSTD returns either the produced byte count or a negated error code
        // (values above (usize::MAX - 256) are errors per ZSTD_isError convention).
        if rc > usize::MAX - 256 {
            return Err(anyhow!("ZSTD decompression error code {}", rc as isize));
        }
        out.truncate(rc);
        Ok(out)
    }
}

impl Drop for MagiclessDictDecoder<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.zstd.free_dctx)(self.dctx);
            (self.zstd.free_ddict)(self.ddict);
        }
    }
}

// ─── Languages.bin parser ────────────────────────────────────────────────

/// Parse a Languages.bin H-chunk into a sorted map of `path+name` → text.
pub fn languages_unpack(
    bin: &[u8],
    zstd: &Zstd,
) -> Result<std::collections::BTreeMap<String, String>> {
    use byteorder::{LittleEndian, ReadBytesExt};
    use std::io::Cursor;

    let mut cursor = Cursor::new(bin);
    let mut entries = std::collections::BTreeMap::new();

    cursor.set_position(16); // Skip 16-byte hash.

    let magic1 = cursor.read_u32::<LittleEndian>()?; // 0x14
    let magic2 = cursor.read_u32::<LittleEndian>()?; // 0x2B / 0x2C
    let magic3 = cursor.read_u32::<LittleEndian>()?; // 0x01

    if magic1 != 0x14 || (magic2 != 0x2B && magic2 != 0x2C) || magic3 != 0x01 {
        return Err(anyhow!(
            "Invalid language file magic: 0x{:X}, 0x{:X}, 0x{:X} (expected 0x14, 0x2B/0x2C, 0x01)",
            magic1,
            magic2,
            magic3
        ));
    }

    let num_suffixes = cursor.read_u32::<LittleEndian>()?;
    for _ in 0..num_suffixes {
        let suffix_len = cursor.read_u32::<LittleEndian>()?;
        cursor.set_position(cursor.position() + suffix_len as u64);
    }

    let dict_len = cursor.read_u32::<LittleEndian>()?;
    let dict_start = cursor.position() as usize;
    cursor.set_position(cursor.position() + dict_len as u64);
    let dict_bin = &bin[dict_start..dict_start + dict_len as usize];

    let num_paths = cursor.read_u32::<LittleEndian>()?;

    let dict = zstd.create_ddict(dict_bin);
    let ctx = zstd.create_dctx();
    zstd.dctx_ref_multi_ddicts(ctx);

    for _ in 0..num_paths {
        let path_len = cursor.read_u32::<LittleEndian>()?;
        let path_start = cursor.position() as usize;
        cursor.set_position(cursor.position() + path_len as u64);
        let path =
            String::from_utf8_lossy(&bin[path_start..path_start + path_len as usize]).to_string();

        let chunk_len = cursor.read_u32::<LittleEndian>()?;
        let chunk_start = cursor.position() as usize;
        cursor.set_position(cursor.position() + chunk_len as u64);
        let chunk = &bin[chunk_start..chunk_start + chunk_len as usize];

        let num_labels = cursor.read_u32::<LittleEndian>()?;
        for _ in 0..num_labels {
            let name_len = cursor.read_u32::<LittleEndian>()?;
            let name_start = cursor.position() as usize;
            cursor.set_position(cursor.position() + name_len as u64);
            let name = String::from_utf8_lossy(&bin[name_start..name_start + name_len as usize])
                .to_string();

            let offset = cursor.read_u32::<LittleEndian>()?;
            let size = cursor.read_u16::<LittleEndian>()?;
            let flags = cursor.read_u16::<LittleEndian>()?;

            let mut data = chunk[offset as usize..(offset + size as u32) as usize].to_vec();

            if (flags & 0x200) != 0 {
                let (decompressed_size, data_offset) = unpack_u32_dyn_le(&data, 0)?;
                let compressed = &data[data_offset..];
                let mut output = vec![0u8; decompressed_size as usize];
                let result = zstd.decompress(ctx, &mut output, compressed, dict);
                if result != decompressed_size as usize {
                    zstd.free_dctx(ctx);
                    zstd.free_ddict(dict);
                    return Err(anyhow!(
                        "ZSTD decompression failed: expected {}, got {}",
                        decompressed_size,
                        result
                    ));
                }
                data = output;
            }

            let full_key = format!("{}{}", path, name);
            entries.insert(full_key, String::from_utf8_lossy(&data).to_string());
        }
    }

    zstd.free_dctx(ctx);
    zstd.free_ddict(dict);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_explicit_override_does_not_fall_back_to_installed_zstd() {
        let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/missing-override-regression/libzstd.dll");
        assert!(!missing.exists());
        let error = match Zstd::from_override(Some(&missing)) {
            Ok(_) => panic!("An explicit missing override must fail"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains(&missing.to_string_lossy().to_string()));
    }

    #[test]
    fn b64m_round_trip() {
        let data = b"hello world\xff\x00\x80";
        let encoded = b64m_encode(data);
        let decoded = b64m_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn b64m_replaces_slash_with_dash() {
        // base64 of [0xff, 0xff, 0xff] is "////" which becomes "----".
        let s = b64m_encode(&[0xff, 0xff, 0xff]);
        assert!(!s.contains('/'));
        assert!(s.contains('-'));
    }

    #[test]
    fn varint_decodes_single_byte() {
        let (v, n) = unpack_u32_dyn_le(&[0x05], 0).unwrap();
        assert_eq!(v, 5);
        assert_eq!(n, 1);
    }

    #[test]
    fn varint_decodes_multi_byte() {
        // 0xAC 0x02 = (0x2C | (0x02 << 7)) = 0x12C = 300
        let (v, n) = unpack_u32_dyn_le(&[0xAC, 0x02], 0).unwrap();
        assert_eq!(v, 300);
        assert_eq!(n, 2);
    }
}
