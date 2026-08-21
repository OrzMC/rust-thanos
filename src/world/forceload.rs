//! Force-loaded chunk coordinate parsing.
//!
//! Probes the two known file locations in priority order and uses the FIRST
//! one that exists, mirroring OrzMCBackup's `ForceLoad.kt`:
//!   1. `data/minecraft/chunk_tickets.dat` — Minecraft 1.21.6+/26.1+
//!   2. `data/chunks.dat` — legacy Forge format
//!
//! Within the chosen file, both NBT structures are supported (matching
//! `NbtForceLoader.kt`):
//!   - `data.Forced` (LongArray) — legacy packed 64-bit x/z pairs
//!   - `data.tickets[].chunk_pos` (IntArray[2]) with type `minecraft:forced`

use crate::world::fs::{FileSystem, RealFileSystem};
use fastnbt::Value;
use std::path::Path;

/// Files probed in priority order for force-loaded chunk data.
const FORCE_LOAD_FILES: [&str; 2] = ["data/minecraft/chunk_tickets.dat", "data/chunks.dat"];

/// Parse force-loaded chunk coordinates for a dimension on the real filesystem.
///
/// Convenience for callers that always use the disk; the engine uses the
/// [`FileSystem`]-aware variant so memory-backed tests behave identically.
pub fn parse_force_loaded(dimension: &Path) -> Vec<(i32, i32)> {
    parse_force_loaded_fs(&RealFileSystem, dimension)
}

/// Parse force-loaded chunk coordinates for a dimension.
///
/// Returns the coordinates from the first force-load file that exists, or an
/// empty list when none exists / none can be parsed — a force-loaded pattern
/// that keeps nothing is the safe fallback. Mirrors OrzMCBackup `ForceLoad.parse`.
pub fn parse_force_loaded_fs(fs: &dyn FileSystem, dimension: &Path) -> Vec<(i32, i32)> {
    for rel in FORCE_LOAD_FILES {
        let f = dimension.join(rel);
        if fs.is_regular_file(&f) {
            return match parse_file(fs, &f) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!(
                        "Failed to parse force-loaded chunks from {}: {}",
                        f.display(),
                        e
                    );
                    Vec::new()
                }
            };
        }
    }
    Vec::new()
}

fn parse_file(fs: &dyn FileSystem, path: &Path) -> anyhow::Result<Vec<(i32, i32)>> {
    let data = match fs.read(path)? {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };
    let mut dec = flate2::read::GzDecoder::new(&data[..]);
    let mut out = Vec::new();
    std::io::copy(&mut dec, &mut out)?;
    let v = fastnbt::from_bytes::<Value>(&out)?;
    let Value::Compound(root) = v else {
        return Ok(Vec::new());
    };
    let Some(Value::Compound(data_tag)) = root.get("data") else {
        return Ok(Vec::new());
    };
    let mut coords = Vec::new();
    // Legacy Forge format: data.Forced is a LongArray of packed pairs.
    if let Some(Value::LongArray(arr)) = data_tag.get("Forced") {
        let mut it = arr.iter();
        while let (Some(a), Some(b)) = (it.next(), it.next()) {
            coords.push((*a as i32, *b as i32));
        }
    }
    // Modern tickets format: data.tickets[] with type == "minecraft:forced".
    if let Some(Value::List(list)) = data_tag.get("tickets") {
        for t in list {
            let Value::Compound(tm) = t else { continue };
            let matches_type = tm
                .get("type")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.as_str() == "minecraft:forced"),
                    _ => None,
                })
                .unwrap_or(false);
            if !matches_type {
                continue;
            }
            if let Some(Value::IntArray(pos)) = tm.get("chunk_pos") {
                if pos.len() == 2 {
                    coords.push((pos[0], pos[1]));
                }
            }
        }
    }
    Ok(coords)
}
