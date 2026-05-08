//! Rust-native filesystem contracts and first-party backends.
//!
//! This crate ports the Go `fs` and core `fskit` surfaces into a single trait
//! family. Filesystems share helper operations for read/write, create, remove,
//! copy, metadata, symlinks, and open flags so backends do not need to duplicate
//! fallback behavior.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::SystemTime;

pub use wanix_core::{
    base_name, clean_path, parent_path, valid_path, DirEntry, Error, ErrorKind, FileMode,
    FsContext, Metadata, OpenFlags, Result,
};

pub type FsRef = Arc<dyn FileSystem>;
pub type BoxFile = Box<dyn FileHandle>;

fn current_time() -> SystemTime {
    #[cfg(target_arch = "wasm32")]
    {
        SystemTime::UNIX_EPOCH
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
    }
}

pub fn fs_ref<T>(fs: T) -> FsRef
where
    T: FileSystem + 'static,
{
    Arc::new(fs)
}

pub trait FileHandle: Send {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Err(ErrorKind::PermissionDenied.into())
    }

    fn write(&mut self, _data: &[u8]) -> Result<usize> {
        Err(ErrorKind::PermissionDenied.into())
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.seek(SeekFrom::Start(offset))?;
        self.read(buf)
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.seek(SeekFrom::Start(offset))?;
        self.write(data)
    }

    fn seek(&mut self, _pos: SeekFrom) -> Result<u64> {
        Err(ErrorKind::NotSupported.into())
    }

    fn stat(&self) -> Result<Metadata>;

    fn read_dir(&mut self, _count: isize) -> Result<Vec<DirEntry>> {
        Err(Error::path(
            "readdir",
            self.stat()?.name,
            ErrorKind::NotSupported,
        ))
    }

    fn sync(&mut self) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

pub trait FileSystem: Send + Sync {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile>;

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut f = self.open(&ctx.clone().with_origin(name, "stat"), name)?;
        let stat = f.stat();
        let _ = f.close();
        stat
    }

    fn lstat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.stat(&ctx.clone().no_follow(), name)
    }

    fn read_dir(&self, ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
        let mut f = self.open(&ctx.clone().read_only().with_origin(name, "readdir"), name)?;
        let mut entries = f.read_dir(-1)?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn create(&self, _name: &str) -> Result<BoxFile> {
        Err(ErrorKind::NotSupported.into())
    }

    fn open_file(&self, name: &str, flags: OpenFlags, perm: FileMode) -> Result<BoxFile> {
        if flags.is_write() {
            let mut created = false;
            let mut file = match self.open(&FsContext::new(), name) {
                Ok(file) => file,
                Err(err)
                    if flags.contains(OpenFlags::CREATE) && err.kind() == ErrorKind::NotFound =>
                {
                    created = true;
                    self.create(name)?
                }
                Err(err) => return Err(err),
            };
            if flags.contains(OpenFlags::TRUNC) && !created {
                let _ = file.close();
                file = self.create(name)?;
            }
            if perm.bits() != 0 {
                let _ = file.close();
                match self.chmod(name, perm) {
                    Ok(()) | Err(Error::Kind(ErrorKind::NotSupported)) => {}
                    Err(err) => return Err(err),
                }
                file = self.open(&FsContext::new(), name)?;
            }
            if flags.contains(OpenFlags::APPEND) {
                let _ = file.seek(SeekFrom::End(0))?;
            }
            Ok(file)
        } else {
            self.open(&FsContext::new(), name)
        }
    }

    fn mkdir(&self, _name: &str, _perm: FileMode) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn remove(&self, _name: &str) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn rename(&self, _old: &str, _new: &str) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn chmod(&self, _name: &str, _mode: FileMode) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn chown(&self, _name: &str, _uid: u32, _gid: u32) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn chtimes(&self, _name: &str, _mtime: SystemTime) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn truncate(&self, _name: &str, _size: u64) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn symlink(&self, _old: &str, _new: &str) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn readlink(&self, _name: &str) -> Result<String> {
        Err(ErrorKind::NotSupported.into())
    }

    fn set_xattr(&self, _name: &str, _attr: &str, _data: &[u8]) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn get_xattr(&self, _name: &str, _attr: &str) -> Result<Vec<u8>> {
        Err(ErrorKind::NotSupported.into())
    }

    fn list_xattrs(&self, _name: &str) -> Result<Vec<String>> {
        Err(ErrorKind::NotSupported.into())
    }

    fn remove_xattr(&self, _name: &str, _attr: &str) -> Result<()> {
        Err(ErrorKind::NotSupported.into())
    }

    fn watch(&self, _name: &str) -> Result<BoxFile> {
        Err(ErrorKind::NotSupported.into())
    }

    fn sync_fs(&self) -> Result<()> {
        Ok(())
    }
}

pub fn open(fsys: &dyn FileSystem, name: &str) -> Result<BoxFile> {
    fsys.open(&FsContext::new().with_origin(name, "open"), name)
}

pub fn stat(fsys: &dyn FileSystem, name: &str) -> Result<Metadata> {
    fsys.stat(&FsContext::new().with_origin(name, "stat"), name)
}

pub fn lstat(fsys: &dyn FileSystem, name: &str) -> Result<Metadata> {
    fsys.lstat(&FsContext::new().with_origin(name, "stat"), name)
}

