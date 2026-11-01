//! Oodle decompression FFI wrapper.
//!
//! Oodle is a proprietary compression library shipped with the Soulframe
//! client as `oo2core_9.dll`. This module loads it dynamically at runtime
//! via `libloading` and exposes a single `decompress` method.

use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};

/// Environment variable consulted by [`Oodle::new`] to locate `oo2core_9.dll`.
pub const OODLE_LIB_ENV: &str = "SOULFRAME_OODLE_LIB";

/// Dynamic wrapper around `OodleLZ_Decompress` from `oo2core_9.dll`.
pub struct Oodle {
    // Field order matters: the symbol borrows from the library, so the
    // library must outlive it. Rust drops fields in declaration order, so
    // `decompress_fn` is dropped first.
    decompress_fn: Symbol<
        'static,
        unsafe extern "C" fn(
            *const c_char,
            usize,
            *mut c_void,
            usize,
            c_int,
            c_int,
            c_int,
            usize,
            usize,
            usize,
            usize,
            usize,
            usize,
            c_int,
        ) -> c_int,
    >,
    #[allow(dead_code)]
    lib: Library,
}

impl Oodle {
    /// Load Oodle, searching the standard set of locations.
    ///
    /// Search order:
    /// 1. `$SOULFRAME_OODLE_LIB`
    /// 2. `lib/oo2core_9.dll` next to the current executable
    /// 3. `lib/oo2core_9.dll` next to the current working directory
    pub fn new() -> Result<Self> {
        let override_path = std::env::var_os(OODLE_LIB_ENV);
        Self::from_override(override_path.as_deref().map(Path::new))
    }

    fn from_override(override_path: Option<&Path>) -> Result<Self> {
        // An explicit override must not silently load a different copy when
        // its path is missing or invalid (including a GUI-selected folder).
        if let Some(path) = override_path {
            return Self::with_path(path);
        }
        for candidate in default_search_paths() {
            if candidate.exists() {
                return Self::with_path(&candidate);
            }
        }
        Err(anyhow!(
            "Could not locate {lib}. Set the {env} environment variable, or place it at <exe-dir>/lib/{lib} or <cwd>/lib/{lib}.",
            lib = oodle_lib_filename(),
            env = OODLE_LIB_ENV,
        ))
    }

    /// Load Oodle from an explicit path.
    pub fn with_path(lib_path: &Path) -> Result<Self> {
        unsafe {
            let lib = Library::new(lib_path).with_context(|| {
                format!("Failed to load Oodle library from {}", lib_path.display())
            })?;

            let decompress_fn: Symbol<
                unsafe extern "C" fn(
                    *const c_char,
                    usize,
                    *mut c_void,
                    usize,
                    c_int,
                    c_int,
                    c_int,
                    usize,
                    usize,
                    usize,
                    usize,
                    usize,
                    usize,
                    c_int,
                ) -> c_int,
            > = lib
                .get(b"OodleLZ_Decompress\0")
                .context("OodleLZ_Decompress symbol not found in Oodle library")?;

            // Extend the lifetime to 'static. Safe because:
            // - `lib` is owned by `Self` and dropped after `decompress_fn`
            //   (fields are dropped in declaration order).
            // - The symbol is never exposed without going through `Self`.
            let decompress_fn: Symbol<'static, _> = std::mem::transmute(decompress_fn);

            Ok(Self { lib, decompress_fn })
        }
    }

    /// Decompress an Oodle-compressed buffer of known decompressed size.
    pub fn decompress(&self, compressed: &[u8], decompressed_size: usize) -> Result<Vec<u8>> {
        let mut output = vec![0u8; decompressed_size];

        let actual = unsafe {
            (self.decompress_fn)(
                compressed.as_ptr() as *const c_char,
                compressed.len(),
                output.as_mut_ptr() as *mut c_void,
                decompressed_size,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                3,
            )
        };

        if actual as usize != decompressed_size {
            return Err(anyhow!(
                "Oodle decompression failed: expected {} bytes, got {}",
                decompressed_size,
                actual
            ));
        }

        Ok(output)
    }
}

fn oodle_lib_filename() -> &'static str {
    if cfg!(windows) {
        "oo2core_9.dll"
    } else {
        "oo2core_9.so"
    }
}

fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let filename = oodle_lib_filename();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("lib").join(filename));
            paths.push(dir.join(filename));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("lib").join(filename));
        paths.push(cwd.join(filename));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_explicit_override_does_not_fall_back_to_installed_oodle() {
        let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/missing-override-regression/oo2core_9.dll");
        assert!(!missing.exists());
        let error = match Oodle::from_override(Some(&missing)) {
            Ok(_) => panic!("An explicit missing override must fail"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains(&missing.to_string_lossy().to_string()));
    }
}
