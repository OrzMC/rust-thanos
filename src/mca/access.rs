//! Random-access I/O abstraction.
//!
//! Mirrors OrzMCBackup's `RandomAccess.kt`: MCA code operates on a
//! `Box<dyn RandomAccess>` instead of a concrete file, so the same reader and
//! writer can target either the real filesystem (`FileAccess` → `File`) or an
//! in-memory buffer (`MemoryAccess`).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

/// Random-access read/write handle used by the MCA reader, writer, and entries.
///
/// All methods follow the same semantics as [`std::fs::File`]. Implementations
/// must be `Send` so handles can cross Rayon threads.
pub trait RandomAccess: Send {
    /// Move the read/write position to `pos`.
    fn seek(&mut self, pos: u64) -> io::Result<()>;
    /// Read exactly `buf.len()` bytes into `buf`.
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()>;
    /// Write `buf` at the current position.
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    /// Duplicate the handle sharing the same underlying data (used to give each
    /// chunk entry an independent position).
    fn try_clone(&self) -> io::Result<Box<dyn RandomAccess>>;
    /// Total length of the underlying data in bytes.
    fn len(&self) -> io::Result<u64>;
    /// True when the underlying data has zero length.
    fn is_empty(&self) -> io::Result<bool> {
        self.len().map(|n| n == 0)
    }
    /// Flush/sync to durable storage. No-op for in-memory backends; `File`
    /// overrides with `sync_all` (this is what `--no-fsync` toggles).
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A `Box<dyn RandomAccess>` is itself a `RandomAccess` (delegates through), so
/// generic call sites accept either a concrete implementation or a boxed one.
impl RandomAccess for Box<dyn RandomAccess> {
    fn seek(&mut self, pos: u64) -> io::Result<()> {
        (**self).seek(pos)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        (**self).read_exact(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        (**self).write_all(buf)
    }
    fn try_clone(&self) -> io::Result<Box<dyn RandomAccess>> {
        (**self).try_clone()
    }
    fn len(&self) -> io::Result<u64> {
        (**self).len()
    }
    fn sync(&mut self) -> io::Result<()> {
        (**self).sync()
    }
}

/// [`RandomAccess`] backed by a real file. Mirrors OrzMCBackup's `RafAccess`
/// (the newtype avoids trait-method ambiguity with `std::io::Seek`/`Write`).
///
/// The underlying [`File`] is shared through `Arc<Mutex<File>>`, so `try_clone`
/// — which the reader calls once per chunk entry — costs an `Arc` bump instead
/// of a new file descriptor. Without this, a large world opened one fd per
/// chunk and exhausted the process ulimit, silently dropping regions.
///
/// The read/write position is tracked per-clone, exactly like [`MemoryAccess`],
/// so every clone has an independent cursor over one shared file. Each
/// `read_exact`/`write_all` seeks to its own position and performs the I/O
/// under the mutex in a single step, so interleaved access stays correct.
pub struct FileAccess {
    file: Arc<Mutex<File>>,
    pos: u64,
}

impl FileAccess {
    pub fn new(file: File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
            pos: 0,
        }
    }

    /// Open read-only (for the reader).
    pub fn open(path: &str) -> io::Result<Self> {
        Ok(Self::new(File::open(path)?))
    }

    /// Open read+write, truncating (for the writer).
    pub fn open_rw(path: &str) -> io::Result<Self> {
        let f = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        Ok(Self::new(f))
    }
}

impl RandomAccess for FileAccess {
    fn seek(&mut self, pos: u64) -> io::Result<()> {
        self.pos = pos;
        Ok(())
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(self.pos))?;
        f.read_exact(buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(self.pos))?;
        f.write_all(buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }
    fn try_clone(&self) -> io::Result<Box<dyn RandomAccess>> {
        Ok(Box::new(FileAccess {
            file: self.file.clone(),
            pos: 0,
        }))
    }
    fn len(&self) -> io::Result<u64> {
        Ok(self.file.lock().unwrap().metadata()?.len())
    }
    fn sync(&mut self) -> io::Result<()> {
        self.file.lock().unwrap().sync_all()
    }
}

/// [`RandomAccess`] backed by a shared in-memory buffer.
///
/// The buffer is shared via `Arc<Mutex<Vec<u8>>>` so reader clones (chunk
/// entries) see the same data. Mirrors OrzMCBackup's `MemoryAccess`.
pub struct MemoryAccess {
    data: Arc<Mutex<Vec<u8>>>,
    pos: u64,
}

impl MemoryAccess {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data: Arc::new(Mutex::new(data)),
            pos: 0,
        }
    }

    /// Shared buffer handle, for when the writer must flush its bytes elsewhere
    /// (e.g. back into a `MemoryFS`) after finalizing the header tables.
    pub fn shared(&self) -> Arc<Mutex<Vec<u8>>> {
        self.data.clone()
    }
}

impl RandomAccess for MemoryAccess {
    fn seek(&mut self, pos: u64) -> io::Result<()> {
        self.pos = pos;
        Ok(())
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let data = self.data.lock().unwrap();
        let start = self.pos as usize;
        let end = start.saturating_add(buf.len());
        if end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "memory read past end of buffer",
            ));
        }
        buf.copy_from_slice(&data[start..end]);
        self.pos = end as u64;
        Ok(())
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut data = self.data.lock().unwrap();
        let start = self.pos as usize;
        if start + buf.len() > data.len() {
            data.resize(start + buf.len(), 0);
        }
        data[start..start + buf.len()].copy_from_slice(buf);
        self.pos = (start + buf.len()) as u64;
        Ok(())
    }
    fn try_clone(&self) -> io::Result<Box<dyn RandomAccess>> {
        Ok(Box::new(MemoryAccess {
            data: self.data.clone(),
            pos: 0,
        }))
    }
    fn len(&self) -> io::Result<u64> {
        Ok(self.data.lock().unwrap().len() as u64)
    }
}
