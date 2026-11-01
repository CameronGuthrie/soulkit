//! Soulframe game-cache parsing library.
//!
//! This crate provides shared functionality for reading the Soulframe game's
//! on-disk cache format (`*.toc` / `*.cache` file pairs) and decompressing
//! cache entries via Oodle.
//!
//! It was extracted from [`soulframe-image-extractor-rust`](../soulframe-image-extractor-rust)
//! so that the image extractor, drop-table pipeline and item-generation
//! pipeline can share a single, vetted implementation of the cache format.
//!
//! # Quick start
//!
//! ```no_run
//! use soulframe_cache::{detect_soulframe_cache, load_local_cache, toc_find_path, cache_get_toc_entry, Oodle};
//!
//! # fn main() -> anyhow::Result<()> {
//! let cache_dir = detect_soulframe_cache()
//!     .expect("Soulframe cache directory not found");
//!
//! // Load the H.Misc cache pair.
//! let (entries, cache_bytes) = load_local_cache(&cache_dir, "H.Misc")?
//!     .expect("H.Misc cache pair missing");
//!
//! // Open the Oodle decompressor (loads oo2core_9.dll).
//! let oodle = Oodle::new()?;
//!
//! // Find a specific entry and decompress it.
//! if let Some(entry) = toc_find_path(&entries, "Packages.bin") {
//!     let bytes = cache_get_toc_entry(&oodle, &cache_bytes, entry)?;
//!     println!("Packages.bin: {} bytes", bytes.len());
//! }
//! # Ok(()) }
//! ```
//!
//! # Locating `oo2core_9.dll`
//!
//! [`Oodle::new`] searches in this order:
//!
//! 1. The path in the `SOULFRAME_OODLE_LIB` environment variable, if set.
//! 2. `lib/oo2core_9.dll` next to the current executable.
//! 3. `lib/oo2core_9.dll` next to the current working directory.
//!
//! Use [`Oodle::with_path`] to load from an explicit location instead.

mod cache;
mod header;
mod oodle;
mod toc;

pub use cache::{
    cache_get_toc_entry, detect_soulframe_cache, load_local_cache, shcc_decompress_chunk_oodle,
    LazyCache,
};
pub use header::{parse_common_header, CommonHeader};
pub use oodle::Oodle;
pub use toc::{toc_decode, toc_find_path, toc_get_path, TocEntry};