pub fn read_dir(fsys: &dyn FileSystem, name: &str) -> Result<Vec<DirEntry>> {
    fsys.read_dir(&FsContext::new().with_origin(name, "readdir"), name)
}

pub fn read_file(fsys: &dyn FileSystem, name: &str) -> Result<Vec<u8>> {
    let mut file = open(fsys, name)?;
    let mut out = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err),
        }
    }
    let _ = file.close();
    Ok(out)
}

pub fn write_file(fsys: &dyn FileSystem, name: &str, data: &[u8], perm: FileMode) -> Result<()> {
    let mut file = fsys.open_file(
        name,
        OpenFlags::WRONLY | OpenFlags::CREATE | OpenFlags::TRUNC,
        perm,
    )?;
    file.write(data)?;
    file.close()
}

pub fn append_file(fsys: &dyn FileSystem, name: &str, data: &[u8]) -> Result<()> {
    let mut file = fsys.open_file(
        name,
        OpenFlags::WRONLY | OpenFlags::CREATE | OpenFlags::APPEND,
        FileMode::from_perm(0o644),
    )?;
    file.write(data)?;
    file.close()
}

pub fn exists(fsys: &dyn FileSystem, name: &str) -> Result<bool> {
    match stat(fsys, name) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn is_dir(fsys: &dyn FileSystem, name: &str) -> Result<bool> {
    Ok(stat(fsys, name)?.is_dir())
}

pub fn is_empty(fsys: &dyn FileSystem, name: &str) -> Result<bool> {
    Ok(read_dir(fsys, name)?.is_empty())
}

pub fn mkdir_all(fsys: &dyn FileSystem, name: &str, perm: FileMode) -> Result<()> {
    if name == "." {
        return Ok(());
    }
    let mut cur = String::new();
    for part in clean_path(name).split('/') {
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(part);
        match fsys.mkdir(&cur, perm) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub fn remove_all(fsys: &dyn FileSystem, name: &str) -> Result<()> {
    if stat(fsys, name)?.is_dir() {
        for entry in read_dir(fsys, name)? {
            let child = if name == "." {
                entry.name
            } else {
                format!("{name}/{}", entry.name)
            };
            remove_all(fsys, &child)?;
        }
    }
    fsys.remove(name)
}

pub fn copy_all(fsys: &dyn FileSystem, src: &str, dst: &str) -> Result<()> {
    copy_fs(fsys, src, fsys, dst)
}

pub fn copy_fs(
    src_fs: &dyn FileSystem,
    src: &str,
    dst_fs: &dyn FileSystem,
    dst: &str,
) -> Result<()> {
    let meta = lstat(src_fs, src)?;
    if meta.mode.is_symlink() {
        return dst_fs.symlink(&src_fs.readlink(src)?, dst);
    }
    if meta.is_dir() {
        dst_fs.mkdir(dst, meta.mode)?;
        for entry in read_dir(src_fs, src)? {
            let child_src = if src == "." {
                entry.name.clone()
            } else {
                format!("{src}/{}", entry.name)
            };
            let child_dst = if dst == "." {
                entry.name
            } else {
                format!("{dst}/{}", entry.name)
            };
            copy_fs(src_fs, &child_src, dst_fs, &child_dst)?;
        }
    } else {
        let data = read_file(src_fs, src)?;
        write_file(dst_fs, dst, &data, meta.mode)?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct Node {
    inner: Arc<Mutex<NodeInner>>,
}

#[derive(Clone)]
struct NodeInner {
    meta: Metadata,
    data: Vec<u8>,
}

impl Node {
    pub fn file(name: impl Into<String>, data: impl Into<Vec<u8>>, mode: FileMode) -> Self {
        let data = data.into();
        let mut meta = Metadata::file(name, mode.perm(), data.len() as u64);
        meta.mode = mode;
        Self {
            inner: Arc::new(Mutex::new(NodeInner { meta, data })),
        }
    }

    pub fn dir(name: impl Into<String>, mode: FileMode) -> Self {
        let mut meta = Metadata::dir(name, mode.perm());
        meta.mode = mode | FileMode::DIR;
        Self {
            inner: Arc::new(Mutex::new(NodeInner {
                meta,
                data: Vec::new(),
            })),
        }
    }

    pub fn symlink(name: impl Into<String>, target: impl Into<String>) -> Self {
        let data = target.into().into_bytes();
        Self {
            inner: Arc::new(Mutex::new(NodeInner {
                meta: Metadata::symlink(name, data.len() as u64),
                data,
            })),
        }
    }

    pub fn metadata(&self) -> Metadata {
        self.inner.lock().unwrap().meta.clone()
    }

    pub fn set_name(&self, name: impl Into<String>) {
        self.inner.lock().unwrap().meta.name = name.into();
    }

    pub fn data(&self) -> Vec<u8> {
        self.inner.lock().unwrap().data.clone()
    }

    fn set_data(&self, data: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        inner.meta.size = data.len() as u64;
        inner.meta.modified = current_time();
        inner.data = data;
    }

    fn set_mode(&self, mode: FileMode) {
        self.inner.lock().unwrap().meta.mode = mode;
    }

    fn set_size(&self, size: u64) {
        self.inner.lock().unwrap().meta.size = size;
    }

    fn set_modified(&self, modified: SystemTime) {
        self.inner.lock().unwrap().meta.modified = modified;
    }
}

impl FileSystem for Node {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let inner = self.inner.lock().unwrap();
        Ok(Box::new(NodeFile {
            node: self.clone(),
            meta: inner.meta.clone(),
            data: inner.data.clone(),
            offset: 0,
            dirty: false,
            closed: false,
        }))
    }

    fn stat(&self, _ctx: &FsContext, name: &str) -> Result<Metadata> {
        if name != "." {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        Ok(self.metadata())
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        if name != "." {
            return Err(Error::path("truncate", name, ErrorKind::NotFound));
        }
        let mut data = self.data();
        data.resize(size as usize, 0);
        self.set_data(data);
        Ok(())
    }
}

struct NodeFile {
    node: Node,
    meta: Metadata,
    data: Vec<u8>,
    offset: u64,
    dirty: bool,
    closed: bool,
}

impl FileHandle for NodeFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        if self.meta.mode.is_dir() {
            return Err(Error::path("read", &self.meta.name, ErrorKind::Invalid));
        }
        let start = self.offset as usize;
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        if self.meta.mode.is_dir() {
            return Err(Error::path("write", &self.meta.name, ErrorKind::IsDir));
        }
        let start = self.offset as usize;
        let end = start.saturating_add(data.len());
        if start > self.data.len() {
            self.data.resize(start, 0);
        }
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(data);
        self.offset = end as u64;
        self.meta.size = self.data.len() as u64;
        self.meta.modified = current_time();
        self.dirty = true;
        Ok(data.len())
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        if offset as usize > self.data.len() {
            return Err(Error::path("read", &self.meta.name, ErrorKind::Invalid));
        }
        let start = offset as usize;
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.offset = offset;
        self.write(data)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let next = match pos {
            SeekFrom::Start(pos) => pos as i128,
            SeekFrom::End(off) => self.data.len() as i128 + off as i128,
            SeekFrom::Current(off) => self.offset as i128 + off as i128,
        };
        if next < 0 || next > self.data.len() as i128 {
            return Err(Error::path("seek", &self.meta.name, ErrorKind::Invalid));
        }
        self.offset = next as u64;
        Ok(self.offset)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(self.meta.clone())
    }

    fn sync(&mut self) -> Result<()> {
        if self.dirty {
            self.node.set_data(self.data.clone());
            self.dirty = false;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        self.sync()?;
        self.closed = true;
        Ok(())
    }
}

struct DirFile {
    meta: Metadata,
    entries: Vec<DirEntry>,
    offset: usize,
}

impl DirFile {
    fn new(mut meta: Metadata, entries: Vec<DirEntry>) -> Self {
        meta.mode |= FileMode::DIR;
        if meta.size == 0 {
            meta.size = 2 + entries.len() as u64;
        }
        Self {
            meta,
            entries: dedupe_sort_hide(entries),
            offset: 0,
        }
    }
}

pub fn directory_file(meta: Metadata, entries: Vec<DirEntry>) -> BoxFile {
    Box::new(DirFile::new(meta, entries))
}

impl FileHandle for DirFile {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Err(Error::path("read", &self.meta.name, ErrorKind::Invalid))
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(self.meta.clone())
    }

    fn read_dir(&mut self, count: isize) -> Result<Vec<DirEntry>> {
        if count < 0 {
            self.offset = self.entries.len();
            return Ok(self.entries.clone());
        }
        if self.offset >= self.entries.len() {
            return Ok(Vec::new());
        }
        let end = if count == 0 {
            self.entries.len()
        } else {
            (self.offset + count as usize).min(self.entries.len())
        };
        let out = self.entries[self.offset..end].to_vec();
        self.offset = end;
        Ok(out)
    }
}

fn dedupe_sort_hide(entries: Vec<DirEntry>) -> Vec<DirEntry> {
    let mut map = BTreeMap::new();
    for entry in entries {
        if !entry.name.starts_with('#') {
            map.insert(entry.name.clone(), entry);
        }
    }
    map.into_values().collect()
}

#[derive(Clone, Default)]
pub struct MapFs {
    entries: Arc<RwLock<BTreeMap<String, FsRef>>>,
}

impl MapFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, name: impl Into<String>, fs: FsRef) {
        self.entries
            .write()
            .unwrap()
            .insert(clean_path(&name.into()), fs);
    }

    pub fn file(&self, name: impl Into<String>, data: impl Into<Vec<u8>>) {
        let name = name.into();
        let node = Node::file(base_name(&name), data, FileMode::from_perm(0o644));
        self.insert(name, fs_ref(node));
    }

    pub fn dir(&self, name: impl Into<String>) {
        let name = name.into();
        self.insert(
            name.clone(),
            fs_ref(Node::dir(
                base_name(&name),
                FileMode::DIR | FileMode::from_perm(0o555),
            )),
        );
    }

    fn direct(&self, name: &str) -> Option<FsRef> {
        self.entries.read().unwrap().get(name).cloned()
    }

    fn match_prefixes(&self, name: &str) -> Vec<(String, FsRef)> {
        let entries = self.entries.read().unwrap();
        let mut out: Vec<_> = entries
            .iter()
            .filter_map(|(key, fs)| {
                if key == name || (key != "." && name.starts_with(&format!("{key}/"))) {
                    Some((key.clone(), fs.clone()))
                } else {
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        out
    }
}

impl FileSystem for MapFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if let Some(fs) = self.direct(&name) {
            return fs.open(ctx, ".");
        }
        for (key, fs) in self.match_prefixes(&name) {
            let rel = if key == "." {
                name.clone()
            } else {
                clean_path(name.trim_start_matches(&format!("{key}/")))
            };
            if rel != "." {
                return fs.open(ctx, &rel);
            }
        }
        let mut entries = Vec::new();
        let mut need = BTreeSet::new();
        let map = self.entries.read().unwrap();
        if name == "." {
            for (fname, subfs) in map.iter() {
                if let Some((head, _)) = fname.split_once('/') {
                    need.insert(head.to_string());
                } else if fname != "." {
                    if let Ok(mut meta) = subfs.stat(ctx, ".") {
                        meta.name = fname.clone();
                        entries.push(DirEntry::new(fname.clone(), meta));
                    }
                }
            }
        } else {
            let prefix = format!("{name}/");
            for (fname, subfs) in map.iter() {
                if let Some(rest) = fname.strip_prefix(&prefix) {
                    if let Some((head, _)) = rest.split_once('/') {
                        need.insert(head.to_string());
                    } else if let Ok(mut meta) = subfs.stat(ctx, ".") {
                        meta.name = rest.to_string();
                        entries.push(DirEntry::new(rest.to_string(), meta));
                    }
                }
            }
            if entries.is_empty() && need.is_empty() {
                return Err(Error::path("open", name, ErrorKind::NotFound));
            }
        }
        for name in need {
            entries.push(DirEntry::new(name.clone(), Metadata::dir(name, 0o555)));
        }
        Ok(Box::new(DirFile::new(Metadata::dir(name, 0o555), entries)))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        if name == "." {
            if let Some(root) = self.direct(".") {
                return root.stat(ctx, ".");
            }
            return Ok(Metadata::dir(".", 0o555));
        }
        let mut f = self.open(ctx, name)?;
        let stat = f.stat();
        let _ = f.close();
        stat
    }
}

#[derive(Clone)]
pub struct MemFs {
    nodes: Arc<RwLock<BTreeMap<String, Node>>>,
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFs {
    pub fn new() -> Self {
        let fs = Self {
            nodes: Arc::new(RwLock::new(BTreeMap::new())),
        };
        fs.nodes.write().unwrap().insert(
            ".".to_string(),
            Node::dir(".", FileMode::DIR | FileMode::from_perm(0o755)),
        );
        fs
    }

    pub fn from_entries(
        entries: impl IntoIterator<Item = (impl Into<String>, impl Into<Vec<u8>>)>,
    ) -> Self {
        let fs = Self::new();
        for (name, data) in entries {
            let name = clean_path(&name.into());
            fs.ensure_parent_dirs(&name);
            fs.nodes.write().unwrap().insert(
                name.clone(),
                Node::file(base_name(&name), data, FileMode::from_perm(0o644)),
            );
        }
        fs.recompute_dir_sizes();
        fs
    }

    pub fn set_node(&self, name: impl Into<String>, node: Node) {
        self.nodes
            .write()
            .unwrap()
            .insert(clean_path(&name.into()), node);
        self.recompute_dir_sizes();
    }

    fn ensure_parent_dirs(&self, name: &str) {
        let mut cur = parent_path(name);
        let mut dirs = Vec::new();
        while cur != "." {
            dirs.push(cur.clone());
            cur = parent_path(&cur);
        }
        let mut nodes = self.nodes.write().unwrap();
        for dir in dirs.into_iter().rev() {
            nodes.entry(dir.clone()).or_insert_with(|| {
                Node::dir(base_name(&dir), FileMode::DIR | FileMode::from_perm(0o755))
            });
        }
    }

    fn resolve_symlink(&self, ctx: &FsContext, name: &str, target: String) -> String {
        if target.starts_with('/') {
            clean_path(target.trim_start_matches('/'))
        } else if ctx.filepath.as_deref() == Some(name) || ctx.filepath.is_none() {
            clean_path(&format!("{}/{}", parent_path(name), target))
        } else {
            clean_path(&target)
        }
    }

    fn node(&self, name: &str) -> Option<Node> {
        self.nodes.read().unwrap().get(name).cloned()
    }

    fn children_for(&self, name: &str) -> (Vec<DirEntry>, BTreeSet<String>) {
        let mut entries = Vec::new();
        let mut need = BTreeSet::new();
        let nodes = self.nodes.read().unwrap();
        if name == "." {
            for (fname, node) in nodes.iter() {
                if fname == "." {
                    continue;
                }
                if let Some((head, _)) = fname.split_once('/') {
                    need.insert(head.to_string());
                } else {
                    let mut meta = node.metadata();
                    meta.name = fname.clone();
                    entries.push(DirEntry::new(fname.clone(), meta));
                }
            }
        } else {
            let prefix = format!("{name}/");
            for (fname, node) in nodes.iter() {
                if let Some(rest) = fname.strip_prefix(&prefix) {
                    if let Some((head, _)) = rest.split_once('/') {
                        need.insert(head.to_string());
                    } else {
                        let mut meta = node.metadata();
                        meta.name = rest.to_string();
                        entries.push(DirEntry::new(rest.to_string(), meta));
                    }
                }
            }
        }
        for entry in &entries {
            need.remove(&entry.name);
        }
        (entries, need)
    }

    fn recompute_dir_sizes(&self) {
        let dirs: Vec<String> = self
            .nodes
            .read()
            .unwrap()
            .iter()
            .filter_map(|(name, node)| node.metadata().is_dir().then_some(name.clone()))
            .collect();
        for dir in dirs {
            let (entries, need) = self.children_for(&dir);
            if let Some(node) = self.node(&dir) {
                node.set_size(2 + entries.len() as u64 + need.len() as u64);
            }
        }
    }
}

impl FileSystem for MemFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let node = self.node(&name);
        if let Some(node) = node.clone() {
            let meta = node.metadata();
            if ctx.follow_symlinks && meta.mode.is_symlink() {
                let target = String::from_utf8_lossy(&node.data()).into_owned();
                let resolved = self.resolve_symlink(ctx, &name, target);
                return self.open(ctx, &resolved);
            }
            if !meta.is_dir() {
                node.set_name(base_name(&name));
                return node.open(ctx, ".");
            }
        }
        let (mut entries, need) = self.children_for(&name);
        if node.is_none() && entries.is_empty() && need.is_empty() && name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        for dir in need {
            entries.push(DirEntry::new(dir.clone(), Metadata::dir(dir, 0o755)));
        }
        let meta = node
            .map(|n| n.metadata())
            .unwrap_or_else(|| Metadata::dir(base_name(&name), 0o755));
        Ok(Box::new(DirFile::new(meta, entries)))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        if !valid_path(name) {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("stat", &name, ErrorKind::NotFound))?;
        let meta = node.metadata();
        if ctx.follow_symlinks && meta.mode.is_symlink() {
            let target = String::from_utf8_lossy(&node.data()).into_owned();
            let resolved = self.resolve_symlink(ctx, &name, target);
            return self.stat(ctx, &resolved);
        }
        Ok(meta)
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        let name = clean_path(name);
        if !valid_path(&name) {
            return Err(Error::path("create", name, ErrorKind::NotFound));
        }
        let parent = parent_path(&name);
        if parent != "." && !exists(self, &parent)? {
            return Err(Error::path("create", name, ErrorKind::NotFound));
        }
        let node = Node::file(base_name(&name), Vec::new(), FileMode::from_perm(0o644));
        self.nodes
            .write()
            .unwrap()
            .insert(name.clone(), node.clone());
        self.recompute_dir_sizes();
        node.open(&FsContext::new(), ".")
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        let name = clean_path(name);
        if !valid_path(&name) {
            return Err(Error::path("mkdir", name, ErrorKind::NotFound));
        }
        if self.node(&name).is_some() {
            return Err(Error::path("mkdir", name, ErrorKind::AlreadyExists));
        }
        let parent = parent_path(&name);
        if parent != "." && !exists(self, &parent)? {
            return Err(Error::path("mkdir", name, ErrorKind::NotFound));
        }
        self.nodes.write().unwrap().insert(
            name.clone(),
            Node::dir(base_name(&name), FileMode::DIR | perm),
        );
        self.recompute_dir_sizes();
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        let name = clean_path(name);
        if name == "." {
            return Err(Error::path("remove", name, ErrorKind::Invalid));
        }
        if self.node(&name).is_none() {
            return Err(Error::path("remove", name, ErrorKind::NotFound));
        }
        if is_dir(self, &name)? && !is_empty(self, &name)? {
            return Err(Error::path("remove", name, ErrorKind::NotEmpty));
        }
        self.nodes.write().unwrap().remove(&name);
        self.recompute_dir_sizes();
        Ok(())
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        let old = clean_path(old);
        let new = clean_path(new);
        if !valid_path(&old) || !valid_path(&new) {
            return Err(Error::path("rename", old, ErrorKind::NotFound));
        }
        if old == new {
            return Ok(());
        }
        let parent = parent_path(&new);
        if parent != "." && !is_dir(self, &parent).unwrap_or(false) {
            return Err(Error::path("rename", new, ErrorKind::NotFound));
        }
        let mut nodes = self.nodes.write().unwrap();
        let old_node = nodes
            .get(&old)
            .cloned()
            .ok_or_else(|| Error::path("rename", &old, ErrorKind::NotFound))?;
        if let Some(existing) = nodes.get(&new) {
            if existing.metadata().is_dir()
                && nodes
                    .keys()
                    .any(|path| path.starts_with(&format!("{new}/")))
            {
                return Err(Error::path("rename", &new, ErrorKind::AlreadyExists));
            }
        }
        if old_node.metadata().is_dir() {
            let prefix = format!("{old}/");
            let to_move: Vec<_> = nodes
                .iter()
                .filter(|(path, _)| path == &&old || path.starts_with(&prefix))
                .map(|(path, node)| {
                    let suffix = path.strip_prefix(&old).unwrap_or("");
                    (path.clone(), format!("{new}{suffix}"), node.clone())
                })
                .collect();
            for (old_path, _, _) in &to_move {
                nodes.remove(old_path);
            }
            for (_, new_path, node) in to_move {
                nodes.insert(new_path, node);
            }
        } else {
            nodes.remove(&new);
            nodes.remove(&old);
            nodes.insert(new, old_node);
        }
        drop(nodes);
        self.recompute_dir_sizes();
        Ok(())
    }

    fn chmod(&self, name: &str, mode: FileMode) -> Result<()> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("chmod", &name, ErrorKind::NotFound))?;
        let current = node.metadata().mode;
        node.set_mode(current.type_bits() | FileMode::from_perm(mode.perm()));
        Ok(())
    }

    fn chown(&self, name: &str, uid: u32, gid: u32) -> Result<()> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("chown", &name, ErrorKind::NotFound))?;
        let mut inner = node.inner.lock().unwrap();
        inner.meta.uid = uid;
        inner.meta.gid = gid;
        Ok(())
    }

    fn chtimes(&self, name: &str, mtime: SystemTime) -> Result<()> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("chtimes", &name, ErrorKind::NotFound))?;
        node.set_modified(mtime);
        Ok(())
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("truncate", &name, ErrorKind::NotFound))?;
        let mut data = node.data();
        data.resize(size as usize, 0);
        node.set_data(data);
        Ok(())
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        let new = clean_path(new);
        if !valid_path(&new) {
            return Err(Error::path("symlink", new, ErrorKind::Invalid));
        }
        let parent = parent_path(&new);
        if parent != "." && !exists(self, &parent)? {
            return Err(Error::path("symlink", new, ErrorKind::NotFound));
        }
        self.nodes
            .write()
            .unwrap()
            .insert(new.clone(), Node::symlink(base_name(&new), old));
        self.recompute_dir_sizes();
        Ok(())
    }

    fn readlink(&self, name: &str) -> Result<String> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("readlink", &name, ErrorKind::NotFound))?;
        if !node.metadata().mode.is_symlink() {
            return Err(Error::path("readlink", name, ErrorKind::Invalid));
        }
        Ok(String::from_utf8_lossy(&node.data()).into_owned())
    }
}

