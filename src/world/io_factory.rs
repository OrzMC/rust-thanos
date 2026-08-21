//! MCA IO factory.
//!
//! Mirrors OrzMCBackup's `McaIOFactory.kt`: the engine obtains readers/writers
//! through a factory so the same pipeline can target the real filesystem
//! ([`DefaultMcaIOFactory`]) or an in-memory tree ([`MemoryMcaIOFactory`]).

use crate::mca::entry::McaEntry;
use crate::mca::reader::McaReader;
use crate::mca::writer::McaWriter;
use crate::world::fs::{FileSystem, MemoryFS};
use anyhow::{anyhow, Result};
use std::path::Path;
use std::sync::Arc;

/// Read-only view of an MCA region file.
pub trait McaReaderLike: Send {
    /// All chunk entries in the region file.
    fn entries(&mut self) -> Result<Vec<McaEntry>>;
    /// Count used chunk slots without allocating entries.
    fn count(&mut self) -> Result<usize>;
    /// Get one chunk by sector index (0-1023), or null if unused.
    fn get(&mut self, index: usize) -> Result<Option<McaEntry>>;
}

/// Write handle for an MCA region file.
pub trait McaWriterLike: Send {
    /// Write one chunk entry (4 KiB sector-aligned).
    fn write_entry(&mut self, entry: &mut McaEntry) -> Result<()>;
    /// Flush the location/timestamp header tables.
    fn finalize_file(&mut self) -> Result<()>;
    /// True if no chunk was written; a fully-removed region must not produce a file.
    fn is_empty(&self) -> bool;
}

impl McaReaderLike for McaReader {
    fn entries(&mut self) -> Result<Vec<McaEntry>> {
        McaReader::entries(self)
    }
    fn count(&mut self) -> Result<usize> {
        McaReader::count(self)
    }
    fn get(&mut self, index: usize) -> Result<Option<McaEntry>> {
        McaReader::get(self, index)
    }
}

impl McaWriterLike for McaWriter {
    fn write_entry(&mut self, entry: &mut McaEntry) -> Result<()> {
        McaWriter::write_entry(self, entry)
    }
    fn finalize_file(&mut self) -> Result<()> {
        McaWriter::finalize(self)
    }
    fn is_empty(&self) -> bool {
        McaWriter::is_empty(self)
    }
}

/// Factory for creating [`McaReaderLike`] and [`McaWriterLike`] for a [`FileSystem`].
pub trait McaIOFactory: Send + Sync {
    fn open_reader(&self, fs: Arc<dyn FileSystem>, path: &Path) -> Result<Box<dyn McaReaderLike>>;
    fn create_writer(
        &self,
        fs: Arc<dyn FileSystem>,
        path: &Path,
        sync_on_finalize: bool,
    ) -> Result<Box<dyn McaWriterLike>>;
}

/// Default factory backed by the real filesystem.
pub struct DefaultMcaIOFactory;

impl McaIOFactory for DefaultMcaIOFactory {
    fn open_reader(&self, _fs: Arc<dyn FileSystem>, path: &Path) -> Result<Box<dyn McaReaderLike>> {
        Ok(Box::new(McaReader::open(&path.to_string_lossy())?))
    }
    fn create_writer(
        &self,
        _fs: Arc<dyn FileSystem>,
        path: &Path,
        sync_on_finalize: bool,
    ) -> Result<Box<dyn McaWriterLike>> {
        Ok(Box::new(
            McaWriter::open(&path.to_string_lossy())?.with_sync_on_finalize(sync_on_finalize),
        ))
    }
}

/// Factory backed by an in-memory tree: readers parse bytes from the
/// [`MemoryFS`], writers accumulate and flush back to it at finalize.
pub struct MemoryMcaIOFactory;

impl McaIOFactory for MemoryMcaIOFactory {
    fn open_reader(&self, fs: Arc<dyn FileSystem>, path: &Path) -> Result<Box<dyn McaReaderLike>> {
        let mem = downcast_mem(fs)?;
        let bytes = mem.read(path)?.unwrap_or_default();
        Ok(Box::new(McaReader::open_from_bytes(
            &path.to_string_lossy(),
            bytes,
        )?))
    }
    fn create_writer(
        &self,
        fs: Arc<dyn FileSystem>,
        path: &Path,
        _sync_on_finalize: bool,
    ) -> Result<Box<dyn McaWriterLike>> {
        let mem = downcast_mem(fs)?;
        Ok(Box::new(MemoryMcaWriter::new(mem, path)))
    }
}

fn downcast_mem(fs: Arc<dyn FileSystem>) -> Result<Arc<MemoryFS>> {
    fs.as_any_arc()
        .downcast::<MemoryFS>()
        .map_err(|_| anyhow!("MemoryMcaIOFactory requires MemoryFS"))
}

/// In-memory MCA writer. Accumulates chunk data and writes the whole file to a
/// [`MemoryFS`] at [`finalize_file`] — mirrors OrzMCBackup's `MemoryMcaWriter`
/// (ByteArrayOutputStream accumulate + `mem.write` at finalize).
pub struct MemoryMcaWriter {
    mem: Arc<MemoryFS>,
    path: std::path::PathBuf,
    data: Vec<u8>,
    written_any: bool,
    data_offset: usize,
    offsets: [u32; 1024],
    sizes: [u32; 1024],
    timestamps: [u32; 1024],
}

impl MemoryMcaWriter {
    pub fn new(mem: Arc<MemoryFS>, path: &Path) -> Self {
        let mut data = Vec::with_capacity(8192);
        data.extend_from_slice(&[0u8; 8192]);
        Self {
            mem,
            path: path.to_path_buf(),
            data,
            written_any: false,
            data_offset: 8192,
            offsets: [0; 1024],
            sizes: [0; 1024],
            timestamps: [0; 1024],
        }
    }
}

impl McaWriterLike for MemoryMcaWriter {
    fn write_entry(&mut self, entry: &mut McaEntry) -> Result<()> {
        let serialized = entry.serialized_bytes()?;
        let start = self.data_offset;
        self.data.extend_from_slice(&serialized);
        let written = serialized.len();
        let pad = (4096 - (written % 4096)) % 4096;
        self.data.extend(vec![0u8; pad]);
        self.data_offset += written + pad;
        let idx = entry.region_index() as usize;
        self.offsets[idx] = start as u32;
        self.sizes[idx] = (written + pad) as u32;
        self.timestamps[idx] = entry.modified_time();
        self.written_any = true;
        Ok(())
    }

    fn finalize_file(&mut self) -> Result<()> {
        // A fully-removed region must not produce a file, matching the lazy
        // real-filesystem writer.
        if !self.written_any {
            return Ok(());
        }
        let mut loc = Vec::with_capacity(4096);
        for i in 0..1024 {
            let off = self.offsets[i] / 4096;
            let size = self.sizes[i] / 4096;
            let v = (off << 8) | (size & 0xFF);
            loc.extend_from_slice(&v.to_be_bytes());
        }
        let mut time = Vec::with_capacity(4096);
        for i in 0..1024 {
            time.extend_from_slice(&self.timestamps[i].to_be_bytes());
        }
        self.data[0..4096].copy_from_slice(&loc);
        self.data[4096..8192].copy_from_slice(&time);
        self.mem.write(&self.path, &self.data)?;
        Ok(())
    }

    fn is_empty(&self) -> bool {
        !self.written_any
    }
}
