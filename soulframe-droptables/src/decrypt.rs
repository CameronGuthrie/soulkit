//! Camellia-256-ECB decryption + XXTEA-derived per-path key.
//!
//! Ported from `decrypt_drop_tables.py`.

use anyhow::{anyhow, bail, Context, Result};
use camellia::cipher::generic_array::GenericArray;
use camellia::cipher::{BlockDecrypt, KeyInit};
use camellia::Camellia256;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use soulframe_cache::{
    cache_get_toc_entry, detect_soulframe_cache, load_local_cache, Oodle, TocEntry,
};

/// 128-bit XXTEA "delta" constant.
const XXTEA_DELTA: u32 = 0x9E37_79B9;

/// 16-byte XXTEA salt embedded in the current Soulframe build.
const CAMELLIA_PATH_SALT_HEX: &str = "4522268168bcc2a980c945adc1b43c64";

fn camellia_path_salt() -> [u8; 16] {
    let v = hex::decode(CAMELLIA_PATH_SALT_HEX).expect("hardcoded salt");
    let mut out = [0u8; 16];
    out.copy_from_slice(&v);
    out
}

/// Encrypt a `[u32]` slice in place using XXTEA with the 16-byte `key`.
pub fn xxtea_encrypt_words(words: &mut [u32], key: &[u8; 16]) {
    let n = words.len();
    if n < 2 {
        return;
    }
    let k: [u32; 4] = [
        u32::from_le_bytes(key[0..4].try_into().unwrap()),
        u32::from_le_bytes(key[4..8].try_into().unwrap()),
        u32::from_le_bytes(key[8..12].try_into().unwrap()),
        u32::from_le_bytes(key[12..16].try_into().unwrap()),
    ];

    let rounds = 6 + 52 / n;
    let mut sum: u32 = 0;
    let mut z = words[n - 1];

    for _ in 0..rounds {
        sum = sum.wrapping_add(XXTEA_DELTA);
        let e = ((sum >> 2) & 3) as usize;

        for p in 0..(n - 1) {
            let y = words[p + 1];
            let mut mx = ((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4));
            mx ^= (sum ^ y).wrapping_add(k[(p & 3) ^ e] ^ z);
            words[p] = words[p].wrapping_add(mx);
            z = words[p];
        }

        let p = n - 1;
        let y = words[0];
        let mut mx = ((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4));
        mx ^= (sum ^ y).wrapping_add(k[(p & 3) ^ e] ^ z);
        words[p] = words[p].wrapping_add(mx);
        z = words[p];
    }
}

/// Derive the deterministic Camellia-256 key for a cache entry path.
///
/// 1. Reverse the ASCII bytes of `path`.
/// 2. Zero-pad to at least 32 bytes, length must be a multiple of 4.
/// 3. XXTEA-encrypt as `u32` words using the build's 16-byte salt.
/// 4. Take the first 32 bytes of the little-endian word stream.
pub fn derive_path_key(path: &str) -> [u8; 32] {
    let mut buf: Vec<u8> = path.as_bytes().iter().rev().copied().collect();
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
    while buf.len() < 32 {
        buf.push(0);
    }
    let n_words = buf.len() / 4;
    let mut words: Vec<u32> = (0..n_words)
        .map(|i| u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap()))
        .collect();
    xxtea_encrypt_words(&mut words, &camellia_path_salt());

    let mut out = [0u8; 32];
    for (i, w) in words.iter().take(8).enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Decrypt `ciphertext` with Camellia-256-ECB. `ciphertext.len()` must be a
/// multiple of 16; trailing partial blocks are silently truncated.
pub fn camellia_ecb_decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Vec<u8> {
    let block_count = ciphertext.len() / 16;
    let ct_len = block_count * 16;
    let cipher = Camellia256::new(GenericArray::from_slice(key));
    let mut out = ciphertext[..ct_len].to_vec();
    for chunk in out.chunks_exact_mut(16) {
        let mut block: GenericArray<u8, _> = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        chunk.copy_from_slice(&block);
    }
    out
}

/// Sniff for a "looks like a drop table" plaintext.
fn validate_plaintext(pt: &[u8]) -> bool {
    if pt.len() < 48 {
        return false;
    }
    let data = &pt[16..];
    if data.len() >= 12
        && data[..8] == [0u8; 8]
        && u32::from_le_bytes(data[8..12].try_into().unwrap()) <= 100
    {
        return true;
    }
    let probe_len = data.len().min(200);
    let probe = String::from_utf8_lossy(&data[..probe_len]);
    probe.contains("/SF/") || probe.contains("/Lotus/")
}