#[derive(Clone)]
pub struct LocalFs {
    root: Arc<PathBuf>,
}

impl LocalFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Arc::new(root.into()),
        }
    }

    fn full_path(&self, name: &str) -> Result<PathBuf> {
        if !valid_path(name) {
            return Err(Error::path("localfs", name, ErrorKind::NotFound));
        }
        let mut full = (*self.root).clone();
        if name != "." {
            full.push(clean_path(name));
        }
        Ok(full)
    }

    fn meta_for(path: &Path, name: &str) -> Result<Metadata> {
        let meta = std::fs::symlink_metadata(path)?;
        let mode = if meta.file_type().is_dir() {
            FileMode::DIR
        } else if meta.file_type().is_symlink() {
            FileMode::SYMLINK
        } else {
            FileMode::empty()
        } | FileMode::from_perm(readonly_perm(meta.permissions().readonly()));
        Ok(Metadata {
            name: base_name(name).to_string(),
            mode,
            size: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            uid: 0,
            gid: 0,
        })
    }
}

fn readonly_perm(readonly: bool) -> u32 {
    if readonly {
        0o444
    } else {
        0o666
    }
}

impl FileSystem for LocalFs {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        let path = self.full_path(name)?;
        let meta = Self::meta_for(&path, name)?;
        if meta.is_dir() {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let fname = entry.file_name().to_string_lossy().into_owned();
                let child_meta = Self::meta_for(&entry.path(), &fname)?;
                entries.push(DirEntry::new(fname, child_meta));
            }
            Ok(Box::new(DirFile::new(meta, entries)))
        } else {
            Ok(Box::new(LocalFile {
                path: self.full_path(name)?,
                file: OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(self.full_path(name)?)?,
                meta,
            }))
        }
    }

    fn stat(&self, _ctx: &FsContext, name: &str) -> Result<Metadata> {
        Self::meta_for(&self.full_path(name)?, name)
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        let path = self.full_path(name)?;
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                return Err(Error::path("create", name, ErrorKind::NotFound));
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)?;
        let meta = Self::meta_for(&path, name)?;
        Ok(Box::new(LocalFile { path, file, meta }))
    }

    fn mkdir(&self, name: &str, _perm: FileMode) -> Result<()> {
        std::fs::create_dir(self.full_path(name)?)?;
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        let path = self.full_path(name)?;
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            std::fs::remove_dir(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        std::fs::rename(self.full_path(old)?, self.full_path(new)?)?;
        Ok(())
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        File::options()
            .write(true)
            .open(self.full_path(name)?)?
            .set_len(size)?;
        Ok(())
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(old, self.full_path(new)?)?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (old, new);
            Err(ErrorKind::NotSupported.into())
        }
    }

    fn readlink(&self, name: &str) -> Result<String> {
        Ok(std::fs::read_link(self.full_path(name)?)?
            .to_string_lossy()
            .into_owned())
    }
}

