//! ZIP compression for world output directories.
//!
//! Moved out of `main.rs` so the engine can offer `--zip-output` through the
//! filesystem abstraction. Entry names always use `/` separators (the ZIP spec
//! requires it; Windows `Path` uses `\`, which would make Unix unzip tools
//! treat the whole path as a single file name).

use crate::world::fs::FileSystem;
use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::FileOptions;

/// Compress `root` into a sibling `yyyyMMddHHmmss.zip` next to it (outside the
/// walked tree), and return the archive path. Mirrors OrzMCBackup's
/// `Compressor.compressToTimestampZip`.
pub fn compress_to_timestamp_zip(fs: &dyn FileSystem, root: &Path) -> Result<PathBuf> {
    let ts = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
    let parent = root
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let zip_path = parent.join(format!("{}.zip", ts));

    let mut data = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut data));
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for p in fs.walk(root, false)? {
            let rel = p.strip_prefix(root)?;
            if rel.as_os_str().is_empty() {
                continue; // the walk root itself
            }
            let name = rel.to_string_lossy().replace('\\', "/");
            if fs.is_directory(&p) {
                zip.add_directory(format!("{}/", name), options)?;
            } else if let Some(bytes) = fs.read(&p)? {
                zip.start_file(name, options)?;
                zip.write_all(&bytes)?;
            }
        }
        zip.finish()?;
    }
    fs.write(&zip_path, &data)?;
    Ok(zip_path)
}