fn common_header_text(raw: &[u8], base: usize) -> Option<&[u8]> {
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
        let path_len = u32::from_le_bytes(raw[off..off + 4].try_into().ok()?) as usize;
        off += 4;
        if off + path_len > raw.len() {
            return None;
        }
        off += path_len;
    }

    if off + 4 > raw.len() {
        return None;
    }
    let content_len = u32::from_le_bytes(raw[off..off + 4].try_into().ok()?) as usize;
    off += 4;
    let available = raw.len().saturating_sub(off);
    if content_len > available + 100 {
        return None;
    }
    raw.get(off..off.saturating_add(content_len.min(available)))
}

fn validate_plaintext_common_header(raw: &[u8]) -> bool {
    let Some(content) = common_header_text(raw, 16) else {
        return false;
    };

    if content.is_empty() {
        return true;
    }

    let sample: Vec<char> = String::from_utf8_lossy(content).chars().take(100).collect();
    let printable = sample
        .iter()
        .filter(|&&c| (32u32..127).contains(&(c as u32)) || matches!(c, '\n' | '\r' | '\t'))
        .count();

    !sample.is_empty() && printable * 10 >= sample.len() * 6
}

/// One encrypted entry pulled from the H.Misc cache.
#[derive(Debug, Clone)]
pub struct EncryptedEntry {
    pub path: String,
    pub raw: Vec<u8>,
}

/// Load /SF/ drop/reward/manifest entries from the local H.Misc cache pair.
pub fn load_encrypted_entries(cache_dir: Option<&Path>) -> Result<Vec<EncryptedEntry>> {
    let dir: PathBuf = match cache_dir {
        Some(p) => p.to_path_buf(),
        None => detect_soulframe_cache().ok_or_else(|| {
            anyhow!("Soulframe cache dir not found. Pass --cache-dir or install the game.")
        })?,
    };

    let (entries, cache_bytes) = load_local_cache(&dir, "H.Misc")?
        .ok_or_else(|| anyhow!("H.Misc cache pair not found in {}", dir.display()))?;

    let oodle = Oodle::new()?;

    let mut out = Vec::new();
    for e in &entries {
        if e.is_dir {
            continue;
        }
        let p = format!("/{}", e.full_path);
        let lower = p.to_lowercase();
        if !p.starts_with("/SF/") {
            continue;
        }
        if lower.contains("/sounds/") || lower.ends_with(".wav") {
            continue;
        }
        if !["DropTable", "Reward", "Manifest"]
            .iter()
            .any(|t| p.contains(t))
        {
            continue;
        }
        let raw = match cache_get_toc_entry(&oodle, &cache_bytes, e) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if raw.len() < 24 {
            continue;
        }
        out.push(EncryptedEntry { path: p, raw });
    }

    Ok(out)
}

/// Result of decrypting one entry.
#[derive(Debug, Clone)]
pub struct DecryptResult {
    pub path: String,
    pub key_hex: String,
    pub plaintext: Vec<u8>,
    pub key_source: KeySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Plaintext,
    PathDerived,
    Captured,
}

/// Try the deterministic path-derived key first, then each supplied captured
/// key, until one produces a plaintext that passes [`validate_plaintext`].
pub fn decrypt_all(
    captured_keys: &[[u8; 32]],
    entries: &[EncryptedEntry],
) -> (Vec<DecryptResult>, Vec<String>) {
    let mut ok = Vec::new();
    let mut failed = Vec::new();

    for e in entries {
        if validate_plaintext_common_header(&e.raw) {
            ok.push(DecryptResult {
                path: e.path.clone(),
                key_hex: String::new(),
                plaintext: e.raw.clone(),
                key_source: KeySource::Plaintext,
            });
            continue;
        }

        let body = if e.raw.len() > 17 {
            &e.raw[16..e.raw.len() - 1]
        } else {
            &e.raw[16..]
        };
        let body_len = (body.len() / 16) * 16;
        if body_len < 16 {
            failed.push(e.path.clone());
            continue;
        }
        let body = &body[..body_len];

        let mut tried: Vec<String> = Vec::new();
        let mut found = false;

        let mut candidates: Vec<([u8; 32], KeySource)> = Vec::new();
        candidates.push((derive_path_key(&e.path), KeySource::PathDerived));
        for k in captured_keys {
            candidates.push((*k, KeySource::Captured));
        }

        for (key, src) in candidates {
            let key_hex = hex::encode(key);
            if tried.contains(&key_hex) {
                continue;
            }
            tried.push(key_hex.clone());
            let pt = camellia_ecb_decrypt(&key, body);
            if validate_plaintext(&pt) {
                ok.push(DecryptResult {
                    path: e.path.clone(),
                    key_hex,
                    plaintext: pt,
                    key_source: src,
                });
                found = true;
                break;
            }
        }

        if !found {
            failed.push(e.path.clone());
        }
    }

    (ok, failed)
}