struct LocalFile {
    path: PathBuf,
    file: File,
    meta: Metadata,
}

impl FileHandle for LocalFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.file.read(buf)?)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        Ok(self.file.write(data)?)
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        Ok(self.file.read(buf)?)
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        Ok(self.file.write(data)?)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        Ok(self.file.seek(pos)?)
    }

    fn stat(&self) -> Result<Metadata> {
        LocalFs::meta_for(&self.path, &self.meta.name)
    }

    fn sync(&mut self) -> Result<()> {
        Ok(self.file.sync_all()?)
    }
}

#[derive(Clone, Default)]
pub struct UnionFs {
    layers: Arc<RwLock<Vec<FsRef>>>,
}

impl UnionFs {
    pub fn new(layers: Vec<FsRef>) -> Self {
        Self {
            layers: Arc::new(RwLock::new(layers)),
        }
    }

    pub fn push_front(&self, fs: FsRef) {
        self.layers.write().unwrap().insert(0, fs);
    }
}

impl FileSystem for UnionFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        let mut entries = Vec::new();
        let layers = self.layers.read().unwrap().clone();
        for layer in &layers {
            match layer.stat(ctx, name) {
                Ok(meta) if meta.is_dir() => {
                    entries.extend(layer.read_dir(ctx, name).unwrap_or_default());
                }
                Ok(_) => return layer.open(ctx, name),
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        if !entries.is_empty() || name == "." {
            return Ok(Box::new(DirFile::new(
                Metadata::dir(base_name(name), 0o555),
                entries,
            )));
        }
        Err(Error::path("open", name, ErrorKind::NotFound))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        for layer in self.layers.read().unwrap().iter() {
            match layer.stat(ctx, name) {
                Ok(meta) => return Ok(meta),
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Err(Error::path("stat", name, ErrorKind::NotFound))
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        for layer in self.layers.read().unwrap().iter() {
            match layer.create(name) {
                Ok(file) => return Ok(file),
                Err(err) if err.kind() == ErrorKind::NotSupported => {}
                Err(err) => return Err(err),
            }
        }
        Err(ErrorKind::NotSupported.into())
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        for layer in self.layers.read().unwrap().iter() {
            match layer.mkdir(name, perm) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == ErrorKind::NotSupported => {}
                Err(err) => return Err(err),
            }
        }
        Err(ErrorKind::NotSupported.into())
    }
}

#[derive(Clone)]
pub struct FieldFile {
    name: String,
    value: Arc<Mutex<String>>,
    writable: bool,
}

impl FieldFile {
    pub fn readonly(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Arc::new(Mutex::new(value.into())),
            writable: false,
        }
    }

    pub fn writable(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Arc::new(Mutex::new(value.into())),
            writable: true,
        }
    }

    pub fn get(&self) -> String {
        self.value.lock().unwrap().clone()
    }

    pub fn set(&self, value: impl Into<String>) -> Result<()> {
        if !self.writable {
            return Err(ErrorKind::PermissionDenied.into());
        }
        *self.value.lock().unwrap() = value.into();
        Ok(())
    }
}

