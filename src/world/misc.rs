//! Miscellaneous (non-MCA) file copying.
//!
//! After region processing the output must be a usable world, so non-MCA files
//! are copied from the discovered dimensions AND their non-dimension ancestors
//! — the "misc parents" of a 26.1+ Paper layout (`world/` → `world/dimensions/
//! minecraft/overworld/`), where `world/level.dat`, `world/players/`, etc. live
//! outside any dimension directory. Mirrors OrzMCBackup's `miscRel` /
//! `discoverMiscParents` / `copyMiscFiles`.
//!
//! What gets skipped:
//! - the walk root itself and entries inside another dimension's subtree
//!   (that dimension's source walk handles them)
//! - files matching `session.lock`
//! - top-level `.mca` files directly inside `region`/`entities`/`poi` (the
//!   dimension processor rewrites them); nested `.mca` and non-`.mca` files
//!   (e.g. `.bak`/`.backup`) in those subtrees ARE misc and must be kept.

use crate::world::fs::FileSystem;
use crate::world::report::ErrorKind;
use std::path::{Path, PathBuf};

/// Directories whose top-level `.mca` files are rewritten by the dimension
/// processor rather than copied.
const RESERVED: &[&str] = &["region", "entities", "poi"];
/// Never copied, regardless of location (a stale session lock must not leak
/// into a fresh output).
const SKIP_FILES: &[&str] = &["session.lock"];

fn is_dimension_dir(fs: &dyn FileSystem, path: &Path) -> bool {
    fs.is_directory(&path.join("region"))
}

/// Ancestor directories of the discovered dimensions that hold world-level misc
/// files in a 26.1+ Paper layout. Empty for the classic flat layout where the
/// world root is itself a dimension (its own misc is covered by its source walk).
pub fn discover_misc_parents(fs: &dyn FileSystem, input: &Path, dims: &[PathBuf]) -> Vec<PathBuf> {
    let mut parents: Vec<PathBuf> = Vec::new();
    for dim in dims {
        if dim == input {
            continue;
        }
        let mut parent = dim.parent();
        while let Some(p) = parent {
            if p == input {
                break;
            }
            if !is_dimension_dir(fs, p) && !parents.contains(&p.to_path_buf()) {
                parents.push(p.to_path_buf());
            }
            parent = p.parent();
        }
    }
    // Input itself isn't a dimension but directly contains dimensions
    // (a 26.1+ world root): copy world-level files too. `dimensions/` is the
    // definitive marker of this layout.
    if !is_dimension_dir(fs, input)
        && fs.is_directory(&input.join("dimensions"))
        && !parents.contains(&input.to_path_buf())
    {
        parents.push(input.to_path_buf());
    }
    parents
}

/// Decides whether a walked misc-file path `p` under `dir` is a real misc file
/// to keep, returning its path relative to `dir` (or `None` to skip).
fn misc_rel(dir: &Path, p: &Path, exclude_paths: &[PathBuf]) -> Option<PathBuf> {
    if p == dir {
        return None;
    }
    // Exclude a path only when it lies inside a dimension subtree that some
    // OTHER source walk handles. An ancestor of `dir` in exclude_paths (e.g. a
    // world root that is itself a dimension and nests DIM-1/DIM1 below it) must
    // NOT hide `dir`'s own misc files.
    for ex in exclude_paths {
        if !dir.starts_with(ex) && p.starts_with(ex) {
            return None;
        }
    }
    let rel = p.strip_prefix(dir).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let file_name = rel.file_name()?.to_string_lossy().to_string();
    if SKIP_FILES.contains(&file_name.as_str()) {
        return None;
    }
    let depth = rel.components().count();
    let top = rel
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .to_string();
    if RESERVED.contains(&top.as_str()) && depth == 2 && file_name.ends_with(".mca") {
        return None;
    }
    Some(rel.to_path_buf())
}

/// Copy misc files from `sources` into `out`, preserving each source's path
/// relative to `input`. Records per-file failures via `record` instead of
/// aborting the copy (collect-don't-throw).
pub fn copy_misc_files(
    fs: &dyn FileSystem,
    input: &Path,
    out: &Path,
    sources: &[PathBuf],
    exclude_paths: &[PathBuf],
    record: &dyn Fn(&Path, ErrorKind, &str),
) {
    for dir in sources {
        let rel = dir.strip_prefix(input).unwrap_or(dir);
        let out_dir = out.join(rel);
        if let Err(e) = fs.create_directories(&out_dir) {
            record(
                dir,
                ErrorKind::CopyMisc,
                &format!("Failed to create directory: {}", e),
            );
            continue;
        }
        let Ok(walked) = fs.walk(dir, true) else {
            continue;
        };
        for p in walked {
            let Some(rel_path) = misc_rel(dir, &p, exclude_paths) else {
                continue;
            };
            let target = out_dir.join(&rel_path);
            if fs.is_directory(&p) {
                if let Err(e) = fs.create_directories(&target) {
                    record(
                        &p,
                        ErrorKind::CopyMisc,
                        &format!("Failed to create directory: {}", e),
                    );
                }
            } else {
                if let Some(parent) = target.parent() {
                    if let Err(e) = fs.create_directories(parent) {
                        record(
                            &p,
                            ErrorKind::CopyMisc,
                            &format!("Failed to create directory: {}", e),
                        );
                    }
                }
                if let Err(e) = fs.copy(&p, &target, true) {
                    record(
                        &p,
                        ErrorKind::CopyMisc,
                        &format!("Failed to copy file: {}", e),
                    );
                }
            }
        }
    }
}
