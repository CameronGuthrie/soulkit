//! Scan a running Soulframe process for runtime reward-table data.
//!
//! The three faction `MissionRewardItemManifest` cache objects currently
//! decrypt to identical empty 32-byte objects. In-game and item metadata still
//! confirm that faction Votives can award item fragments, so the empty cache
//! objects must not be interpreted as an empty reward pool. The roll may be
//! server-authoritative or assembled elsewhere by native code. This scanner
//! remains useful for detecting a hot-loaded runtime copy, but a zero-hit result
//! does not disprove Votive item rewards.
//!
//! This module attaches to the running game process (read-only), walks the
//! virtual address space, and extracts any drop-table items it finds.
//!
//! ## Usage
//!
//! 1. Launch Soulframe normally.
//! 2. Navigate to the activity whose runtime reward data you want to inspect.
//! 3. Run:
//!    ```text
//!    soulframe-droptables scan-rewards
//!    ```
//! 4. Inspect `output/mission_rewards_scan.json`.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

// ── Shared types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ScannedItem {
    pub path: String,
    pub name: String,
    pub tier: u32,
    pub rarity: String,
    pub weight: f32,
    pub bias: f32,
}

/// A contiguous memory region that contained at least one item.
#[derive(Debug, Clone, Serialize)]
pub struct ScannedRegion {
    /// Virtual base address (hex string).
    pub base_address: String,
    pub region_size: usize,
    pub item_count: usize,
    pub items: Vec<ScannedItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanOutput {
    pub process_name: String,
    pub pid: u32,
    pub regions_scanned: usize,
    pub regions_with_hits: usize,
    pub total_items_found: usize,
    pub results: Vec<ScannedRegion>,
}

const RARITY_NAMES: &[(u32, &str)] = &[
    (0, "Common"),
    (1, "Uncommon"),
    (2, "Rare"),
    (3, "Legendary"),
];

fn rarity_name(tier: u32) -> &'static str {
    for (t, n) in RARITY_NAMES {
        if *t == tier {
            return n;
        }
    }
    "Unknown"
}

// ── Non-Windows stub ──────────────────────────────────────────────────────────

#[cfg(not(windows))]
pub fn run(_pid: Option<u32>, _output: &Path) -> Result<()> {
    anyhow::bail!("scan-rewards is Windows-only");
}

// ── Windows implementation ────────────────────────────────────────────────────

#[cfg(windows)]
pub fn run(pid: Option<u32>, output: &Path) -> Result<()> {
    use anyhow::{anyhow, Context};

    let (pid, proc_name) = if let Some(p) = pid {
        (p, format!("PID:{}", p))
    } else {
        find_soulframe_pid()
            .context("searching for Soulframe process")?
            .ok_or_else(|| {
                anyhow!(
                    "Soulframe.x64.exe not found. \
                     Is the game running? Pass --pid to specify a PID manually."
                )
            })?
    };

    println!("[*] Found process: {} (PID {})", proc_name, pid);

    let scan = scan_process(pid, &proc_name)?;

    println!(
        "[*] Scanned {} regions — {} with hits, {} items total",
        scan.regions_scanned, scan.regions_with_hits, scan.total_items_found
    );
    for (i, region) in scan.results.iter().enumerate() {
        println!(
            "\n  [Region {}]  base={}  size={} bytes  {} items",
            i + 1,
            region.base_address,
            region.region_size,
            region.item_count,
        );
        for item in &region.items {
            println!("    {:5} w={:.3}  {}", item.rarity, item.weight, item.path);
        }
    }

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, serde_json::to_string_pretty(&scan)?)?;
    println!("\n[*] Wrote {}", output.display());
    Ok(())
}