impl FileSystem for FieldFile {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        Ok(Box::new(FieldHandle {
            field: self.clone(),
            data: format!("{}\n", self.get()).into_bytes(),
            offset: 0,
            dirty: Vec::new(),
        }))
    }

    fn stat(&self, _ctx: &FsContext, _name: &str) -> Result<Metadata> {
        Ok(Metadata::file(
            self.name.clone(),
            if self.writable { 0o666 } else { 0o444 },
            self.get().len() as u64 + 1,
        ))
    }
}

struct FieldHandle {
    field: FieldFile,
    data: Vec<u8>,
    offset: u64,
    dirty: Vec<u8>,
}

impl FileHandle for FieldHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let start = self.offset as usize;
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if !self.field.writable {
            return Err(ErrorKind::PermissionDenied.into());
        }
        self.dirty.extend_from_slice(data);
        Ok(data.len())
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let next = match pos {
            SeekFrom::Start(pos) => pos as i128,
            SeekFrom::End(off) => self.data.len() as i128 + off as i128,
            SeekFrom::Current(off) => self.offset as i128 + off as i128,
        };
        if next < 0 || next > self.data.len() as i128 {
            return Err(ErrorKind::Invalid.into());
        }
        self.offset = next as u64;
        Ok(self.offset)
    }

    fn stat(&self) -> Result<Metadata> {
        self.field.stat(&FsContext::new(), ".")
    }

    fn close(&mut self) -> Result<()> {
        if !self.dirty.is_empty() {
            let value = String::from_utf8_lossy(&self.dirty).trim().to_string();
            self.field.set(value)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ControlFile {
    name: String,
    callback: ControlCallback,
}

type ControlCallback = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

impl ControlFile {
    pub fn new(
        name: impl Into<String>,
        callback: impl Fn(&str) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            callback: Arc::new(callback),
        }
    }
}

impl FileSystem for ControlFile {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        Ok(Box::new(ControlHandle {
            ctl: self.clone(),
            buf: Vec::new(),
        }))
    }

    fn stat(&self, _ctx: &FsContext, _name: &str) -> Result<Metadata> {
        Ok(Metadata::file(self.name.clone(), 0o222, 0))
    }
}

