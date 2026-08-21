//! Filesystem abstraction.
//!
//! Mirrors OrzMCBackup's `FileSystem.kt`: the optimizer operates on a
//! `Arc<dyn FileSystem>` so the same pipeline runs against the real filesystem
//! (`RealFileSystem`) or an in-memory tree (`MemoryFS`) for tests without disk
//! I/O.

use std::any::Any;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Abstract filesystem used by the optimizer pipeline.
pub trait FileSystem: Send + Sync {
    /// True if `path` exists and is a directory.
    fn is_directory(&self, path: &Path) -> bool;
    /// True if `path` exists and is a regular file.
    fn is_regular_file(&self, path: &Path) -> bool;
    /// True if `path` exists (file or directory).
    fn exists(&self, path: &Path) -> bool;
    /// Create a fresh temporary directory and return its path.
    fn create_temp_directory(&self, prefix: &str) -> io::Result<PathBuf>;
    /// Direct children of `path`.
    fn list(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    /// Recursively walk all descendants of `path`. When `follow_links` is false,
    /// symlink entries are listed but not descended into; when true, symlinked
    /// directories are followed with loop protection.
    fn walk(&self, path: &Path, follow_links: bool) -> io::Result<Vec<PathBuf>>;
    /// Create a directory and any missing parents.
    fn create_directories(&self, path: &Path) -> io::Result<()>;
    /// Delete `path` if it exists (single entry; for trees use `delete_tree`).
    fn delete_if_exists(&self, path: &Path) -> io::Result<()>;
    /// Copy `src` to `dst`, replacing an existing `dst` when `replace_existing`.
    fn copy(&self, src: &Path, dst: &Path, replace_existing: bool) -> io::Result<()>;
    /// Write `bytes` to `path`, overwriting if it exists.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Read all bytes from `path`, or `None` if absent/unreadable.
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>>;
    /// File size in bytes, or 0 if absent.
    fn size(&self, path: &Path) -> u64;
    /// Recursively delete `root` and all its contents.
    fn delete_tree(&self, root: &Path) -> io::Result<()>;
    /// Resolve to a real filesystem path (MemoryFS materializes to a temp dir).
    fn to_real_path(&self, path: &Path) -> PathBuf;
    /// For downcasting a `&dyn FileSystem` back to a concrete type.
    fn as_any(&self) -> &dyn Any;
    /// For downcasting an `Arc<dyn FileSystem>` to a concrete `Arc<T>`.
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

/// Real filesystem backed by `std::fs`. Used in production.
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }
    fn is_regular_file(&self, path: &Path) -> bool {
        path.is_file()
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn create_temp_directory(&self, prefix: &str) -> io::Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
    fn list(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for ent in std::fs::read_dir(path)? {
            out.push(ent?.path());
        }
        Ok(out)
    }
    fn walk(&self, path: &Path, follow_links: bool) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let mut visited: HashMap<PathBuf, ()> = HashMap::new();
        let mut stack = vec![path.to_path_buf()];
        while let Some(p) = stack.pop() {
            out.push(p.clone());
            let is_symlink = std::fs::symlink_metadata(&p)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if is_symlink && !follow_links {
                continue;
            }
            let real = p.canonicalize().unwrap_or_else(|_| p.clone());
            if follow_links && visited.insert(real, ()).is_some() {
                continue; // loop guard
            }
            if !p.is_dir() {
                continue;
            }
            match std::fs::read_dir(&p) {
                Ok(rd) => {
                    let mut children: Vec<PathBuf> =
                        rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
                    children.sort();
                    stack.extend(children.into_iter().rev());
                }
                Err(_) => continue,
            }
        }
        Ok(out)
    }
    fn create_directories(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }
    fn delete_if_exists(&self, path: &Path) -> io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => match std::fs::remove_dir(path) {
                Ok(()) => Ok(()),
                Err(e2) if e2.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(e),
            },
        }
    }
    fn copy(&self, src: &Path, dst: &Path, replace_existing: bool) -> io::Result<()> {
        if src.is_dir() {
            return self.create_directories(dst);
        }
        if replace_existing {
            std::fs::copy(src, dst)?;
            Ok(())
        } else {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            let mut dst_file = opts.open(dst)?;
            use std::io::Write;
            dst_file.write_all(&std::fs::read(src)?)?;
            Ok(())
        }
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, bytes)
    }
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn size(&self, path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }
    fn delete_tree(&self, root: &Path) -> io::Result<()> {
        if root.is_dir() {
            std::fs::remove_dir_all(root)
        } else {
            std::fs::remove_file(root)
        }
    }
    fn to_real_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// In-memory filesystem. All operations are thread-safe. Used for testing the
/// optimizer pipeline without disk I/O.
pub struct MemoryFS {
    nodes: Mutex<HashMap<PathBuf, NodeType>>,
    contents: Mutex<HashMap<PathBuf, Vec<u8>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeType {
    Directory,
    File,
}

impl Default for MemoryFS {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryFS {
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(PathBuf::new(), NodeType::Directory);
        Self {
            nodes: Mutex::new(nodes),
            contents: Mutex::new(HashMap::new()),
        }
    }

