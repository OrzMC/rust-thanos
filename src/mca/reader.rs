use crate::mca::access::{FileAccess, MemoryAccess, RandomAccess};
use crate::mca::entry::McaEntry;
use anyhow::{anyhow, Result};
use regex::Regex;

/// Reads Minecraft Anvil region files (.mca).
///
/// The underlying data is accessed through [`RandomAccess`], so the same reader
/// works on the real filesystem (`File`) or in-memory buffers (`MemoryAccess`).
/// Mirrors OrzMCBackup's `McaReader`.
pub struct McaReader {
    access: Box<dyn RandomAccess>,
    x_pos: i32,
    z_pos: i32,
    offsets: Option<Vec<u64>>,
    sizes: Option<Vec<u64>>,
    timestamps: Option<Vec<u32>>,
}

impl McaReader {
    /// Open a region file from disk. The filename must match `r.x.z.mca`.
    pub fn open(path: &str) -> Result<Self> {
        let (x, z) = parse_coords(path)?;
        let access: Box<dyn RandomAccess> = Box::new(FileAccess::open(path)?);
        Ok(Self::new(access, x, z))
    }

    /// Open a region file from in-memory bytes. Useful for testing and for the
    /// `MemoryMcaIOFactory`. Mirrors OrzMCBackup `McaReader.openFromBytes`.
    pub fn open_from_bytes(path: &str, bytes: Vec<u8>) -> Result<Self> {
        let (x, z) = parse_coords(path)?;
        let access: Box<dyn RandomAccess> = Box::new(MemoryAccess::new(bytes));
        Ok(Self::new(access, x, z))
    }

    /// Build a reader over an injected access handle with explicit coordinates.
    pub fn new(access: Box<dyn RandomAccess>, x_pos: i32, z_pos: i32) -> Self {
        Self {
            access,
            x_pos,
            z_pos,
            offsets: None,
            sizes: None,
            timestamps: None,
        }
    }

    fn read_header(&mut self) -> Result<()> {
        self.access.seek(0)?;
        let mut loc = vec![0u8; 4096];
        self.access.read_exact(&mut loc)?;
        let mut time = vec![0u8; 4096];
        self.access.read_exact(&mut time)?;
        let mut offsets = Vec::with_capacity(1024);
        let mut sizes = Vec::with_capacity(1024);
        for i in 0..1024 {
            let base = i * 4;
            let v = u32::from_be_bytes([loc[base], loc[base + 1], loc[base + 2], loc[base + 3]]);
            // Compute in u64: a corrupt location entry can push (v>>8)*4096 past
            // u32::MAX (e.g. all-0xFF header), which used to overflow here.
            let off = ((v >> 8) as u64) * 4096;
            let size = ((v & 0xFF) as u64) * 4096;
            offsets.push(off);
            sizes.push(size);
        }
        let mut timestamps = Vec::with_capacity(1024);
        for i in 0..1024 {
            let base = i * 4;
            let v =
                u32::from_be_bytes([time[base], time[base + 1], time[base + 2], time[base + 3]]);
            timestamps.push(v);
        }
        self.offsets = Some(offsets);
        self.sizes = Some(sizes);
        self.timestamps = Some(timestamps);
        Ok(())
    }

    fn ensure(&mut self) -> Result<()> {
        if self.offsets.is_none() {
            self.read_header()?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn x_pos(&self) -> i32 {
        self.x_pos
    }
    #[allow(dead_code)]
    pub fn z_pos(&self) -> i32 {
        self.z_pos
    }

    pub fn entries(&mut self) -> Result<Vec<McaEntry>> {
        self.ensure()?;
        let offsets = self.offsets.as_ref().unwrap();
        let sizes = self.sizes.as_ref().unwrap();
        let timestamps = self.timestamps.as_ref().unwrap();
        let mut out = Vec::new();
        for i in 0..1024 {
            let off = offsets[i];
            let size = sizes[i];
            let ts = timestamps[i];
            if off == 0 || size == 0 {
                continue;
            }
            out.push(McaEntry::new(
                self.access.try_clone()?,
                off,
                size as usize,
                i as u32,
                ts,
                self.x_pos,
                self.z_pos,
            ));
        }
        Ok(out)
    }

    /// Count used chunk slots without allocating entries.
    pub fn count(&mut self) -> Result<usize> {
        self.ensure()?;
        let offsets = self.offsets.as_ref().unwrap();
        let sizes = self.sizes.as_ref().unwrap();
        Ok((0..1024)
            .filter(|&i| offsets[i] != 0 && sizes[i] != 0)
            .count())
    }

    pub fn get(&mut self, index: usize) -> Result<Option<McaEntry>> {
        self.ensure()?;
        let offsets = self.offsets.as_ref().unwrap();
        let sizes = self.sizes.as_ref().unwrap();
        let timestamps = self.timestamps.as_ref().unwrap();
        let off = offsets[index];
        let size = sizes[index];
        let ts = timestamps[index];
        if off == 0 || size == 0 {
            return Ok(None);
        }
        Ok(Some(McaEntry::new(
            self.access.try_clone()?,
            off,
            size as usize,
            index as u32,
            ts,
            self.x_pos,
            self.z_pos,
        )))
    }

    /// Get the entry for the chunk at world coordinates `(x, z)`, or `None`
    /// when that region slot is empty. Mirrors thanos `McaReader::getChunkAt`.
    pub fn get_chunk_at(&mut self, x: i32, z: i32) -> Result<Option<McaEntry>> {
        let index = x.rem_euclid(32) as usize + z.rem_euclid(32) as usize * 32;
        self.get(index)
    }
}

fn parse_coords(path: &str) -> Result<(i32, i32)> {
    let re = Regex::new(r#"r\.(-?\d+)\.(-?\d+)\.mca$"#)?;
    let caps = re
        .captures(path)
        .ok_or_else(|| anyhow!("invalid mca filename"))?;
    let x_pos: i32 = caps.get(1).unwrap().as_str().parse()?;
    let z_pos: i32 = caps.get(2).unwrap().as_str().parse()?;
    Ok((x_pos, z_pos))
}