struct ControlHandle {
    ctl: ControlFile,
    buf: Vec<u8>,
}

impl FileHandle for ControlHandle {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        self.ctl.stat(&FsContext::new(), ".")
    }

    fn close(&mut self) -> Result<()> {
        let cmd = String::from_utf8_lossy(&self.buf).trim().to_string();
        if !cmd.is_empty() {
            (self.ctl.callback)(&cmd)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct PipePort {
    reader: Arc<PipeBuffer>,
    writer: Arc<PipeBuffer>,
}

pub fn pipe_pair(blocking: bool) -> (PipePort, PipePort) {
    let a = Arc::new(PipeBuffer::new(blocking));
    let b = Arc::new(PipeBuffer::new(blocking));
    (
        PipePort {
            reader: a.clone(),
            writer: b.clone(),
        },
        PipePort {
            reader: b,
            writer: a,
        },
    )
}

struct PipeBuffer {
    state: Mutex<PipeState>,
    ready: Condvar,
    blocking: bool,
}

struct PipeState {
    data: VecDeque<u8>,
    closed: bool,
}

impl PipeBuffer {
    fn new(blocking: bool) -> Self {
        Self {
            state: Mutex::new(PipeState {
                data: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
            blocking,
        }
    }

    fn len(&self) -> usize {
        self.state.lock().unwrap().data.len()
    }
}

impl FileHandle for PipePort {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut state = self.reader.state.lock().unwrap();
        while self.reader.blocking && state.data.is_empty() && !state.closed {
            state = self.reader.ready.wait(state).unwrap();
        }
        if state.data.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(state.data.len());
        for out in &mut buf[..n] {
            *out = state.data.pop_front().unwrap();
        }
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let mut state = self.writer.state.lock().unwrap();
        if state.closed {
            return Err(ErrorKind::Closed.into());
        }
        state.data.extend(data);
        self.writer.ready.notify_all();
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file("data", 0o666, self.reader.len() as u64))
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct PipeFs {
    a: PipePort,
    b: PipePort,
}

impl PipeFs {
    pub fn new(blocking: bool) -> Self {
        let (a, b) = pipe_pair(blocking);
        Self { a, b }
    }
}

impl FileSystem for PipeFs {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        match name {
            "." => Ok(Box::new(DirFile::new(
                Metadata::dir(".", 0o555),
                vec![
                    DirEntry::new("data", Metadata::file("data", 0o666, 0)),
                    DirEntry::new("data1", Metadata::file("data1", 0o666, 0)),
                ],
            ))),
            "data" => Ok(Box::new(self.a.clone())),
            "data1" => Ok(Box::new(self.b.clone())),
            _ => Err(Error::path("open", name, ErrorKind::NotFound)),
        }
    }
}

#[derive(Clone, Default)]
pub struct SignalFs {
    subscribers: Arc<Mutex<Vec<Arc<PipeBuffer>>>>,
}

impl FileSystem for SignalFs {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name == "." {
            return Ok(Box::new(DirFile::new(
                Metadata::dir(".", 0o555),
                vec![DirEntry::new("data", Metadata::file("data", 0o666, 0))],
            )));
        }
        if name != "data" {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let buffer = Arc::new(PipeBuffer::new(true));
        self.subscribers.lock().unwrap().push(buffer.clone());
        Ok(Box::new(SignalHandle {
            owner: self.clone(),
            reader: buffer,
        }))
    }
}

struct SignalHandle {
    owner: SignalFs,
    reader: Arc<PipeBuffer>,
}

impl FileHandle for SignalHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut port = PipePort {
            reader: self.reader.clone(),
            writer: self.reader.clone(),
        };
        port.read(buf)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let subscribers = self.owner.subscribers.lock().unwrap().clone();
        for sub in subscribers {
            let mut state = sub.state.lock().unwrap();
            if !state.closed {
                state.data.extend(data);
                sub.ready.notify_all();
            }
        }
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file("data", 0o666, self.reader.len() as u64))
    }
}

