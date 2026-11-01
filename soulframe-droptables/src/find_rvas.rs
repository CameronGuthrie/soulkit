//! Scan the Soulframe binary for Camellia-256 functions by locating the
//! SIGMA round-constants that any Camellia implementation must contain.
//!
//! Ported from `find_camellia_rva.py`.

use anyhow::{anyhow, bail, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Default game executable path.
///
/// `SOULFRAME_ROOT` (the `Downloaded\Public` folder, e.g. a Steam library)
/// overrides the default `%LOCALAPPDATA%\Soulframe\Downloaded\Public` location.
pub fn default_game_exe() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("SOULFRAME_ROOT") {
        return Some(PathBuf::from(root).join("Soulframe.x64.exe"));
    }
    let local = std::env::var("LOCALAPPDATA").ok()?;
    Some(
        PathBuf::from(local)
            .join("Soulframe")
            .join("Downloaded")
            .join("Public")
            .join("Soulframe.x64.exe"),
    )
}

/// Camellia SIGMA constants (hi, lo) pairs as they appear in x86 code.
const SIGMA: [(u32, u32); 6] = [
    (0xA09E667F, 0x3BCC908B), // SIGMA1
    (0xB67AE858, 0x4CAA73B2), // SIGMA2
    (0x3C6EF372, 0xFE94F82B), // SIGMA3
    (0xA54FF53A, 0x5F1D36F1), // SIGMA4
    (0x10E527FA, 0xDE682D1D), // SIGMA5
    (0xB05688C2, 0xB3E6C1FD), // SIGMA6
];

#[derive(Debug, Clone)]
pub struct PeSection {
    pub name: String,
    pub rva: u32,
    pub vsize: u32,
    pub raw_off: u32,
    pub raw_size: u32,
}

/// Result of an inline-Camellia scan.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub sigma_count: usize,
    pub pairs: usize,
    pub rva_start: u32,
    pub rva_end: u32,
    pub func_rva: Option<u32>,
    pub section: String,
    pub prologue_hex: Option<String>,
    pub span: usize,
}

/// Final pick: best inline-Camellia function plus a backup.
#[derive(Debug, Clone)]
pub struct CamelliaRvas {
    pub inline: Option<u32>,
    pub backup: Option<u32>,
    pub inline_prologue: Option<String>,
    pub clusters: Vec<Cluster>,
}

pub fn parse_pe_sections(data: &[u8]) -> Result<Vec<PeSection>> {
    if data.len() < 0x40 {
        bail!("file too small to be a PE");
    }
    let mut cur = Cursor::new(&data[0x3C..0x40]);
    let pe_off = cur.read_u32::<LittleEndian>()? as usize;
    if pe_off + 0x18 > data.len() {
        bail!("PE header offset out of range");
    }
    let num_sec = (&data[pe_off + 6..pe_off + 8]).read_u16::<LittleEndian>()? as usize;
    let opt_size = (&data[pe_off + 0x14..pe_off + 0x16]).read_u16::<LittleEndian>()? as usize;
    let sec_start = pe_off + 0x18 + opt_size;

    let mut out = Vec::with_capacity(num_sec);
    for i in 0..num_sec {
        let off = sec_start + i * 40;
        if off + 40 > data.len() {
            bail!("section header out of range");
        }
        let name_bytes = &data[off..off + 8];
        let name_end = name_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_bytes.len());
        let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();

        let vsize = (&data[off + 8..off + 12]).read_u32::<LittleEndian>()?;
        let rva = (&data[off + 12..off + 16]).read_u32::<LittleEndian>()?;
        let raw_size = (&data[off + 16..off + 20]).read_u32::<LittleEndian>()?;
        let raw_off = (&data[off + 20..off + 24]).read_u32::<LittleEndian>()?;

        out.push(PeSection {
            name,
            rva,
            vsize,
            raw_off,
            raw_size,
        });
    }
    Ok(out)
}

pub fn file_offset_to_rva(sections: &[PeSection], foff: usize) -> Option<u32> {
    for s in sections {
        let raw_off = s.raw_off as usize;
        let raw_end = raw_off + s.raw_size as usize;
        if foff >= raw_off && foff < raw_end {
            return Some(s.rva + (foff as u32 - s.raw_off));
        }
    }
    None
}