    fn create_dir_node(&self, path: &Path) {
        let mut nodes = self.nodes.lock().unwrap();
        let mut cur = PathBuf::new();
        for comp in path.components() {
            cur.push(comp.as_os_str());
            nodes.entry(cur.clone()).or_insert(NodeType::Directory);
        }
    }
}

impl FileSystem for MemoryFS {
    fn is_directory(&self, path: &Path) -> bool {
        self.nodes.lock().unwrap().get(path) == Some(&NodeType::Directory)
    }
    fn is_regular_file(&self, path: &Path) -> bool {
        self.nodes.lock().unwrap().get(path) == Some(&NodeType::File)
    }
    fn exists(&self, path: &Path) -> bool {
        self.nodes.lock().unwrap().contains_key(path)
    }
    fn create_temp_directory(&self, prefix: &str) -> io::Result<PathBuf> {
        let p = PathBuf::from(format!("/mem-{}-{}", prefix, uuid::Uuid::new_v4()));
        self.create_dir_node(&p);
        Ok(p)
    }
    fn list(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let nodes = self.nodes.lock().unwrap();
        let mut out: Vec<PathBuf> = nodes
            .keys()
            .filter(|k| k.parent().map(|p| p.as_os_str()) == Some(path.as_os_str()))
            .cloned()
            .collect();
        out.sort();
        Ok(out)
    }
    fn walk(&self, path: &Path, _follow_links: bool) -> io::Result<Vec<PathBuf>> {
        let nodes = self.nodes.lock().unwrap();
        let mut out: Vec<PathBuf> = nodes
            .keys()
            .filter(|k| k.starts_with(path))
            .cloned()
            .collect();
        out.sort();
        Ok(out)
    }
    fn create_directories(&self, path: &Path) -> io::Result<()> {
        self.create_dir_node(path);
        Ok(())
    }
    fn delete_if_exists(&self, path: &Path) -> io::Result<()> {
        self.nodes.lock().unwrap().remove(path);
        self.contents.lock().unwrap().remove(path);
        Ok(())
    }
    fn copy(&self, src: &Path, dst: &Path, replace_existing: bool) -> io::Result<()> {
        let nodes = self.nodes.lock().unwrap();
        let ty = nodes
            .get(src)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source not found"))?;
        if !replace_existing && nodes.contains_key(dst) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "dest exists"));
        }
        let ty = *ty;
        drop(nodes);
        match ty {
            NodeType::Directory => {
                self.create_dir_node(dst);
                Ok(())
            }
            NodeType::File => {
                let data = {
                    let c = self.contents.lock().unwrap();
                    c.get(src).cloned().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "source data not found")
                    })?
                };
                self.nodes
                    .lock()
                    .unwrap()
                    .insert(dst.to_path_buf(), NodeType::File);
                self.contents
                    .lock()
                    .unwrap()
                    .insert(dst.to_path_buf(), data);
                Ok(())
            }
        }
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.create_dir_node(path);
        self.nodes
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), NodeType::File);
        self.contents
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), bytes.to_vec());
        Ok(())
    }
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        Ok(self.contents.lock().unwrap().get(path).cloned())
    }
    fn size(&self, path: &Path) -> u64 {
        self.contents
            .lock()
            .unwrap()
            .get(path)
            .map(|b| b.len() as u64)
            .unwrap_or(0)
    }
    fn delete_tree(&self, root: &Path) -> io::Result<()> {
        let targets: Vec<PathBuf> = {
            let nodes = self.nodes.lock().unwrap();
            nodes
                .keys()
                .filter(|k| k.starts_with(root))
                .cloned()
                .collect()
        };
        let mut nodes = self.nodes.lock().unwrap();
        let mut contents = self.contents.lock().unwrap();
        for t in targets {
            nodes.remove(&t);
            contents.remove(&t);
        }
        Ok(())
    }
    fn to_real_path(&self, path: &Path) -> PathBuf {
        // Best-effort materialization: copy this subtree into a temp dir.
        let staging = std::env::temp_dir().join(format!("memfs-{}", uuid::Uuid::new_v4()));
        let real = staging.join(path.strip_prefix("/").unwrap_or(path));
        let entries: Vec<(PathBuf, NodeType, Option<Vec<u8>>)> = {
            let nodes = self.nodes.lock().unwrap();
            let contents = self.contents.lock().unwrap();
            nodes
                .keys()
                .filter(|k| k.starts_with(path) || k.as_path() == path)
                .map(|k| (k.clone(), nodes[k], contents.get(k).cloned()))
                .collect()
        };
        for (k, ty, data) in entries {
            let r = staging.join(k.strip_prefix("/").unwrap_or(&k));
            if ty == NodeType::Directory {
                std::fs::create_dir_all(&r).ok();
            } else if let Some(d) = data {
                std::fs::create_dir_all(r.parent().unwrap_or(&staging)).ok();
                std::fs::write(&r, d).ok();
            }
        }
        real
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}