pub type CacheFs = MemFs;
pub type TarFs = MemFs;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memfs_create_read_write_and_stat() {
        let fs = MemFs::from_entries([("hello", b"hello, world\n".to_vec())]);
        assert_eq!(read_file(&fs, "hello").unwrap(), b"hello, world\n");
        write_file(&fs, "fortune", b"rust\n", FileMode::from_perm(0o644)).unwrap();
        assert_eq!(read_file(&fs, "fortune").unwrap(), b"rust\n");
        assert_eq!(stat(&fs, "fortune").unwrap().size, 5);
    }

    #[test]
    fn memfs_synthesizes_and_renames_directories() {
        let fs = MemFs::from_entries([
            ("dir/a.txt", b"a\n".to_vec()),
            ("dir/sub/c.txt", b"c\n".to_vec()),
        ]);
        let names: Vec<_> = read_dir(&fs, "dir")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["a.txt", "sub"]);
        fs.rename("dir", "newdir").unwrap();
        assert!(stat(&fs, "dir/a.txt").is_err());
        assert_eq!(read_file(&fs, "newdir/sub/c.txt").unwrap(), b"c\n");
    }

    #[test]
    fn memfs_symlink_lstat_and_stat() {
        let fs = MemFs::from_entries([("target", b"value".to_vec())]);
        fs.symlink("target", "link").unwrap();
        assert!(lstat(&fs, "link").unwrap().mode.is_symlink());
        assert_eq!(read_file(&fs, "link").unwrap(), b"value");
        assert_eq!(fs.readlink("link").unwrap(), "target");
    }

    #[test]
    fn mapfs_exposes_mounts_and_synthetic_parents() {
        let map = MapFs::new();
        map.insert(
            "sub/file",
            fs_ref(Node::file(
                "file",
                b"x".to_vec(),
                FileMode::from_perm(0o644),
            )),
        );
        let names: Vec<_> = read_dir(&map, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["sub"]);
        assert_eq!(read_file(&map, "sub/file").unwrap(), b"x");
    }

    #[test]
    fn pipe_ports_are_bidirectional() {
        let (mut a, mut b) = pipe_pair(false);
        a.write(b"ping").unwrap();
        let mut buf = [0_u8; 8];
        let n = b.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        b.write(b"pong").unwrap();
        let n = a.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"pong");
    }
}
