//! Camellia-256 key capture via Win32 user-mode debugger.
//!
//! Ported (and simplified) from `capture_keys.py`. Behavioural changes from
//! the Python:
//!
//! * No in-process drop-table verification on each captured key — the
//!   `decrypt` subcommand already does the full verification afterwards.
//! * Anti-debug patches mirror the Python (`PEB.BeingDebugged = 0`,
//!   `NtGlobalFlag & ~0x70`, `NtSetInformationThread -> xor eax,eax; ret`).
//! * Soft INT3 breakpoints with restore + single-step + reinstall just like
//!   the Python.
//!
//! On non-Windows targets this module compiles to a stub returning
//! `Err("capture_keys is Windows-only")`.

#![allow(clippy::too_many_lines)]

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CaptureOptions {
    /// Path to the game `.exe`.
    pub game_exe: PathBuf,
    /// Where to write `camellia_keys.txt` and `capture_output.txt`.
    pub output_dir: PathBuf,
    /// RVA of the inline Camellia function (primary breakpoint).
    pub inline_rva: u32,
    /// RVA of the backup Camellia function (e.g. OpenSSL-style key schedule).
    pub backup_rva: u32,
    /// Hard timeout in seconds before forcibly stopping the debugger.
    pub timeout_secs: u64,
    /// After this many seconds of no hits *after the first hit*, stop early.
    pub quiet_detach_secs: u64,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            game_exe: super::find_rvas::default_game_exe().unwrap_or_default(),
            output_dir: PathBuf::from("output"),
            inline_rva: 0x01919D70,
            backup_rva: 0x01D69540,
            timeout_secs: 600,
            quiet_detach_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CaptureSummary {
    pub hits: usize,
    pub unique_keys: usize,
    pub keys_hex: Vec<String>,
    pub elapsed_secs: f64,
    /// PID of the game instance this capture launched. Callers that want the
    /// game gone once they are done with it must terminate this PID and no
    /// other — a Soulframe the user started themselves is not ours to kill.
    pub launched_pid: u32,
}

#[cfg(not(windows))]
pub fn run(_opts: &CaptureOptions) -> Result<CaptureSummary> {
    anyhow::bail!("capture_keys is Windows-only");
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn load_existing_keys(_output_dir: &Path) -> Vec<String> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn terminate_pid(_pid: u32) -> Result<()> {
    anyhow::bail!("terminate_pid is Windows-only");
}

// ====================== Windows implementation ======================

#[cfg(windows)]
pub use windows_impl::{load_existing_keys, run, terminate_pid};

#[cfg(windows)]
mod windows_impl {
    use super::{CaptureOptions, CaptureSummary};
    use anyhow::{anyhow, Context, Result};
    use std::collections::{BTreeSet, HashMap};
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_SEM_TIMEOUT, EXCEPTION_BREAKPOINT, EXCEPTION_SINGLE_STEP,
        HANDLE, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        ContinueDebugEvent, DebugSetProcessKillOnExit, FlushInstructionCache, GetThreadContext,
        ReadProcessMemory, SetThreadContext, WaitForDebugEventEx, WriteProcessMemory, CONTEXT,
        DEBUG_EVENT, EXCEPTION_DEBUG_EVENT,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows::Win32::System::Memory::{
        VirtualProtectEx, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
    };
    use windows::Win32::System::Threading::{
        CreateProcessW, ResumeThread, TerminateProcess, CREATE_SUSPENDED, DEBUG_ONLY_THIS_PROCESS,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    // Win32 returns these as NTSTATUS; expose as raw i32 for ContinueDebugEvent.
    const DBG_CONTINUE: windows::Win32::Foundation::NTSTATUS =
        windows::Win32::Foundation::NTSTATUS(0x0001_0002u32 as i32);
    const DBG_EXCEPTION_NOT_HANDLED: windows::Win32::Foundation::NTSTATUS =
        windows::Win32::Foundation::NTSTATUS(0x8001_0001u32 as i32);
    // x86_64 CONTEXT_ALL = 0x10003F | 0x40 (XSTATE) — use canonical 0x10003F.
    const CONTEXT_ALL_AMD64: windows::Win32::System::Diagnostics::Debug::CONTEXT_FLAGS =
        windows::Win32::System::Diagnostics::Debug::CONTEXT_FLAGS(0x0010_003F);

    // NtQueryInformationProcess lives in ntdll; pull it in by hand.
    #[repr(C)]
    struct ProcessBasicInformation {
        exit_status: i32,
        peb_base_address: u64,
        affinity_mask: u64,
        base_priority: i32,
        _pad: [u8; 4],
        unique_process_id: u64,
        inherited_from_unique_process_id: u64,
    }

    type NtQueryFn = unsafe extern "system" fn(
        process: HANDLE,
        info_class: i32,
        info: *mut core::ffi::c_void,
        info_len: u32,
        return_len: *mut u32,
    ) -> i32;

    fn nt_query_information_process(h: HANDLE) -> Result<ProcessBasicInformation> {
        unsafe {
            let ntdll = GetModuleHandleA(PCSTR(b"ntdll.dll\0".as_ptr()))
                .context("GetModuleHandleA(ntdll.dll)")?;
            let sym = GetProcAddress(ntdll, PCSTR(b"NtQueryInformationProcess\0".as_ptr()))
                .ok_or_else(|| anyhow!("NtQueryInformationProcess not found"))?;
            let f: NtQueryFn = std::mem::transmute(sym);
            let mut pbi: ProcessBasicInformation = std::mem::zeroed();
            let mut ret_len: u32 = 0;
            let status = f(
                h,
                0, // ProcessBasicInformation
                &mut pbi as *mut _ as *mut _,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                &mut ret_len,
            );
            if status != 0 {
                anyhow::bail!("NtQueryInformationProcess failed: 0x{:08x}", status);
            }
            Ok(pbi)
        }
    }

    fn read_mem(hproc: HANDLE, addr: u64, n: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; n];
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                hproc,
                addr as *const _,
                buf.as_mut_ptr() as *mut _,
                n,
                Some(&mut read),
            )
        };
        if ok.is_ok() && read > 0 {
            buf.truncate(read);
            Some(buf)
        } else {
            None
        }
    }

    fn write_mem(hproc: HANDLE, addr: u64, data: &[u8]) -> bool {
        let mut wrote = 0usize;
        unsafe {
            WriteProcessMemory(
                hproc,
                addr as *const _,
                data.as_ptr() as *const _,
                data.len(),
                Some(&mut wrote),
            )
            .is_ok()
        }
    }

    fn write_with_protect(hproc: HANDLE, addr: u64, data: &[u8]) -> bool {
        let mut old = PAGE_PROTECTION_FLAGS(0);
        unsafe {
            let _ = VirtualProtectEx(
                hproc,
                addr as *const _,
                data.len(),
                PAGE_EXECUTE_READWRITE,
                &mut old,
            );
        }
        let ok = write_mem(hproc, addr, data);
        unsafe {
            let mut tmp = PAGE_PROTECTION_FLAGS(0);
            let _ = VirtualProtectEx(hproc, addr as *const _, data.len(), old, &mut tmp);
            let _ = FlushInstructionCache(hproc, Some(addr as *const _), data.len());
        }
        ok
    }

    struct SoftBp {
        addr: u64,
        name: &'static str,
        orig_byte: Option<u8>,
        armed: bool,
    }

    impl SoftBp {
        fn install(&mut self, h: HANDLE) -> bool {
            let orig = match read_mem(h, self.addr, 1) {
                Some(b) if !b.is_empty() => b[0],
                _ => return false,
            };
            self.orig_byte = Some(orig);
            let ok = write_with_protect(h, self.addr, &[0xCC]);
            if ok {
                self.armed = true;
            }
            ok
        }
        fn restore(&mut self, h: HANDLE) -> bool {
            let Some(b) = self.orig_byte else {
                return false;
            };
            let ok = write_with_protect(h, self.addr, &[b]);
            self.armed = false;
            ok
        }
        fn reinstall(&mut self, h: HANDLE) -> bool {
            let ok = write_with_protect(h, self.addr, &[0xCC]);
            if ok {
                self.armed = true;
            }
            ok
        }
    }

    fn patch_peb_anti_debug(h: HANDLE, peb_addr: u64, log: &mut dyn FnMut(String)) {
        if write_mem(h, peb_addr + 0x02, &[0u8]) {
            log("[+]   Patched PEB.BeingDebugged -> 0".into());
        }
        if let Some(flags) = read_mem(h, peb_addr + 0xBC, 4) {
            let val = u32::from_le_bytes(flags.try_into().unwrap_or([0; 4]));
            let anti = val & 0x70;
            if anti != 0 {
                let new = val & !0x70u32;
                write_mem(h, peb_addr + 0xBC, &new.to_le_bytes());
                log(format!(
                    "[+]   NtGlobalFlag: 0x{:08x} -> 0x{:08x}",
                    val, new
                ));
            } else {
                log(format!("[*]   NtGlobalFlag clean (0x{:08x})", val));
            }
        }
    }

    fn patch_anti_debug_hooks(h: HANDLE, log: &mut dyn FnMut(String)) -> bool {
        unsafe {
            let ntdll = match GetModuleHandleA(PCSTR(b"ntdll.dll\0".as_ptr())) {
                Ok(h) => h,
                Err(_) => {
                    log("[!] Cannot find ntdll.dll".into());
                    return false;
                }
            };
            let addr = match GetProcAddress(ntdll, PCSTR(b"NtSetInformationThread\0".as_ptr())) {
                Some(a) => a as u64,
                None => {
                    log("[!] NtSetInformationThread export missing".into());
                    return false;
                }
            };
            // 'xor eax, eax; ret'
            let patch: [u8; 3] = [0x31, 0xc0, 0xc3];
            let ok = write_with_protect(h, addr, &patch);
            if ok {
                log(format!(
                    "[+]   NtSetInformationThread @ 0x{:016x} neutralised",
                    addr
                ));
            } else {
                log("[!]   Failed to patch NtSetInformationThread".into());
            }
            ok
        }
    }

    /// Public: read previously-captured keys from `camellia_keys.txt`.
    pub fn load_existing_keys(output_dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let p = output_dir.join("camellia_keys.txt");
        if let Ok(s) = fs::read_to_string(&p) {
            for line in s.lines() {
                let line = line.trim();
                if line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()) {
                    out.push(line.to_ascii_lowercase());
                }
            }
        }
        out
    }

    pub fn run(opts: &CaptureOptions) -> Result<CaptureSummary> {
        fs::create_dir_all(&opts.output_dir)?;
        let log_path = opts.output_dir.join("capture_output.txt");
        let keys_path = opts.output_dir.join("camellia_keys.txt");

        let mut log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)?;

        let mut log = |msg: String| {
            println!("{}", msg);
            let _ = writeln!(log_file, "{}", msg);
        };

        log(format!(
            "=================================================="
        ));
        log(format!("  Soulframe Camellia-256 Key Capture (Rust)"));
        log(format!("  INLINE   RVA 0x{:08X}", opts.inline_rva));
        log(format!("  BACKUP   RVA 0x{:08X}", opts.backup_rva));
        log(format!("  Timeout: {}s", opts.timeout_secs));
        log(format!(
            "=================================================="
        ));

        if !opts.game_exe.exists() {
            anyhow::bail!("Game exe not found: {}", opts.game_exe.display());
        }

        // Build command line.
        let cmd_line = format!(
            "\"{}\" graphicsDriver:dx11 -threadedworker:1 -cluster:public -language:en -clienttype:Steam -deferred:0 ",
            opts.game_exe.display()
        );

        let game_dir = opts
            .game_exe
            .parent()
            .ok_or_else(|| anyhow!("game exe has no parent dir"))?
            .to_path_buf();

        log(format!(
            "[*] Launching SUSPENDED: {}",
            opts.game_exe.display()
        ));

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

        let exe_w: Vec<u16> = opts
            .game_exe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut cmd_w: Vec<u16> = cmd_line.encode_utf16().chain(std::iter::once(0)).collect();
        let dir_w: Vec<u16> = game_dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            CreateProcessW(
                PCWSTR(exe_w.as_ptr()),
                Some(windows::core::PWSTR(cmd_w.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED | DEBUG_ONLY_THIS_PROCESS,
                None,
                PCWSTR(dir_w.as_ptr()),
                &si,
                &mut pi,
            )
            .map_err(|e| anyhow!("CreateProcessW failed: {:?}", e))?;
        }

        let hproc = pi.hProcess;
        let hthread = pi.hThread;
        unsafe {
            let _ = DebugSetProcessKillOnExit(false);
        }
        log(format!(
            "[+] PID={} TID={} suspended",
            pi.dwProcessId, pi.dwThreadId
        ));

        // Get module base via PEB.ImageBaseAddress (offset 0x10).
        let pbi = nt_query_information_process(hproc)?;
        let base = read_mem(hproc, pbi.peb_base_address + 0x10, 8)
            .and_then(|b| b.try_into().ok().map(u64::from_le_bytes))
            .unwrap_or(0);
        if base < 0x10000 {
            unsafe {
                let _ = TerminateProcess(hproc, 1);
            }
            anyhow::bail!("Could not determine module base");
        }
        log(format!("[*] Module base: 0x{:016x}", base));

        // Install INT3 breakpoints.
        let mut bps: HashMap<u64, SoftBp> = HashMap::new();
        for (name, rva) in [
            ("CAMELLIA_INLINE", opts.inline_rva),
            ("OPENSSL_KEYSCHED", opts.backup_rva),
        ] {
            let addr = base + rva as u64;
            let mut sbp = SoftBp {
                addr,
                name,
                orig_byte: None,
                armed: false,
            };
            if sbp.install(hproc) {
                log(format!(
                    "[+] INT3 installed: {} @ 0x{:016x} (orig=0x{:02x})",
                    name,
                    addr,
                    sbp.orig_byte.unwrap_or(0)
                ));
                bps.insert(addr, sbp);
            } else {
                log(format!("[!] Failed to install INT3 for {}", name));
            }
        }

        // Resume.
        log("[*] Resuming process...".into());
        unsafe {
            ResumeThread(hthread);
        }

        // Debug loop.
        let mut hits = 0usize;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut keys_out: Vec<String> = Vec::new();
        let mut stepping: HashMap<u32, u64> = HashMap::new();
        let mut peb_patched = false;
        let mut sys_bp_count = 0;
        let t_start = Instant::now();
        let mut last_hit = t_start;
        let mut last_progress = t_start;

        loop {
            let mut ev: DEBUG_EVENT = unsafe { std::mem::zeroed() };
            let ok = unsafe { WaitForDebugEventEx(&mut ev, 5000) };
            if ok.is_err() {
                let err = unsafe { GetLastError() };
                // WaitForDebugEventEx reports a timeout as WAIT_TIMEOUT (258) on
                // most builds but as ERROR_SEM_TIMEOUT (121) on others — treat
                // both as "no event yet", not a fatal error.
                if err.0 == WAIT_TIMEOUT.0 || err.0 == ERROR_SEM_TIMEOUT.0 {
                    let elapsed = t_start.elapsed();
                    let since_hit = last_hit.elapsed();
                    if last_progress.elapsed() >= Duration::from_secs(10)
                        || elapsed < Duration::from_secs(5)
                    {
                        log(format!(
                            "[*] +{:.0}s  hits={} unique={} sys_bp={} peb={} (quiet {:.0}s)",
                            elapsed.as_secs_f64(),
                            hits,
                            seen.len(),
                            sys_bp_count,
                            peb_patched,
                            since_hit.as_secs_f64()
                        ));
                        last_progress = Instant::now();
                    }
                    if hits > 0 && since_hit.as_secs() > opts.quiet_detach_secs {
                        log(format!(
                            "[*] Quiet for {}s after captures — stopping.",
                            opts.quiet_detach_secs
                        ));
                        break;
                    }
                    if elapsed.as_secs() > opts.timeout_secs {
                        log(format!("[*] Timeout {}s reached.", opts.timeout_secs));
                        break;
                    }
                    continue;
                }
                log(format!("[!] WaitForDebugEvent error: {:?}", err));
                break;
            }

            let mut cont_status = DBG_CONTINUE;
            let tid = ev.dwThreadId;

            if ev.dwDebugEventCode == EXCEPTION_DEBUG_EVENT {
                let exc = unsafe { &ev.u.Exception };
                let exc_addr = exc.ExceptionRecord.ExceptionAddress as u64;
                let exc_code = exc.ExceptionRecord.ExceptionCode;

                if exc_code == EXCEPTION_BREAKPOINT {
                    // Map exception address back to one of our INT3s (try addr and addr-1).
                    let mut bp_addr = exc_addr;
                    let mut have = bps.contains_key(&bp_addr);
                    if !have && exc_addr > 0 {
                        bp_addr = exc_addr - 1;
                        have = bps.contains_key(&bp_addr);
                    }

                    if have {
                        let sbp_armed = bps.get(&bp_addr).map(|s| s.armed).unwrap_or(false);
                        if sbp_armed {
                            hits += 1;
                            last_hit = Instant::now();

                            // Restore original byte, snapshot context, back up RIP.
                            if let Some(sbp) = bps.get_mut(&bp_addr) {
                                sbp.restore(hproc);
                            }

                            let hth = open_thread(tid).ok();
                            let mut ctx = aligned_context();
                            ctx.ContextFlags = CONTEXT_ALL_AMD64;
                            let got_ctx = match hth {
                                Some(h) => unsafe { GetThreadContext(h, &mut ctx).is_ok() },
                                None => false,
                            };

                            if got_ctx {
                                ctx.Rip = bp_addr;

                                // Read 32 bytes from [RCX] = the key.
                                let key_ptr = ctx.Rcx;
                                let key_data = if key_ptr > 0x10000 {
                                    read_mem(hproc, key_ptr, 32).unwrap_or_default()
                                } else {
                                    Vec::new()
                                };

                                let bp_name = bps.get(&bp_addr).map(|s| s.name).unwrap_or("?");
                                log(format!(
                                    "[+] INT3 {} #{} TID={} RCX=0x{:x} RDX=0x{:x}",
                                    bp_name, hits, tid, ctx.Rcx, ctx.Rdx
                                ));
                                if key_data.len() == 32 {
                                    let hex_str = hex::encode(&key_data);
                                    let is_new = seen.insert(hex_str.clone());
                                    log(format!(
                                        "    KEY [RCX] (256b): {}{}",
                                        hex_str,
                                        if is_new { "  [NEW]" } else { "" }
                                    ));
                                    if is_new {
                                        keys_out.push(hex_str);
                                    }
                                } else {
                                    log(format!(
                                        "    Cannot read key @ RCX=0x{:x} (got {} bytes)",
                                        key_ptr,
                                        key_data.len()
                                    ));
                                }

                                // Trap flag → single step.
                                ctx.EFlags |= 0x100;
                                if let Some(h) = hth {
                                    unsafe {
                                        let _ = SetThreadContext(h, &ctx);
                                    }
                                }
                                stepping.insert(tid, bp_addr);
                            }
                            if let Some(h) = hth {
                                unsafe {
                                    let _ = CloseHandle(h);
                                }
                            }
                        }
                    } else {
                        // System breakpoint (e.g. loader BP) — perfect place to patch anti-debug.
                        sys_bp_count += 1;
                        log(format!(
                            "[*] System BP #{} @ 0x{:016x}",
                            sys_bp_count, exc_addr
                        ));
                        if !peb_patched {
                            patch_peb_anti_debug(hproc, pbi.peb_base_address, &mut log);
                            patch_anti_debug_hooks(hproc, &mut log);
                            peb_patched = true;
                        }
                    }
                } else if exc_code == EXCEPTION_SINGLE_STEP {
                    if let Some(bp_addr) = stepping.remove(&tid) {
                        if let Some(sbp) = bps.get_mut(&bp_addr) {
                            sbp.reinstall(hproc);
                        }
                    } else {
                        cont_status = DBG_EXCEPTION_NOT_HANDLED;
                    }
                } else {
                    cont_status = DBG_EXCEPTION_NOT_HANDLED;
                }
            }

            unsafe {
                let _ = ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, cont_status);
            }
        }

        let elapsed = t_start.elapsed().as_secs_f64();

        // Persist keys (merge with any existing) — match Python format (one hex per line).
        let mut all_keys: BTreeSet<String> =
            load_existing_keys(&opts.output_dir).into_iter().collect();
        for k in &keys_out {
            all_keys.insert(k.clone());
        }
        let body = all_keys.iter().cloned().collect::<Vec<_>>().join("\n");
        fs::write(&keys_path, body.as_bytes())?;
        log(format!(
            "[*] Wrote {} keys to {} (this run: {} new)",
            all_keys.len(),
            keys_path.display(),
            keys_out.len()
        ));

        // The game outlives the debugger here on purpose: scan-rewards walks
        // its heap next. Shutting it down is the caller's call, via
        // `launched_pid` — see `Cmd::RunAll`.
        // (Python `DebugSetProcessKillOnExit(False)` set the same policy.)

        Ok(CaptureSummary {
            hits,
            unique_keys: seen.len(),
            keys_hex: keys_out,
            elapsed_secs: elapsed,
            launched_pid: pi.dwProcessId,
        })
    }

    /// Terminate one process by PID.
    ///
    /// Deliberately PID-scoped rather than by image name: a Soulframe the user
    /// launched themselves must survive, so only the instance whose PID we were
    /// handed gets killed. A PID that has already exited is not an error.
    pub fn terminate_pid(pid: u32) -> Result<()> {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE};

        let handle = match unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
            Ok(h) => h,
            // Already gone (or never ours to open) — nothing to do.
            Err(_) => return Ok(()),
        };
        let result = unsafe { TerminateProcess(handle, 0) }
            .map_err(|e| anyhow!("TerminateProcess({pid}) failed: {e:?}"));
        unsafe {
            let _ = CloseHandle(handle);
        }
        result
    }

    // ----- helpers that need extra imports -----

    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Threading::{OpenThread, THREAD_ALL_ACCESS};

    fn open_thread(tid: u32) -> Result<HANDLE> {
        unsafe { OpenThread(THREAD_ALL_ACCESS, false, tid) }
            .map_err(|e| anyhow!("OpenThread({}) failed: {:?}", tid, e))
    }

    // CONTEXT must be 16-byte aligned for SSE state. Allocate via repr(align).
    #[repr(align(16))]
    struct AlignedCtx(CONTEXT);

    fn aligned_context() -> CONTEXT {
        let a = AlignedCtx(unsafe { std::mem::zeroed::<CONTEXT>() });
        a.0
    }
}