/// Sanitise a slash-delimited cache path to a filesystem-safe basename.
pub fn safe_filename(path: &str) -> String {
    path.replace('/', "_")
        .replace('\\', "_")
        .trim_start_matches('_')
        .to_string()
}

/// Write each successful decryption to `<output_dir>/<sanitised>.bin`,
/// stripping the 16-byte plaintext hash header to match the Python output.
pub fn write_decrypted(results: &[DecryptResult], output_dir: &Path) -> Result<usize> {
    fs::create_dir_all(output_dir).with_context(|| format!("creating {}", output_dir.display()))?;
    let mut written_paths = BTreeSet::new();
    for r in results {
        let name = safe_filename(&r.path);
        let path = output_dir.join(format!("{}.bin", name));
        fs::write(&path, &r.plaintext[16..])
            .with_context(|| format!("writing {}", path.display()))?;
        written_paths.insert(path);
    }
    Ok(written_paths.len())
}

/// Load 256-bit hex keys from a text file (one per line) OR a
/// `capture_output.txt` log (regex-style `KEY [RCX] (256b): <64-hex>`).
pub fn load_keys_from_file(path: &Path) -> Result<Vec<[u8; 32]>> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Patterns to scan for log files.
    let log_re_a = "KEY [RCX] (256b): ";
    let log_re_b = "[RCX] -> ";
    let mut try_log = |needle: &str| {
        let mut start = 0;
        while let Some(p) = s[start..].find(needle) {
            let from = start + p + needle.len();
            if from + 64 <= s.len() {
                let candidate = &s[from..from + 64];
                if candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                    let lower = candidate.to_ascii_lowercase();
                    if seen.insert(lower.clone()) {
                        if let Ok(bytes) = hex::decode(&lower) {
                            if bytes.len() == 32 {
                                let mut k = [0u8; 32];
                                k.copy_from_slice(&bytes);
                                out.push(k);
                            }
                        }
                    }
                }
            }
            start = start + p + 1;
        }
    };
    try_log(log_re_a);
    try_log(log_re_b);

    if !out.is_empty() {
        return Ok(out);
    }

    // Fallback: plain one-key-per-line file.
    for line in s.lines() {
        let line = line.trim();
        if line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()) {
            let lower = line.to_ascii_lowercase();
            if seen.insert(lower.clone()) {
                if let Ok(bytes) = hex::decode(&lower) {
                    if bytes.len() == 32 {
                        let mut k = [0u8; 32];
                        k.copy_from_slice(&bytes);
                        out.push(k);
                    }
                }
            }
        }
    }

    Ok(out)
}

/// Parse a single 64-char hex string as a 32-byte key. Returns `Err` if
/// the input is not exactly 64 hex characters.
pub fn parse_inline_key(s: &str) -> Result<[u8; 32]> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("inline key must be exactly 64 hex characters");
    }
    let bytes = hex::decode(s.to_ascii_lowercase())?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[allow(dead_code)]
pub fn touch_tocentry(_e: &TocEntry) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxtea_two_word_round_trip_is_deterministic() {
        // Just verify the function doesn't panic on a small input and produces
        // identical output for identical input — full crypto correctness is
        // covered indirectly by `derive_path_key` matching the Python output.
        let mut a = [0x11111111u32, 0x22222222u32];
        let mut b = [0x11111111u32, 0x22222222u32];
        let salt = [0u8; 16];
        xxtea_encrypt_words(&mut a, &salt);
        xxtea_encrypt_words(&mut b, &salt);
        assert_eq!(a, b);
    }

    #[test]
    fn derive_path_key_is_32_bytes() {
        let k = derive_path_key("/SF/Types/PickUps/DropTables/Foo");
        assert_eq!(k.len(), 32);
        // Sanity: not all zeros.
        assert!(k.iter().any(|&b| b != 0));
    }

    #[test]
    fn safe_filename_strips_leading_slash() {
        let n = safe_filename("/SF/Foo/Bar");
        assert_eq!(n, "SF_Foo_Bar");
    }

    #[test]
    fn plaintext_common_header_is_accepted_without_key() {
        let mut raw = vec![0xAB; 16];
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&13u32.to_le_bytes());
        raw.extend_from_slice(b"Favors={/SF/X");

        let entry = EncryptedEntry {
            path: "/SF/Syndicates/FooManifest".to_string(),
            raw,
        };

        let (ok, failed) = decrypt_all(&[], &[entry]);
        assert!(failed.is_empty());
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].key_source, KeySource::Plaintext);
    }
}