#[cfg(windows)]
fn find_soulframe_pid() -> Result<Option<(u32, String)>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
    };

    const TARGET: &str = "soulframe.x64.exe";

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| anyhow::anyhow!("CreateToolhelp32Snapshot: {:?}", e))?;

        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;

        let result = if Process32First(snap, &mut entry).is_ok() {
            loop {
                // szExeFile is a fixed [i8; 260] on windows crate; interpret as bytes.
                let raw: &[u8] = &*(entry.szExeFile.as_ptr() as *const [u8; 260]);
                let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                let exe_name = String::from_utf8_lossy(&raw[..len]).into_owned();

                if exe_name.to_ascii_lowercase() == TARGET {
                    let result = Some((entry.th32ProcessID, exe_name));
                    CloseHandle(snap).ok();
                    return Ok(result);
                }

                if Process32Next(snap, &mut entry).is_err() {
                    break;
                }
            }
            None
        } else {
            None
        };

        CloseHandle(snap).ok();
        Ok(result)
    }
}

#[cfg(windows)]
fn scan_process(pid: u32, proc_name: &str) -> Result<ScanOutput> {
    use anyhow::anyhow;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    // MEM_IMAGE type = 0x1000000 — code/data from PE images (DLLs, EXEs).
    // We skip these because drop-table content lives in private heap, not image sections.
    const MEM_IMAGE_TYPE: u32 = 0x0100_0000;

    let hproc = unsafe {
        OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
            .map_err(|e| anyhow!("OpenProcess({pid}) failed: {:?}", e))?
    };

    let mut addr: u64 = 0;
    let mut regions_scanned = 0usize;
    let mut results: Vec<ScannedRegion> = Vec::new();

    println!("[*] Walking virtual address space...");

    loop {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let queried = unsafe {
            VirtualQueryEx(
                hproc,
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == 0 {
            break;
        }

        let base = mbi.BaseAddress as u64;
        let size = mbi.RegionSize;

        // Advance before any `continue` so we never loop on the same region.
        addr = base.saturating_add(size as u64);
        if addr == 0 {
            break;
        }

        // Must be committed.
        if (mbi.State.0 & MEM_COMMIT.0) == 0 {
            continue;
        }

        // Skip PE image sections — drop-table data lives in private heap.
        if mbi.Type.0 == MEM_IMAGE_TYPE {
            continue;
        }

        // Skip inaccessible / guard pages.
        let prot = mbi.Protect.0;
        if (prot & PAGE_NOACCESS.0) != 0 || (prot & PAGE_GUARD.0) != 0 {
            continue;
        }

        // Require at least one readable flag.
        let readable = (prot & PAGE_READONLY.0) != 0
            || (prot & PAGE_READWRITE.0) != 0
            || (prot & PAGE_EXECUTE_READ.0) != 0
            || (prot & PAGE_EXECUTE_READWRITE.0) != 0;
        if !readable {
            continue;
        }

        // Ignore tiny regions (can't hold a drop-table entry) and absurdly
        // large ones to avoid reading GPU memory or file-backed maps.
        if size < 512 || size > 128 * 1024 * 1024 {
            continue;
        }

        regions_scanned += 1;

        // Read in 4 MB chunks so we don't allocate one giant buffer per region.
        const CHUNK: usize = 4 * 1024 * 1024;
        let mut region_items: Vec<ScannedItem> = Vec::new();
        let mut offset = 0usize;

        while offset < size {
            let to_read = CHUNK.min(size - offset);
            let chunk_addr = base + offset as u64;

            let mut buf = vec![0u8; to_read];
            let mut read = 0usize;
            let ok = unsafe {
                ReadProcessMemory(
                    hproc,
                    chunk_addr as *const _,
                    buf.as_mut_ptr() as *mut _,
                    to_read,
                    Some(&mut read),
                )
                .is_ok()
            };
            if ok && read > 0 {
                buf.truncate(read);
                let found = find_drop_table_items(&buf);
                region_items.extend(found);
            }

            offset += to_read;
        }

        if !region_items.is_empty() {
            let count = region_items.len();
            results.push(ScannedRegion {
                base_address: format!("0x{:016x}", base),
                region_size: size,
                item_count: count,
                items: region_items,
            });
        }
    }

    unsafe {
        CloseHandle(hproc).ok();
    }

    let regions_with_hits = results.len();
    let total_items_found = results.iter().map(|r| r.item_count).sum();

    Ok(ScanOutput {
        process_name: proc_name.to_string(),
        pid,
        regions_scanned,
        regions_with_hits,
        total_items_found,
        results,
    })
}

/// Scan a byte buffer for binary drop-table item records.
///
/// Each record has the layout used in decrypted Soulframe drop-table binaries:
///   u32 path_len  |  path_bytes (ASCII)  |  f32 bias  |  u32 tier  |  f32 weight  |  4 bytes
///
/// Items are identified by the combination of a valid `/SF/` or `/Lotus/` path
/// and a weight value that matches one of the four known rarity weights
/// (`0.755`, `0.22`, `0.02`, `0.005`).
fn find_drop_table_items(buf: &[u8]) -> Vec<ScannedItem> {
    let mut items = Vec::new();
    let mut i = 0usize;

    while i + 4 < buf.len() {
        let slen = match read_u32_le(buf, i) {
            Some(v) => v as usize,
            None => break,
        };

        if (10..=500).contains(&slen) && i + 4 + slen + 16 <= buf.len() {
            let s_bytes = &buf[i + 4..i + 4 + slen];
            if let Ok(s) = std::str::from_utf8(s_bytes) {
                // Accept only Soulframe item paths. The broad /Lotus/ namespace
                // also contains Warframe UI strings (/Lotus/Language/...) that
                // produce false positives when the bytes that follow happen to
                // look like rarity weights. All Soulframe mission rewards live
                // under /SF/, so restrict to that prefix and exclude any
                // residual UI/shader/language subtrees.
                let is_valid = s.starts_with("/SF/")
                    && !s.contains("/Language/")
                    && !s.contains("/Interface/")
                    && !s.contains("/Shaders/")
                    && s.len() > 15
                    && !s.ends_with('/')
                    && s.chars().all(|c| (32..127u32).contains(&(c as u32)));

                // Also reject implausible bias values. Real items have bias ~= 0
                // or a small reasonable float; denormalized near-zero garbage
                // (e.g. 6e-36) signals a false-positive path match.

                if is_valid {
                    let dstart = i + 4 + slen;
                    let bias = read_f32_le(buf, dstart).unwrap_or(0.0);
                    let tier = read_u32_le(buf, dstart + 4).unwrap_or(0);
                    let weight = read_f32_le(buf, dstart + 8).unwrap_or(0.0);

                    let weight_ok = (weight - 0.755).abs() < 0.01
                        || (weight - 0.22).abs() < 0.01
                        || (weight - 0.02).abs() < 0.01
                        || (weight - 0.005).abs() < 0.001;
                    // Accept entries with an unknown weight when the tier is
                    // valid and the weight is non-zero (non-standard bias items).
                    let tier_fallback = tier < 4 && weight.abs() > 0.01;
                    // Reject denormalized / subnormal bias values — real items
                    // have bias == 0.0 or a normal float (|bias| < 1e10).
                    let bias_ok = bias == 0.0 || (bias.is_normal() && bias.abs() < 1e10);

                    if (weight_ok || tier_fallback) && bias_ok {
                        let effective_weight = if weight_ok {
                            weight
                        } else {
                            crate::parse::RARITY_WEIGHTS
                                .iter()
                                .find(|(t, _)| *t == tier)
                                .map(|(_, w)| *w as f32)
                                .unwrap_or(weight)
                        };
                        let name = s.rsplit('/').next().unwrap_or(s).to_string();
                        items.push(ScannedItem {
                            path: s.to_string(),
                            name,
                            tier,
                            rarity: rarity_name(tier).to_string(),
                            weight: effective_weight,
                            bias,
                        });
                        i = dstart + 16;
                        continue;
                    }
                }
            }
        }

        i += 1;
    }

    items
}

fn read_u32_le(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn read_f32_le(buf: &[u8], off: usize) -> Option<f32> {
    buf.get(off..off + 4)
        .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
}
