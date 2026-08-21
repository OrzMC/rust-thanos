use crate::mca::access::{FileAccess, RandomAccess};
use crate::mca::entry::McaEntry;
use anyhow::Result;

/// Writes Minecraft Anvil region files (.mca) through a [`RandomAccess`]
/// handle, only touching the filesystem when the first chunk is written.
///
/// A region whose every chunk is removed never produces an output file, so the
/// three-file lockstep (region/entities/poi) won't leave empty `.mca` files on
/// disk. Mirrors OrzMCBackup's lazy writer behavior.
pub struct McaWriter {
    /// Output path, used to lazily open the backing file on the first write.
    path: Option<String>,
    /// Backing access, created on the first `write_entry`. `None` until then.
    access: Option<Box<dyn RandomAccess>>,
    /// Sync to disk on `finalize`. Phase 3 exposes `--no-fsync` through this.
    sync_on_finalize: bool,
    /// True once any chunk has been written; drives `is_empty()` so a fully
    /// removed region reports empty whether backed by a file or a buffer.
    written_any: bool,
    data_offset: u64,
    offsets: Vec<u32>,
    sizes: Vec<u32>,
    timestamps: Vec<u32>,
}

impl McaWriter {
    /// Open an output region, deferring file creation until the first chunk.
    pub fn open(path: &str) -> Result<Self> {
        Ok(Self {
            path: Some(path.to_string()),
            access: None,
            sync_on_finalize: true,
            written_any: false,
            data_offset: 8192,
            offsets: vec![0; 1024],
            sizes: vec![0; 1024],
            timestamps: vec![0; 1024],
        })
    }

    /// Set whether `finalize` flushes to durable storage (`--no-fsync`).
    pub fn with_sync_on_finalize(mut self, sync: bool) -> Self {
        self.sync_on_finalize = sync;
        self
    }

    /// Whether any chunk has been written. `false` means `finalize` is a no-op
    /// and no file exists on disk.
    pub fn is_empty(&self) -> bool {
        !self.written_any
    }

    /// Lazily create the backing access and write the 8192-byte header.
    fn ensure_open(&mut self) -> Result<()> {
        if self.access.is_some() {
            return Ok(());
        }
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("writer has no output path"))?;
        let mut access: Box<dyn RandomAccess> = Box::new(FileAccess::open_rw(path)?);
        // Pre-zero the 8192-byte header so sectors 0-1 read back cleanly.
        access.seek(0)?;
        access.write_all(&[0u8; 8192])?;
        self.access = Some(access);
        Ok(())
    }

    pub fn write_entry(&mut self, entry: &mut McaEntry) -> Result<()> {
        self.ensure_open()?;
        let access = self.access.as_mut().unwrap();
        let serialized = entry.serialized_bytes()?;
        let start = self.data_offset;
        access.seek(start)?;
        access.write_all(&serialized)?;
        let written = serialized.len() as u64;
        let pad = (4096 - (written % 4096)) % 4096;
        if pad > 0 {
            access.write_all(&vec![0u8; pad as usize])?;
        }
        self.data_offset += written + pad;
        let idx = entry.region_index() as usize;
        self.offsets[idx] = start as u32;
        self.sizes[idx] = (written + pad) as u32;
        self.timestamps[idx] = entry.modified_time();
        self.written_any = true;
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<()> {
        // No chunks were written: the access was never created, nothing to do.
        if !self.written_any {
            return Ok(());
        }
        let Some(access) = self.access.as_mut() else {
            return Ok(());
        };
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
        access.seek(0)?;
        access.write_all(&loc)?;
        access.write_all(&time)?;
        if self.sync_on_finalize {
            access.sync()?;
        }
        Ok(())
    }
}