fn find_function_boundary(data: &[u8], foff: usize) -> Option<usize> {
    // Walk back up to 32KB looking for INT3 padding (0xCC).
    let lower = foff.saturating_sub(0x8000).max(1);
    let mut pos = foff.checked_sub(1)?;
    while pos > lower {
        if data[pos] == 0xCC {
            return Some(pos + 1);
        }
        pos -= 1;
    }
    None
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    let mut start = 0;
    while let Some(p) = haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
    {
        hits.push(start + p);
        start += p + 1;
    }
    hits
}

/// Scan a PE file for inline-Camellia function candidates.
pub fn scan(data: &[u8], sections: &[PeSection]) -> Vec<Cluster> {
    let mut sigma_bytes: Vec<[u8; 4]> = Vec::with_capacity(12);
    for (hi, lo) in SIGMA {
        sigma_bytes.push(hi.to_le_bytes());
        sigma_bytes.push(lo.to_le_bytes());
    }

    let mut all_positions: Vec<(usize, usize)> = Vec::new();
    for (idx, pat) in sigma_bytes.iter().enumerate() {
        for foff in find_all(data, pat) {
            all_positions.push((foff, idx));
        }
    }
    all_positions.sort();

    const WINDOW: usize = 8192;
    let mut clusters_raw: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut i = 0;
    while i < all_positions.len() {
        let cluster_start = all_positions[i].0;
        let mut cluster = vec![all_positions[i]];
        let mut j = i + 1;
        while j < all_positions.len() && all_positions[j].0 - cluster_start < WINDOW {
            cluster.push(all_positions[j]);
            j += 1;
        }
        let unique: BTreeSet<_> = cluster.iter().map(|(_, idx)| *idx).collect();
        if unique.len() >= 6 {
            clusters_raw.push(cluster);
            i = j;
        } else {
            i += 1;
        }
    }

    let mut out = Vec::with_capacity(clusters_raw.len());
    for cluster in clusters_raw {
        let foff_min = cluster.first().unwrap().0;
        let foff_max = cluster.last().unwrap().0;
        let unique_sigmas: BTreeSet<_> = cluster.iter().map(|(_, idx)| *idx).collect();
        let unique_pairs: BTreeSet<_> = unique_sigmas.iter().map(|i| i / 2).collect();

        let rva_start = file_offset_to_rva(sections, foff_min).unwrap_or(0);
        let rva_end = file_offset_to_rva(sections, foff_max).unwrap_or(0);

        let sec_name = sections
            .iter()
            .find(|s| rva_start >= s.rva && rva_start < s.rva + s.vsize)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "?".to_string());

        let (func_rva, prologue_hex) = match find_function_boundary(data, foff_min) {
            Some(start_foff) => {
                let rva = file_offset_to_rva(sections, start_foff);
                let prologue = data.get(start_foff..start_foff + 16).map(hex::encode);
                (rva, prologue)
            }
            None => (None, None),
        };

        out.push(Cluster {
            sigma_count: cluster.len(),
            pairs: unique_pairs.len(),
            rva_start,
            rva_end,
            func_rva,
            section: sec_name,
            prologue_hex,
            span: foff_max - foff_min,
        });
    }

    out
}

/// Read the game exe and return the best inline + backup Camellia RVAs.
pub fn find_rvas(exe_path: Option<&Path>) -> Result<CamelliaRvas> {
    let path: PathBuf = match exe_path {
        Some(p) => p.to_path_buf(),
        None => default_game_exe()
            .ok_or_else(|| anyhow!("LOCALAPPDATA not set; cannot guess game exe"))?,
    };
    let data =
        std::fs::read(&path).with_context(|| format!("reading game exe {}", path.display()))?;
    let sections = parse_pe_sections(&data)?;
    let clusters = scan(&data, &sections);

    if clusters.is_empty() {
        bail!("No Camellia function clusters found in {}", path.display());
    }

    // Pick best inline: .text cluster with most pairs, widest span.
    let mut text_clusters: Vec<&Cluster> = clusters
        .iter()
        .filter(|c| c.section == ".text" && c.func_rva.is_some())
        .collect();
    text_clusters.sort_by(|a, b| b.pairs.cmp(&a.pairs).then_with(|| b.span.cmp(&a.span)));

    let inline = text_clusters.first().and_then(|c| c.func_rva);
    let inline_prologue = text_clusters.first().and_then(|c| c.prologue_hex.clone());
    let backup = text_clusters.get(1).and_then(|c| c.func_rva);

    Ok(CamelliaRvas {
        inline,
        backup,
        inline_prologue,
        clusters,
    })
}
