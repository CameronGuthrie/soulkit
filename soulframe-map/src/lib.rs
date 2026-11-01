//! Midrath Redux map extraction.
//!
//! The map tile path is format-specific: the in-cache `_cmm.png` entries are
//! Soulframe DDS-backed LightMap records and are converted to WebP. The JSON
//! path pairs structured level headers and property bodies by record ordinal,
//! validates their instance IDs, and emits traceable authored POI placements.
//! Procedural dungeon room navmeshes are rendered as WebP floor plans, one
//! `<setKey>/{n}.webp` map per tileset with a `rooms.json` index.

pub mod cli;
mod dungeon_tiles;
mod dungeon_layouts;
mod dungeons;
mod level;
mod map_json;
mod properties;
mod tiles;
