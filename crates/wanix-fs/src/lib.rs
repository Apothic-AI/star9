//! Rust-native filesystem contracts and first-party backends.
//!
//! This crate provides Wanix filesystem and core fskit-style surfaces as a trait
//! family. Filesystems share helper operations for read/write, create, remove,
//! copy, metadata, symlinks, and open flags so backends do not need to duplicate
//! fallback behavior.

#[cfg(all(feature = "native-http", not(target_arch = "wasm32")))]
mod http_native;
mod http_server;
mod metacache;
mod s3_object_store;
mod s3_signer;
mod syncfs;

#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use http::Method;

#[cfg(all(feature = "native-http", not(target_arch = "wasm32")))]
pub use http_native::NativeHttpTransport;
pub use http_server::HttpFsHandler;
pub use metacache::{CacheFs, MetaCacheFs};
pub use s3_object_store::{HttpRequestSigner, S3ObjectStore};
pub use s3_signer::AwsSigV4Signer;
pub use syncfs::{
    apply_sync_patch, DebouncedSyncScheduler, DirtyChange, DirtyEntry, RemoteSyncBackend,
    RemoteSyncRef, SyncFs, SyncScheduleSnapshot,
};
pub use wanix_core::{
    base_name, clean_path, parent_path, valid_path, DirEntry, Error, ErrorKind, FileMode,
    FsContext, Metadata, OpenFlags, Result,
};

pub type FsRef = Arc<dyn FileSystem>;
pub type BoxFile = Box<dyn FileHandle>;

fn current_time() -> SystemTime {
    #[cfg(test)]
    if let Some(time) = TEST_CURRENT_TIME.with(|slot| *slot.borrow()) {
        return time;
    }

    #[cfg(target_arch = "wasm32")]
    {
        SystemTime::UNIX_EPOCH
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
    }
}

#[cfg(test)]
thread_local! {
    static TEST_CURRENT_TIME: RefCell<Option<SystemTime>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn with_test_current_time<T>(time: SystemTime, f: impl FnOnce() -> T) -> T {
    struct ResetCurrentTime {
        previous: Option<SystemTime>,
    }

    impl Drop for ResetCurrentTime {
        fn drop(&mut self) {
            TEST_CURRENT_TIME.with(|slot| {
                slot.replace(self.previous.take());
            });
        }
    }

    TEST_CURRENT_TIME.with(|slot| {
        let previous = slot.replace(Some(time));
        let _reset = ResetCurrentTime { previous };
        f()
    })
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

    fn link(&self, _old: &str, _new: &str) -> Result<()> {
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

pub fn set_xattr(fsys: &dyn FileSystem, name: &str, attr: &str, data: &[u8]) -> Result<()> {
    fsys.set_xattr(name, attr, data)
}

pub fn get_xattr(fsys: &dyn FileSystem, name: &str, attr: &str) -> Result<Vec<u8>> {
    fsys.get_xattr(name, attr)
}

pub fn list_xattrs(fsys: &dyn FileSystem, name: &str) -> Result<Vec<String>> {
    fsys.list_xattrs(name)
}

pub fn remove_xattr(fsys: &dyn FileSystem, name: &str, attr: &str) -> Result<()> {
    fsys.remove_xattr(name, attr)
}

#[derive(Clone)]
pub struct Node {
    inner: Arc<Mutex<NodeInner>>,
}

#[derive(Clone)]
struct NodeInner {
    meta: Metadata,
    data: Vec<u8>,
    xattrs: BTreeMap<String, Vec<u8>>,
}

impl Node {
    pub fn file(name: impl Into<String>, data: impl Into<Vec<u8>>, mode: FileMode) -> Self {
        let data = data.into();
        let mut meta = Metadata::file(name, mode.perm(), data.len() as u64);
        meta.mode = mode;
        Self {
            inner: Arc::new(Mutex::new(NodeInner {
                meta,
                data,
                xattrs: BTreeMap::new(),
            })),
        }
    }

    pub fn dir(name: impl Into<String>, mode: FileMode) -> Self {
        let mut meta = Metadata::dir(name, mode.perm());
        meta.mode = mode | FileMode::DIR;
        Self {
            inner: Arc::new(Mutex::new(NodeInner {
                meta,
                data: Vec::new(),
                xattrs: BTreeMap::new(),
            })),
        }
    }

    pub fn symlink(name: impl Into<String>, target: impl Into<String>) -> Self {
        let data = target.into().into_bytes();
        Self {
            inner: Arc::new(Mutex::new(NodeInner {
                meta: Metadata::symlink(name, data.len() as u64),
                data,
                xattrs: BTreeMap::new(),
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

    fn set_xattr(&self, attr: &str, data: &[u8]) {
        self.inner
            .lock()
            .unwrap()
            .xattrs
            .insert(attr.to_string(), data.to_vec());
    }

    fn get_xattr(&self, attr: &str) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().xattrs.get(attr).cloned()
    }

    fn list_xattrs(&self) -> Vec<String> {
        self.inner.lock().unwrap().xattrs.keys().cloned().collect()
    }

    fn remove_xattr(&self, attr: &str) -> bool {
        self.inner.lock().unwrap().xattrs.remove(attr).is_some()
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

    fn set_xattr(&self, name: &str, attr: &str, data: &[u8]) -> Result<()> {
        if name != "." {
            return Err(Error::path("setxattr", name, ErrorKind::NotFound));
        }
        self.set_xattr(attr, data);
        Ok(())
    }

    fn get_xattr(&self, name: &str, attr: &str) -> Result<Vec<u8>> {
        if name != "." {
            return Err(Error::path("getxattr", name, ErrorKind::NotFound));
        }
        self.get_xattr(attr)
            .ok_or_else(|| Error::path("getxattr", attr, ErrorKind::NotFound))
    }

    fn list_xattrs(&self, name: &str) -> Result<Vec<String>> {
        if name != "." {
            return Err(Error::path("listxattrs", name, ErrorKind::NotFound));
        }
        Ok(self.list_xattrs())
    }

    fn remove_xattr(&self, name: &str, attr: &str) -> Result<()> {
        if name != "." {
            return Err(Error::path("removexattr", name, ErrorKind::NotFound));
        }
        if self.remove_xattr(attr) {
            Ok(())
        } else {
            Err(Error::path("removexattr", attr, ErrorKind::NotFound))
        }
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
        out.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
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

    fn link(&self, old: &str, new: &str) -> Result<()> {
        let old = clean_path(old);
        let new = clean_path(new);
        if !valid_path(&old) || !valid_path(&new) {
            return Err(Error::path("link", old, ErrorKind::NotFound));
        }
        if self.node(&new).is_some() {
            return Err(Error::path("link", &new, ErrorKind::AlreadyExists));
        }
        let parent = parent_path(&new);
        if parent != "." && !is_dir(self, &parent).unwrap_or(false) {
            return Err(Error::path("link", &new, ErrorKind::NotFound));
        }
        let node = self
            .node(&old)
            .ok_or_else(|| Error::path("link", &old, ErrorKind::NotFound))?;
        if node.metadata().is_dir() {
            return Err(Error::path("link", &old, ErrorKind::PermissionDenied));
        }
        self.nodes.write().unwrap().insert(new, node);
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

    fn set_xattr(&self, name: &str, attr: &str, data: &[u8]) -> Result<()> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("setxattr", &name, ErrorKind::NotFound))?;
        node.set_xattr(attr, data);
        Ok(())
    }

    fn get_xattr(&self, name: &str, attr: &str) -> Result<Vec<u8>> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("getxattr", &name, ErrorKind::NotFound))?;
        node.get_xattr(attr)
            .ok_or_else(|| Error::path("getxattr", attr, ErrorKind::NotFound))
    }

    fn list_xattrs(&self, name: &str) -> Result<Vec<String>> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("listxattrs", &name, ErrorKind::NotFound))?;
        Ok(node.list_xattrs())
    }

    fn remove_xattr(&self, name: &str, attr: &str) -> Result<()> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("removexattr", &name, ErrorKind::NotFound))?;
        if node.remove_xattr(attr) {
            Ok(())
        } else {
            Err(Error::path("removexattr", attr, ErrorKind::NotFound))
        }
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

    fn link(&self, old: &str, new: &str) -> Result<()> {
        let old_path = self.full_path(old)?;
        if std::fs::symlink_metadata(&old_path)?.is_dir() {
            return Err(Error::path("link", old, ErrorKind::PermissionDenied));
        }
        std::fs::hard_link(old_path, self.full_path(new)?)?;
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

#[derive(Clone)]
pub struct R2Fs {
    inner: Arc<R2FsInner>,
}

struct R2FsInner {
    base_path: String,
    store: Arc<dyn ObjectStore>,
}

pub trait ObjectStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Object>>;
    fn put(&self, key: &str, object: Object) -> Result<()>;
    fn compare_and_swap(
        &self,
        key: &str,
        expected: Option<&Object>,
        new: Option<Object>,
    ) -> Result<bool>;
    fn delete(&self, key: &str) -> Result<()>;
    fn list_prefix(&self, prefix: &str) -> Result<Vec<(String, Object)>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    pub metadata: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Object {
    pub fn new(metadata: BTreeMap<String, String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            metadata,
            body: body.into(),
        }
    }
}

#[derive(Default)]
pub struct InMemoryObjectStore {
    objects: RwLock<BTreeMap<String, Object>>,
}

impl InMemoryObjectStore {
    pub fn objects(&self) -> BTreeMap<String, Object> {
        self.objects.read().unwrap().clone()
    }
}

impl ObjectStore for InMemoryObjectStore {
    fn get(&self, key: &str) -> Result<Option<Object>> {
        Ok(self.objects.read().unwrap().get(key).cloned())
    }

    fn put(&self, key: &str, object: Object) -> Result<()> {
        self.objects
            .write()
            .unwrap()
            .insert(key.to_string(), object);
        Ok(())
    }

    fn compare_and_swap(
        &self,
        key: &str,
        expected: Option<&Object>,
        new: Option<Object>,
    ) -> Result<bool> {
        let mut objects = self.objects.write().unwrap();
        if objects.get(key) != expected {
            return Ok(false);
        }
        match new {
            Some(object) => {
                objects.insert(key.to_string(), object);
            }
            None => {
                objects.remove(key);
            }
        }
        Ok(true)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.objects.write().unwrap().remove(key);
        Ok(())
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<(String, Object)>> {
        Ok(self
            .objects
            .read()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, object)| (key.clone(), object.clone()))
            .collect())
    }
}

impl R2Fs {
    const COMPARE_AND_SWAP_RETRIES: usize = 3;

    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self::with_base_path(store, "")
    }

    pub fn with_base_path(store: Arc<dyn ObjectStore>, base_path: impl Into<String>) -> Self {
        let fs = Self {
            inner: Arc::new(R2FsInner {
                base_path: normalize_r2_base_path(&base_path.into()),
                store,
            }),
        };
        let _ = fs.ensure_root();
        fs
    }

    fn key(&self, name: &str) -> String {
        let path = clean_path(name);
        match (self.inner.base_path.as_str(), path.as_str()) {
            ("", ".") => "/".to_string(),
            ("", path) => format!("/{path}"),
            (base, ".") => format!("/{base}"),
            (base, path) => format!("/{base}/{path}"),
        }
    }

    fn store(&self) -> &dyn ObjectStore {
        self.inner.store.as_ref()
    }

    fn ensure_root(&self) -> Result<()> {
        let key = self.key(".");
        if self.store().get(&key)?.is_none() {
            self.put_directory_object(".", Vec::new())?;
        }
        Ok(())
    }

    fn get_object(&self, name: &str) -> Result<Object> {
        self.store()
            .get(&self.key(name))?
            .ok_or_else(|| Error::path("r2fs", name, ErrorKind::NotFound))
    }

    fn put_file_object(&self, name: &str, mode: FileMode, body: Vec<u8>) -> Result<()> {
        self.store().put(
            &self.key(name),
            Object::new(
                r2_metadata("application/octet-stream", mode, body.len()),
                body,
            ),
        )?;
        self.update_parent_listing(name, Some(mode))
    }

    fn put_directory_object(&self, name: &str, entries: Vec<(String, FileMode)>) -> Result<()> {
        self.store()
            .put(&self.key(name), directory_listing_object(entries))
    }

    fn update_parent_listing(&self, name: &str, child_mode: Option<FileMode>) -> Result<()> {
        let parent = parent_path(name);
        let child = base_name(name).to_string();
        if child == "." || child.is_empty() {
            return Ok(());
        }
        let parent_key = self.key(&parent);
        for _ in 0..Self::COMPARE_AND_SWAP_RETRIES {
            let parent_object = match self.store().get(&parent_key)? {
                Some(object) => object,
                None if parent == "." => directory_listing_object(Vec::new()),
                None => return Err(Error::path("r2fs", parent, ErrorKind::NotFound)),
            };
            let mut entries = parse_r2_listing(&parent_object.body);
            if let Some(mode) = child_mode {
                entries.insert(child.clone(), mode);
            } else {
                entries.remove(&child);
            }
            let next = directory_listing_object(entries.into_iter().collect());
            if self
                .store()
                .compare_and_swap(&parent_key, Some(&parent_object), Some(next))?
            {
                return Ok(());
            }
        }
        Err(Error::Message(format!(
            "r2fs compare-and-swap conflict for {parent}"
        )))
    }

    fn parse_object(&self, name: &str, object: Object) -> R2Node {
        let metadata = parse_http_metadata(name, &object.metadata, object.body.len() as u64);
        let entries = if metadata.is_dir() {
            parse_r2_listing(&object.body)
                .into_iter()
                .map(|(name, mode)| {
                    DirEntry::new(
                        name.clone(),
                        Metadata {
                            name,
                            mode,
                            size: if mode.is_dir() { 2 } else { 0 },
                            modified: SystemTime::UNIX_EPOCH,
                            uid: 0,
                            gid: 0,
                        },
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        R2Node {
            metadata,
            body: object.body,
            entries,
        }
    }
}

impl FileSystem for R2Fs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let object = self.get_object(&name)?;
        let node = self.parse_object(&name, object);
        if ctx.follow_symlinks && node.metadata.mode.is_symlink() {
            let target = String::from_utf8_lossy(&node.body).into_owned();
            let resolved = if target.starts_with('/') {
                clean_path(target.trim_start_matches('/'))
            } else {
                clean_path(&format!("{}/{}", parent_path(&name), target))
            };
            return self.open(ctx, &resolved);
        }
        if node.metadata.is_dir() {
            return Ok(directory_file(node.metadata, node.entries));
        }
        Ok(Box::new(HttpFile {
            metadata: node.metadata,
            data: Cursor::new(node.body),
            closed: false,
        }))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        if !valid_path(name) {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let object = self.get_object(&name)?;
        let node = self.parse_object(&name, object);
        if ctx.follow_symlinks && node.metadata.mode.is_symlink() {
            let target = String::from_utf8_lossy(&node.body).into_owned();
            let resolved = if target.starts_with('/') {
                clean_path(target.trim_start_matches('/'))
            } else {
                clean_path(&format!("{}/{}", parent_path(&name), target))
            };
            return self.stat(ctx, &resolved);
        }
        Ok(node.metadata)
    }

    fn open_file(&self, name: &str, flags: OpenFlags, perm: FileMode) -> Result<BoxFile> {
        if !flags.is_write() {
            return self.open(&FsContext::new(), name);
        }
        let name = clean_path(name);
        let mut data = if flags.contains(OpenFlags::TRUNC) || flags.contains(OpenFlags::CREATE) {
            Vec::new()
        } else {
            read_file(self, &name).unwrap_or_default()
        };
        let offset = if flags.contains(OpenFlags::APPEND) {
            data.len() as u64
        } else {
            0
        };
        data.shrink_to_fit();
        Ok(Box::new(R2WriteFile {
            fs: self.clone(),
            path: name.clone(),
            metadata: Metadata::file(
                name,
                if perm.bits() == 0 { 0o644 } else { perm.perm() },
                data.len() as u64,
            ),
            data,
            offset,
            closed: false,
        }))
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        self.open_file(
            name,
            OpenFlags::WRONLY | OpenFlags::CREATE | OpenFlags::TRUNC,
            FileMode::from_perm(0o644),
        )
    }

    fn mkdir(&self, name: &str, _perm: FileMode) -> Result<()> {
        let name = clean_path(name);
        let parent = parent_path(&name);
        if self.store().get(&self.key(&parent))?.is_none() {
            return Err(Error::path("mkdir", name, ErrorKind::NotFound));
        }
        self.put_directory_object(&name, Vec::new())?;
        self.update_parent_listing(&name, Some(FileMode::DIR | FileMode::from_perm(0o755)))
    }

    fn remove(&self, name: &str) -> Result<()> {
        let name = clean_path(name);
        let object = self.get_object(&name)?;
        let node = self.parse_object(&name, object);
        if node.metadata.is_dir() && !node.entries.is_empty() {
            return Err(Error::path("remove", name, ErrorKind::NotEmpty));
        }
        self.store().delete(&self.key(&name))?;
        self.update_parent_listing(&name, None)
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        let old = clean_path(old);
        let new = clean_path(new);
        let parent = parent_path(&new);
        if self.store().get(&self.key(&parent))?.is_none() {
            return Err(Error::path("rename", new, ErrorKind::NotFound));
        }
        let old_object = self.get_object(&old)?;
        let old_node = self.parse_object(&old, old_object.clone());
        let old_key = self.key(&old);
        let new_key = self.key(&new);
        self.store().put(&new_key, old_object)?;
        self.store().delete(&old_key)?;

        if old_node.metadata.is_dir() {
            let old_prefix = format!("{old_key}/");
            let new_prefix = format!("{new_key}/");
            for (key, object) in self.store().list_prefix(&old_prefix)? {
                let suffix = key.strip_prefix(&old_prefix).unwrap_or_default();
                self.store().put(&format!("{new_prefix}{suffix}"), object)?;
                self.store().delete(&key)?;
            }
        }

        self.update_parent_listing(&old, None)?;
        self.update_parent_listing(&new, Some(old_node.metadata.mode))
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        let new = clean_path(new);
        let parent = parent_path(&new);
        if self.store().get(&self.key(&parent))?.is_none() {
            return Err(Error::path("symlink", new, ErrorKind::NotFound));
        }
        self.store().put(
            &self.key(&new),
            Object::new(
                r2_metadata(
                    "application/x-symlink",
                    FileMode::SYMLINK | FileMode::from_perm(0o777),
                    old.len(),
                ),
                old.as_bytes().to_vec(),
            ),
        )?;
        self.update_parent_listing(&new, Some(FileMode::SYMLINK | FileMode::from_perm(0o777)))
    }

    fn readlink(&self, name: &str) -> Result<String> {
        let object = self.get_object(name)?;
        let node = self.parse_object(name, object);
        if !node.metadata.mode.is_symlink() {
            return Err(Error::path("readlink", name, ErrorKind::Invalid));
        }
        Ok(String::from_utf8_lossy(&node.body).into_owned())
    }
}

struct R2Node {
    metadata: Metadata,
    body: Vec<u8>,
    entries: Vec<DirEntry>,
}

struct R2WriteFile {
    fs: R2Fs,
    path: String,
    metadata: Metadata,
    data: Vec<u8>,
    offset: u64,
    closed: bool,
}

impl FileHandle for R2WriteFile {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Err(ErrorKind::PermissionDenied.into())
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        let start = self.offset as usize;
        let end = start + data.len();
        if start > self.data.len() {
            self.data.resize(start, 0);
        }
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(data);
        self.offset = end as u64;
        self.metadata.size = self.data.len() as u64;
        Ok(data.len())
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.offset = offset;
        self.write(data)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let next = match pos {
            SeekFrom::Start(pos) => pos as i128,
            SeekFrom::End(offset) => self.data.len() as i128 + offset as i128,
            SeekFrom::Current(offset) => self.offset as i128 + offset as i128,
        };
        if next < 0 {
            return Err(ErrorKind::Invalid.into());
        }
        self.offset = next as u64;
        Ok(self.offset)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(self.metadata.clone())
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        self.fs
            .put_file_object(&self.path, self.metadata.mode, self.data.clone())?;
        self.closed = true;
        Ok(())
    }
}

fn normalize_r2_base_path(base_path: &str) -> String {
    let trimmed = base_path.trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        String::new()
    } else {
        clean_path(trimmed)
    }
}

fn r2_metadata(content_type: &str, mode: FileMode, len: usize) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    metadata.insert("Content-Type".to_string(), content_type.to_string());
    metadata.insert("Content-Mode".to_string(), format_http_mode(mode));
    metadata.insert("Content-Length".to_string(), len.to_string());
    metadata.insert(
        "Content-Modified".to_string(),
        unix_secs(current_time()).to_string(),
    );
    metadata.insert("Content-Ownership".to_string(), "0:0".to_string());
    metadata
}

fn parse_r2_listing(body: &[u8]) -> BTreeMap<String, FileMode> {
    String::from_utf8_lossy(body)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?.to_string();
            let mode = parts.next().map(parse_http_mode)?;
            Some((name, mode))
        })
        .collect()
}

fn format_r2_listing(entries: Vec<(String, FileMode)>) -> String {
    let mut entries = entries;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
        .into_iter()
        .map(|(name, mode)| format!("{} {}\n", name, format_http_mode(mode)))
        .collect()
}

fn directory_listing_object(entries: Vec<(String, FileMode)>) -> Object {
    Object::new(
        r2_metadata(
            "application/x-directory",
            FileMode::DIR | FileMode::from_perm(0o755),
            0,
        ),
        format_r2_listing(entries).into_bytes(),
    )
}

#[derive(Clone)]
pub struct HttpFs {
    inner: Arc<HttpFsInner>,
}

struct HttpFsInner {
    base_url: String,
    transport: Arc<dyn HttpTransport>,
    ignores: RwLock<Vec<String>>,
    cache: Mutex<HttpFsCache>,
}

struct HttpFsCache {
    ttl: Duration,
    stat: BTreeMap<String, HttpFsCacheEntry<Metadata>>,
    node: BTreeMap<String, HttpFsCacheEntry<HttpNode>>,
}

#[derive(Clone)]
struct HttpFsCacheEntry<T> {
    payload: HttpFsCachePayload<T>,
    expires_at: SystemTime,
    validators: HttpValidators,
}

#[derive(Clone)]
enum HttpFsCachePayload<T> {
    Value(T),
    NotFound,
}

#[derive(Clone, Default)]
struct HttpValidators {
    etag: Option<String>,
    last_modified: Option<String>,
}

impl HttpValidators {
    fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

pub trait HttpTransport: Send + Sync {
    fn request(&self, request: HttpRequest) -> Result<HttpResponse>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HttpMultipartPart {
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl HttpFs {
    pub fn new(base_url: impl Into<String>, transport: Arc<dyn HttpTransport>) -> Self {
        Self::with_cache_ttl(base_url, transport, Duration::ZERO)
    }

    pub fn with_cache_ttl(
        base_url: impl Into<String>,
        transport: Arc<dyn HttpTransport>,
        ttl: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(HttpFsInner {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                transport,
                ignores: RwLock::new(Vec::new()),
                cache: Mutex::new(HttpFsCache {
                    ttl,
                    stat: BTreeMap::new(),
                    node: BTreeMap::new(),
                }),
            }),
        }
    }

    pub fn cache_ttl(&self) -> Duration {
        self.inner.cache.lock().unwrap().ttl
    }

    pub fn set_cache_ttl(&self, ttl: Duration) {
        let mut cache = self.inner.cache.lock().unwrap();
        cache.ttl = ttl;
        cache.stat.clear();
        cache.node.clear();
    }

    pub fn ignore(&self, names: impl IntoIterator<Item = impl Into<String>>) {
        self.inner
            .ignores
            .write()
            .unwrap()
            .extend(names.into_iter().map(Into::into));
    }

    fn should_ignore(&self, name: &str) -> bool {
        self.inner
            .ignores
            .read()
            .unwrap()
            .iter()
            .any(|ignore| name.ends_with(ignore))
    }

    fn normalize_http_path(name: &str) -> String {
        let cleaned = clean_path(name);
        if cleaned == "." {
            "/".to_string()
        } else {
            format!("/{cleaned}")
        }
    }

    fn build_url(&self, name: &str) -> String {
        format!("{}{}", self.inner.base_url, Self::normalize_http_path(name))
    }

    fn cache_lookup<T: Clone>(
        entries: &mut BTreeMap<String, HttpFsCacheEntry<T>>,
        name: &str,
    ) -> Option<HttpFsCachePayload<T>> {
        let now = current_time();
        if let Some(entry) = entries.get(name) {
            if entry.expires_at > now {
                return Some(entry.payload.clone());
            }
        }
        None
    }

    fn cache_stale_value<T: Clone>(
        entries: &BTreeMap<String, HttpFsCacheEntry<T>>,
        name: &str,
    ) -> Option<HttpFsCacheEntry<T>> {
        entries.get(name).and_then(|entry| {
            matches!(entry.payload, HttpFsCachePayload::Value(_)).then(|| entry.clone())
        })
    }

    fn cache_store<T: Clone>(
        entries: &mut BTreeMap<String, HttpFsCacheEntry<T>>,
        ttl: Duration,
        name: &str,
        payload: HttpFsCachePayload<T>,
        validators: HttpValidators,
    ) {
        if ttl.is_zero() {
            return;
        }
        entries.insert(
            name.to_string(),
            HttpFsCacheEntry {
                payload,
                expires_at: current_time() + ttl,
                validators,
            },
        );
    }

    fn cached_stat(&self, name: &str) -> Option<Result<Metadata>> {
        let mut cache = self.inner.cache.lock().unwrap();
        Self::cache_lookup(&mut cache.stat, name).map(|payload| match payload {
            HttpFsCachePayload::Value(metadata) => Ok(metadata),
            HttpFsCachePayload::NotFound => Err(Error::path("httpfs", name, ErrorKind::NotFound)),
        })
    }

    fn cache_stat_http_result(
        &self,
        name: &str,
        result: &Result<Metadata>,
        validators: HttpValidators,
    ) {
        let mut cache = self.inner.cache.lock().unwrap();
        let ttl = cache.ttl;
        match result {
            Ok(metadata) => Self::cache_store(
                &mut cache.stat,
                ttl,
                name,
                HttpFsCachePayload::Value(metadata.clone()),
                validators,
            ),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                Self::cache_store(
                    &mut cache.stat,
                    ttl,
                    name,
                    HttpFsCachePayload::NotFound,
                    HttpValidators::default(),
                );
            }
            Err(_) => {}
        }
    }

    fn stale_stat_entry(&self, name: &str) -> Option<HttpFsCacheEntry<Metadata>> {
        let cache = self.inner.cache.lock().unwrap();
        Self::cache_stale_value(&cache.stat, name)
    }

    fn cached_node(&self, name: &str) -> Option<Result<HttpNode>> {
        let mut cache = self.inner.cache.lock().unwrap();
        Self::cache_lookup(&mut cache.node, name).map(|payload| match payload {
            HttpFsCachePayload::Value(node) => Ok(node),
            HttpFsCachePayload::NotFound => Err(Error::path("httpfs", name, ErrorKind::NotFound)),
        })
    }

    fn cache_node_http_result(
        &self,
        name: &str,
        result: &Result<HttpNode>,
        validators: HttpValidators,
    ) {
        let mut cache = self.inner.cache.lock().unwrap();
        let ttl = cache.ttl;
        match result {
            Ok(node) => Self::cache_store(
                &mut cache.node,
                ttl,
                name,
                HttpFsCachePayload::Value(node.clone()),
                validators,
            ),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                Self::cache_store(
                    &mut cache.node,
                    ttl,
                    name,
                    HttpFsCachePayload::NotFound,
                    HttpValidators::default(),
                );
            }
            Err(_) => {}
        }
    }

    fn stale_node_entry(&self, name: &str) -> Option<HttpFsCacheEntry<HttpNode>> {
        let cache = self.inner.cache.lock().unwrap();
        Self::cache_stale_value(&cache.node, name)
    }

    fn invalidate_exact(cache: &mut HttpFsCache, name: &str) {
        cache.stat.remove(name);
        cache.node.remove(name);
    }

    fn invalidate_tree(cache: &mut HttpFsCache, name: &str) {
        if name == "." {
            cache.stat.clear();
            cache.node.clear();
            return;
        }
        cache
            .stat
            .retain(|path, _| path != name && !path.starts_with(&format!("{name}/")));
        cache
            .node
            .retain(|path, _| path != name && !path.starts_with(&format!("{name}/")));
    }

    fn invalidate_cache_after_put(&self, name: &str) {
        let name = clean_path(name);
        let parent = parent_path(&name);
        let mut cache = self.inner.cache.lock().unwrap();
        Self::invalidate_tree(&mut cache, &name);
        Self::invalidate_exact(&mut cache, &parent);
    }

    fn invalidate_cache_after_remove(&self, name: &str) {
        let name = clean_path(name);
        let parent = parent_path(&name);
        let mut cache = self.inner.cache.lock().unwrap();
        Self::invalidate_tree(&mut cache, &name);
        Self::invalidate_exact(&mut cache, &parent);
    }

    fn invalidate_cache_after_rename(&self, old: &str, new: &str) {
        let old = clean_path(old);
        let new = clean_path(new);
        let old_parent = parent_path(&old);
        let new_parent = parent_path(&new);
        let mut cache = self.inner.cache.lock().unwrap();
        Self::invalidate_tree(&mut cache, &old);
        Self::invalidate_tree(&mut cache, &new);
        Self::invalidate_exact(&mut cache, &old_parent);
        Self::invalidate_exact(&mut cache, &new_parent);
    }

    fn invalidate_cache_after_patch(&self, name: &str) {
        let name = clean_path(name);
        let parent = parent_path(&name);
        let mut cache = self.inner.cache.lock().unwrap();
        Self::invalidate_tree(&mut cache, &name);
        Self::invalidate_exact(&mut cache, &parent);
    }

    fn request(
        &self,
        method: Method,
        name: &str,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpResponse> {
        let response = self.raw_request(method, name, headers, body)?;
        Self::response_status_result(name, response)
    }

    fn raw_request(
        &self,
        method: Method,
        name: &str,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpResponse> {
        self.inner.transport.request(HttpRequest {
            method,
            url: self.build_url(name),
            headers,
            body,
        })
    }

    fn response_status_result(name: &str, response: HttpResponse) -> Result<HttpResponse> {
        match response.status {
            200..=299 => Ok(response),
            404 => Err(Error::path("httpfs", name, ErrorKind::NotFound)),
            _ => Err(Error::Message(format!(
                "httpfs {} returned HTTP {}",
                name, response.status
            ))),
        }
    }

    fn load_node(&self, name: &str) -> Result<HttpNode> {
        self.load_node_with_headers(name, BTreeMap::new())
    }

    fn load_node_with_headers(
        &self,
        name: &str,
        mut headers: BTreeMap<String, String>,
    ) -> Result<HttpNode> {
        if let Some(result) = self.cached_node(name) {
            return result;
        }
        if self.should_ignore(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let stale = self.stale_node_entry(name);
        if let Some(entry) = &stale {
            add_validator_headers(&mut headers, &entry.validators);
        }
        let response = self.raw_request(Method::GET, name, headers, Vec::new())?;
        let validators = response_validators(&response.headers, stale.as_ref());
        let result = match response.status {
            304 => stale
                .and_then(|entry| match entry.payload {
                    HttpFsCachePayload::Value(node) => Some(Ok(node)),
                    HttpFsCachePayload::NotFound => None,
                })
                .unwrap_or_else(|| {
                    Err(Error::Message(format!(
                        "httpfs {name} returned HTTP 304 without a cached node"
                    )))
                }),
            _ => Self::response_status_result(name, response)
                .and_then(|response| self.parse_node(name, response)),
        };
        self.cache_node_http_result(name, &result, validators);
        result
    }

    fn load_metadata(&self, name: &str) -> Result<Metadata> {
        if let Some(result) = self.cached_stat(name) {
            return result;
        }
        if self.should_ignore(name) {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        let stale = self.stale_stat_entry(name);
        let mut headers = BTreeMap::new();
        if let Some(entry) = &stale {
            add_validator_headers(&mut headers, &entry.validators);
        }
        let response = self.raw_request(Method::HEAD, name, headers, Vec::new())?;
        let validators = response_validators(&response.headers, stale.as_ref());
        let result = match response.status {
            304 => stale
                .and_then(|entry| match entry.payload {
                    HttpFsCachePayload::Value(metadata) => Some(Ok(metadata)),
                    HttpFsCachePayload::NotFound => None,
                })
                .unwrap_or_else(|| {
                    Err(Error::Message(format!(
                        "httpfs {name} returned HTTP 304 without cached metadata"
                    )))
                }),
            _ => Self::response_status_result(name, response).map(|response| {
                parse_http_metadata(name, &response.headers, response.body.len() as u64)
            }),
        };
        self.cache_stat_http_result(name, &result, validators);
        result
    }

    fn put_node(
        &self,
        name: &str,
        content_type: &str,
        mode: FileMode,
        body: Vec<u8>,
    ) -> Result<()> {
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".to_string(), content_type.to_string());
        headers.insert("Content-Mode".to_string(), format_http_mode(mode));
        headers.insert(
            "Content-Modified".to_string(),
            unix_secs(current_time()).to_string(),
        );
        headers.insert("Content-Ownership".to_string(), "0:0".to_string());
        headers.insert("Content-Length".to_string(), body.len().to_string());
        insert_change_timestamp(&mut headers);
        self.request(Method::PUT, name, headers, body).map(|_| {
            self.invalidate_cache_after_put(name);
        })
    }

    pub fn patch_tar(&self, name: &str, body: impl Into<Vec<u8>>) -> Result<()> {
        if !valid_path(name) {
            return Err(Error::path("patch", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let body = body.into();
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".to_string(), "application/x-tar".to_string());
        headers.insert("Content-Length".to_string(), body.len().to_string());
        insert_change_timestamp(&mut headers);
        self.request(Method::PATCH, &name, headers, body).map(|_| {
            self.invalidate_cache_after_patch(&name);
        })
    }

    fn parse_node(&self, name: &str, response: HttpResponse) -> Result<HttpNode> {
        if let Some(boundary) = multipart_boundary(&response.headers) {
            return self.parse_multipart_node(name, &boundary, response.body);
        }
        let metadata = parse_http_metadata(name, &response.headers, response.body.len() as u64);
        let entries = if metadata.is_dir() {
            parse_http_directory(name, &response.body)
        } else {
            Vec::new()
        };
        Ok(HttpNode {
            metadata,
            body: response.body,
            entries,
        })
    }

    fn parse_multipart_node(&self, name: &str, boundary: &str, body: Vec<u8>) -> Result<HttpNode> {
        let name = clean_path(name);
        let mut root = None;
        let mut entries = BTreeMap::new();

        for part in parse_http_multipart(&body, boundary)? {
            let Some(part_path) = self.multipart_part_path(&part.headers) else {
                continue;
            };
            let metadata = parse_http_metadata(&part_path, &part.headers, part.body.len() as u64);
            if part_path == name {
                for entry in parse_http_directory(&part_path, &part.body) {
                    entries.insert(entry.name.clone(), entry);
                }
                root = Some((metadata, part.body));
                continue;
            }
            if parent_path(&part_path) != name {
                continue;
            }
            let entry_name = base_name(&part_path).to_string();
            let mut entry_metadata = metadata;
            entry_metadata.name = entry_name.clone();
            entries.insert(
                entry_name.clone(),
                DirEntry::new(entry_name, entry_metadata),
            );
        }

        let Some((metadata, body)) = root else {
            return Err(Error::Message(format!(
                "httpfs {name} returned malformed multipart directory listing"
            )));
        };

        Ok(HttpNode {
            metadata,
            body,
            entries: entries.into_values().collect(),
        })
    }

    fn multipart_part_path(&self, headers: &BTreeMap<String, String>) -> Option<String> {
        if let Some(path) = header_value(headers, "Content-Location") {
            return Some(self.normalize_multipart_path(&path));
        }
        let disposition = header_value(headers, "Content-Disposition")?;
        let (_, params) = parse_header_parameters(&disposition);
        let filename = params
            .get("filename")
            .or_else(|| params.get("filename*"))
            .cloned()?;
        Some(self.normalize_multipart_path(&filename))
    }

    fn normalize_multipart_path(&self, raw: &str) -> String {
        let trimmed = raw.trim();
        let without_base = trimmed
            .strip_prefix(&self.inner.base_url)
            .unwrap_or(trimmed);
        let path = if let Some(scheme) = without_base.find("://") {
            let remainder = &without_base[scheme + 3..];
            remainder
                .find('/')
                .map(|idx| &remainder[idx..])
                .unwrap_or("/")
        } else {
            without_base
        };
        let cleaned = path.trim_matches('"').trim_start_matches('/');
        if cleaned.is_empty() {
            ".".to_string()
        } else {
            clean_path(cleaned)
        }
    }
}

impl FileSystem for HttpFs {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let node = self.load_node(&name)?;
        if node.metadata.is_dir() {
            return Ok(directory_file(node.metadata, node.entries));
        }
        Ok(Box::new(HttpFile {
            metadata: node.metadata,
            data: Cursor::new(node.body),
            closed: false,
        }))
    }

    fn stat(&self, _ctx: &FsContext, name: &str) -> Result<Metadata> {
        if !valid_path(name) {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        self.load_metadata(&name)
    }

    fn read_dir(&self, _ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
        if !valid_path(name) {
            return Err(Error::path("readdir", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let node = if let Some(result) = self.cached_node(&name) {
            result?
        } else {
            let mut headers = BTreeMap::new();
            headers.insert("Accept".to_string(), "multipart/mixed".to_string());
            self.load_node_with_headers(&name, headers)?
        };
        if !node.metadata.is_dir() {
            return Err(Error::path("readdir", &name, ErrorKind::Invalid));
        }
        let mut entries = node.entries;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn open_file(&self, name: &str, flags: OpenFlags, perm: FileMode) -> Result<BoxFile> {
        if !flags.is_write() {
            return self.open(&FsContext::new(), name);
        }
        let name = clean_path(name);
        let mut data = if flags.contains(OpenFlags::TRUNC) || flags.contains(OpenFlags::CREATE) {
            Vec::new()
        } else {
            read_file(self, &name).unwrap_or_default()
        };
        let offset = if flags.contains(OpenFlags::APPEND) {
            data.len() as u64
        } else {
            0
        };
        Ok(Box::new(HttpWriteFile {
            fs: self.clone(),
            path: name.clone(),
            metadata: Metadata::file(
                name,
                if perm.bits() == 0 { 0o644 } else { perm.perm() },
                data.len() as u64,
            ),
            data: {
                data.shrink_to_fit();
                data
            },
            offset,
            closed: false,
        }))
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        self.open_file(
            name,
            OpenFlags::WRONLY | OpenFlags::CREATE | OpenFlags::TRUNC,
            FileMode::from_perm(0o644),
        )
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        self.put_node(
            name,
            "application/x-directory",
            FileMode::DIR | FileMode::from_perm(perm.perm()),
            Vec::new(),
        )
    }

    fn remove(&self, name: &str) -> Result<()> {
        let mut headers = BTreeMap::new();
        insert_change_timestamp(&mut headers);
        self.request(Method::DELETE, name, headers, Vec::new())
            .map(|_| self.invalidate_cache_after_remove(name))
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        let mut headers = BTreeMap::new();
        headers.insert(
            "Destination".to_string(),
            Self::normalize_http_path(new).to_string(),
        );
        insert_change_timestamp(&mut headers);
        self.request(
            Method::from_bytes(b"MOVE").unwrap(),
            old,
            headers,
            Vec::new(),
        )
        .map(|_| self.invalidate_cache_after_rename(old, new))
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        self.put_node(
            new,
            "application/x-symlink",
            FileMode::SYMLINK | FileMode::from_perm(0o777),
            old.as_bytes().to_vec(),
        )
    }

    fn readlink(&self, name: &str) -> Result<String> {
        let node = self.load_node(name)?;
        if !node.metadata.mode.is_symlink() {
            return Err(Error::path("readlink", name, ErrorKind::Invalid));
        }
        Ok(String::from_utf8_lossy(&node.body).into_owned())
    }
}

#[derive(Clone)]
struct HttpNode {
    metadata: Metadata,
    body: Vec<u8>,
    entries: Vec<DirEntry>,
}

struct HttpFile {
    metadata: Metadata,
    data: Cursor<Vec<u8>>,
    closed: bool,
}

impl FileHandle for HttpFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        Ok(self.data.read(buf)?)
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        let pos = self.data.position();
        self.data.seek(SeekFrom::Start(offset))?;
        let result = self.data.read(buf).map_err(Error::from);
        self.data.seek(SeekFrom::Start(pos))?;
        result
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        Ok(self.data.seek(pos)?)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(self.metadata.clone())
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        self.closed = true;
        Ok(())
    }
}

struct HttpWriteFile {
    fs: HttpFs,
    path: String,
    metadata: Metadata,
    data: Vec<u8>,
    offset: u64,
    closed: bool,
}

impl FileHandle for HttpWriteFile {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Err(ErrorKind::PermissionDenied.into())
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        let start = self.offset as usize;
        let end = start + data.len();
        if start > self.data.len() {
            self.data.resize(start, 0);
        }
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(data);
        self.offset = end as u64;
        self.metadata.size = self.data.len() as u64;
        Ok(data.len())
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.offset = offset;
        self.write(data)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let next = match pos {
            SeekFrom::Start(pos) => pos as i128,
            SeekFrom::End(offset) => self.data.len() as i128 + offset as i128,
            SeekFrom::Current(offset) => self.offset as i128 + offset as i128,
        };
        if next < 0 {
            return Err(ErrorKind::Invalid.into());
        }
        self.offset = next as u64;
        Ok(self.offset)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(self.metadata.clone())
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        self.fs.put_node(
            &self.path,
            "application/octet-stream",
            self.metadata.mode,
            self.data.clone(),
        )?;
        self.closed = true;
        Ok(())
    }
}

fn parse_http_directory(base: &str, body: &[u8]) -> Vec<DirEntry> {
    String::from_utf8_lossy(body)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let mode = parts.next().map(parse_http_mode).unwrap_or_else(|| {
                if name.ends_with('/') {
                    FileMode::DIR | FileMode::from_perm(0o755)
                } else {
                    FileMode::from_perm(0o644)
                }
            });
            let name = name.trim_end_matches('/').to_string();
            let metadata = Metadata {
                name: name.clone(),
                mode,
                size: if mode.is_dir() { 2 } else { 0 },
                modified: SystemTime::UNIX_EPOCH,
                uid: 0,
                gid: 0,
            };
            let _ = base;
            Some(DirEntry::new(name, metadata))
        })
        .collect()
}

fn parse_http_multipart(body: &[u8], boundary: &str) -> Result<Vec<HttpMultipartPart>> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut cursor = find_bytes(body, &delimiter, 0)
        .ok_or_else(|| Error::Message("multipart body missing boundary".to_string()))?;
    let mut parts = Vec::new();

    loop {
        if !body[cursor..].starts_with(&delimiter) {
            return Err(Error::Message(
                "multipart body has invalid boundary".to_string(),
            ));
        }
        cursor += delimiter.len();
        if body.get(cursor..cursor + 2) == Some(b"--".as_slice()) {
            break;
        }
        cursor = consume_multipart_newline(body, cursor)
            .ok_or_else(|| Error::Message("multipart body missing newline".to_string()))?;
        let (headers, body_start) = parse_http_part_headers(body, cursor)?;
        let (body_end, next_cursor) = find_multipart_body_end(body, &delimiter, body_start)
            .ok_or_else(|| Error::Message("multipart part missing closing boundary".to_string()))?;
        parts.push(HttpMultipartPart {
            headers,
            body: body[body_start..body_end].to_vec(),
        });
        cursor = next_cursor;
    }

    Ok(parts)
}

fn parse_http_metadata(name: &str, headers: &BTreeMap<String, String>, body_len: u64) -> Metadata {
    let content_type = header_value(headers, "Content-Type").unwrap_or_default();
    let mode = header_value(headers, "Content-Mode")
        .as_deref()
        .map(parse_http_mode)
        .unwrap_or_else(|| match content_type.as_str() {
            "application/x-directory" => FileMode::DIR | FileMode::from_perm(0o755),
            "application/x-symlink" => FileMode::SYMLINK | FileMode::from_perm(0o777),
            _ => FileMode::from_perm(0o644),
        });
    let size = header_value(headers, "Content-Length")
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            header_value(headers, "Content-Range")
                .as_deref()
                .and_then(parse_http_content_range_size)
        })
        .unwrap_or(body_len);
    let modified = header_value(headers, "Content-Modified")
        .and_then(|value| value.parse::<u64>().ok())
        .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let (uid, gid) = header_value(headers, "Content-Ownership")
        .and_then(|value| {
            value
                .split_once(':')
                .map(|(uid, gid)| (uid.to_string(), gid.to_string()))
        })
        .map(|(uid, gid)| {
            (
                uid.parse::<u32>().unwrap_or(0),
                gid.parse::<u32>().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    Metadata {
        name: base_name(name).to_string(),
        mode,
        size: if mode.is_dir() { size.max(2) } else { size },
        modified,
        uid,
        gid,
    }
}

fn parse_http_mode(mode: &str) -> FileMode {
    let Ok(bits) = mode.parse::<u32>() else {
        return FileMode::from_perm(0o644);
    };
    let perm = bits & 0o777;
    match bits & 0o170000 {
        0o040000 => FileMode::DIR | FileMode::from_perm(perm),
        0o120000 => FileMode::SYMLINK | FileMode::from_perm(perm),
        _ => FileMode::from_perm(perm),
    }
}

fn format_http_mode(mode: FileMode) -> String {
    mode.unix_type_and_perm().to_string()
}

fn header_value(headers: &BTreeMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn validators_from_headers(headers: &BTreeMap<String, String>) -> HttpValidators {
    HttpValidators {
        etag: header_value(headers, "ETag"),
        last_modified: header_value(headers, "Last-Modified"),
    }
}

fn response_validators<T: Clone>(
    headers: &BTreeMap<String, String>,
    stale: Option<&HttpFsCacheEntry<T>>,
) -> HttpValidators {
    let validators = validators_from_headers(headers);
    if validators.is_empty() {
        stale
            .map(|entry| entry.validators.clone())
            .unwrap_or(validators)
    } else {
        validators
    }
}

fn add_validator_headers(headers: &mut BTreeMap<String, String>, validators: &HttpValidators) {
    if let Some(etag) = &validators.etag {
        headers.insert("If-None-Match".to_string(), etag.clone());
    }
    if let Some(last_modified) = &validators.last_modified {
        headers.insert("If-Modified-Since".to_string(), last_modified.clone());
    }
}

fn multipart_boundary(headers: &BTreeMap<String, String>) -> Option<String> {
    let content_type = header_value(headers, "Content-Type")?;
    let (media_type, params) = parse_header_parameters(&content_type);
    media_type
        .starts_with("multipart/")
        .then(|| params.get("boundary").cloned())
        .flatten()
}

fn parse_header_parameters(value: &str) -> (String, BTreeMap<String, String>) {
    let mut parts = value.split(';');
    let media_type = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let mut params = BTreeMap::new();
    for part in parts {
        let Some((key, raw_value)) = part.trim().split_once('=') else {
            continue;
        };
        let value = raw_value.trim().trim_matches('"').to_string();
        params.insert(key.trim().to_ascii_lowercase(), value);
    }
    (media_type, params)
}

fn parse_http_content_range_size(value: &str) -> Option<u64> {
    value.split_once('/')?.1.trim().parse().ok()
}

fn find_bytes(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start > haystack.len() || haystack.len() - start < needle.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn consume_multipart_newline(body: &[u8], cursor: usize) -> Option<usize> {
    match body.get(cursor..cursor + 2) {
        Some(b"\r\n") => Some(cursor + 2),
        _ if body.get(cursor) == Some(&b'\n') => Some(cursor + 1),
        _ => None,
    }
}

fn parse_http_part_headers(body: &[u8], start: usize) -> Result<(BTreeMap<String, String>, usize)> {
    let crlf_end = find_bytes(body, b"\r\n\r\n", start).map(|idx| (idx, idx + 4));
    let lf_end = find_bytes(body, b"\n\n", start).map(|idx| (idx, idx + 2));
    let (header_end, next) = match (crlf_end, lf_end) {
        (Some(crlf), Some(lf)) => {
            if crlf.0 <= lf.0 {
                crlf
            } else {
                lf
            }
        }
        (Some(crlf), None) => crlf,
        (None, Some(lf)) => lf,
        (None, None) => {
            return Err(Error::Message(
                "multipart part missing header terminator".to_string(),
            ));
        }
    };

    let mut headers = BTreeMap::new();
    for line in String::from_utf8_lossy(&body[start..header_end]).lines() {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(key.trim().to_string(), value.trim().to_string());
    }
    Ok((headers, next))
}

fn find_multipart_body_end(body: &[u8], delimiter: &[u8], start: usize) -> Option<(usize, usize)> {
    if body[start..].starts_with(delimiter) {
        return Some((start, start));
    }
    let mut search = start;
    while let Some(found) = find_bytes(body, delimiter, search) {
        if found >= 2 && &body[found - 2..found] == b"\r\n" {
            return Some((found - 2, found));
        }
        if found >= 1 && body[found - 1] == b'\n' {
            return Some((found - 1, found));
        }
        search = found + 1;
    }
    None
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_micros(time: SystemTime) -> u128 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
}

fn insert_change_timestamp(headers: &mut BTreeMap<String, String>) {
    headers.insert(
        "Change-Timestamp".to_string(),
        unix_micros(current_time()).to_string(),
    );
}

#[derive(Clone)]
pub struct TarFs {
    nodes: Arc<BTreeMap<String, TarNode>>,
}

#[derive(Clone)]
struct TarNode {
    metadata: Metadata,
    data: Vec<u8>,
    linkname: Option<String>,
}

impl TarFs {
    pub fn from_reader(reader: impl Read) -> Result<Self> {
        let mut archive = tar::Archive::new(reader);
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ".".to_string(),
            TarNode {
                metadata: Metadata::dir(".", 0o755),
                data: Vec::new(),
                linkname: None,
            },
        );

        for entry in archive.entries()? {
            let mut entry = entry?;
            let header = entry.header().clone();
            let raw_path = header.path()?.to_string_lossy().into_owned();
            let path = clean_path(raw_path.trim_start_matches('/'));
            if path == "." || !valid_path(&path) {
                continue;
            }

            ensure_tar_parent_dirs(&mut nodes, &path);

            let entry_type = header.entry_type();
            let perm = header.mode().unwrap_or(0o644) & 0o777;
            let modified = header
                .mtime()
                .ok()
                .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let mut data = Vec::new();
            let mut linkname = None;
            let mode = if entry_type.is_dir() {
                FileMode::DIR | FileMode::from_perm(perm)
            } else if entry_type.is_symlink() {
                if let Some(link) = header.link_name()? {
                    linkname = Some(link.to_string_lossy().into_owned());
                }
                FileMode::SYMLINK | FileMode::from_perm(0o777)
            } else {
                entry.read_to_end(&mut data)?;
                FileMode::from_perm(perm)
            };

            let mut metadata = Metadata {
                name: base_name(&path).to_string(),
                mode,
                size: if mode.is_dir() {
                    2
                } else if mode.is_symlink() {
                    linkname.as_deref().map(str::len).unwrap_or(0) as u64
                } else {
                    data.len() as u64
                },
                modified,
                uid: header.uid().unwrap_or(0) as u32,
                gid: header.gid().unwrap_or(0) as u32,
            };
            if metadata.mode.is_symlink() {
                data = linkname.clone().unwrap_or_default().into_bytes();
                metadata.size = data.len() as u64;
            }
            nodes.insert(
                path,
                TarNode {
                    metadata,
                    data,
                    linkname,
                },
            );
        }

        recompute_tar_dir_sizes(&mut nodes);
        Ok(Self {
            nodes: Arc::new(nodes),
        })
    }

    pub fn archive_to_writer(fsys: &dyn FileSystem, writer: impl Write) -> Result<()> {
        let mut builder = tar::Builder::new(writer);
        append_tar_path(&mut builder, fsys, ".")?;
        builder.finish()?;
        Ok(())
    }

    fn node(&self, name: &str) -> Option<TarNode> {
        self.nodes.get(name).cloned()
    }

    fn children_for(&self, name: &str) -> Vec<DirEntry> {
        let mut entries = Vec::new();
        let prefix = if name == "." {
            String::new()
        } else {
            format!("{name}/")
        };
        let mut seen = BTreeSet::new();
        for path in self.nodes.keys() {
            if path == name || path == "." {
                continue;
            }
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let child_name = rest.split('/').next().unwrap();
            if seen.insert(child_name.to_string()) {
                let child_path = if name == "." {
                    child_name.to_string()
                } else {
                    format!("{name}/{child_name}")
                };
                let metadata = self
                    .nodes
                    .get(&child_path)
                    .map(|child| child.metadata.clone())
                    .unwrap_or_else(|| Metadata::dir(child_name, 0o755));
                entries.push(DirEntry::new(child_name.to_string(), metadata));
            }
        }
        entries
    }
}

impl FileSystem for TarFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("open", &name, ErrorKind::NotFound))?;
        if ctx.follow_symlinks && node.metadata.mode.is_symlink() {
            let target = node.linkname.unwrap_or_default();
            let resolved = if target.starts_with('/') {
                clean_path(target.trim_start_matches('/'))
            } else {
                clean_path(&format!("{}/{}", parent_path(&name), target))
            };
            return self.open(ctx, &resolved);
        }
        if node.metadata.is_dir() {
            return Ok(directory_file(node.metadata, self.children_for(&name)));
        }
        Ok(Box::new(TarFile {
            metadata: node.metadata,
            data: Cursor::new(node.data),
            closed: false,
        }))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        if !valid_path(name) {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("stat", &name, ErrorKind::NotFound))?;
        if ctx.follow_symlinks && node.metadata.mode.is_symlink() {
            let target = node.linkname.unwrap_or_default();
            let resolved = if target.starts_with('/') {
                clean_path(target.trim_start_matches('/'))
            } else {
                clean_path(&format!("{}/{}", parent_path(&name), target))
            };
            return self.stat(ctx, &resolved);
        }
        Ok(node.metadata)
    }

    fn readlink(&self, name: &str) -> Result<String> {
        let name = clean_path(name);
        let node = self
            .node(&name)
            .ok_or_else(|| Error::path("readlink", &name, ErrorKind::NotFound))?;
        node.linkname
            .ok_or_else(|| Error::path("readlink", name, ErrorKind::Invalid))
    }
}

struct TarFile {
    metadata: Metadata,
    data: Cursor<Vec<u8>>,
    closed: bool,
}

impl FileHandle for TarFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        Ok(self.data.read(buf)?)
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        let pos = self.data.position();
        self.data.seek(SeekFrom::Start(offset))?;
        let result = self.data.read(buf).map_err(Error::from);
        self.data.seek(SeekFrom::Start(pos))?;
        result
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        Ok(self.data.seek(pos)?)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(self.metadata.clone())
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        self.closed = true;
        Ok(())
    }
}

fn ensure_tar_parent_dirs(nodes: &mut BTreeMap<String, TarNode>, path: &str) {
    let mut dir = parent_path(path);
    let mut dirs = Vec::new();
    while dir != "." {
        dirs.push(dir.clone());
        dir = parent_path(&dir);
    }
    for dir in dirs.into_iter().rev() {
        nodes.entry(dir.clone()).or_insert_with(|| TarNode {
            metadata: Metadata::dir(base_name(&dir), 0o755),
            data: Vec::new(),
            linkname: None,
        });
    }
}

fn recompute_tar_dir_sizes(nodes: &mut BTreeMap<String, TarNode>) {
    let dirs: Vec<String> = nodes
        .iter()
        .filter_map(|(path, node)| node.metadata.is_dir().then_some(path.clone()))
        .collect();
    for dir in dirs {
        let prefix = if dir == "." {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut children = BTreeSet::new();
        for path in nodes.keys() {
            if path == &dir || path == "." {
                continue;
            }
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(child) = rest.split('/').next() {
                children.insert(child.to_string());
            }
        }
        if let Some(node) = nodes.get_mut(&dir) {
            node.metadata.size = 2 + children.len() as u64;
        }
    }
}

fn append_tar_path<W: Write>(
    builder: &mut tar::Builder<W>,
    fsys: &dyn FileSystem,
    path: &str,
) -> Result<()> {
    let metadata = lstat(fsys, path)?;
    let archive_path = if path == "." { "." } else { path };
    let mut header = tar::Header::new_gnu();
    header.set_mode(metadata.mode.perm());
    header.set_mtime(
        metadata
            .modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );
    header.set_uid(metadata.uid as u64);
    header.set_gid(metadata.gid as u64);

    if metadata.mode.is_symlink() {
        let target = fsys.readlink(path)?;
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_link_name(target)?;
        header.set_cksum();
        builder.append_data(&mut header, archive_path, Cursor::new(Vec::new()))?;
    } else if metadata.is_dir() {
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_cksum();
        builder.append_data(&mut header, archive_path, Cursor::new(Vec::new()))?;
        for entry in read_dir(fsys, path)? {
            let child = if path == "." {
                entry.name
            } else {
                format!("{path}/{}", entry.name)
            };
            append_tar_path(builder, fsys, &child)?;
        }
    } else {
        let data = read_file(fsys, path)?;
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, archive_path, Cursor::new(data))?;
    }
    Ok(())
}

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
    fn memfs_hard_links_share_file_data() {
        let fs = MemFs::from_entries([("file", b"before".to_vec())]);
        fs.link("file", "linked").unwrap();
        let mut linked = open(&fs, "linked").unwrap();
        linked.write(b"shared").unwrap();
        linked.close().unwrap();

        assert_eq!(read_file(&fs, "file").unwrap(), b"shared");
        fs.remove("file").unwrap();
        assert_eq!(read_file(&fs, "linked").unwrap(), b"shared");
        assert_eq!(
            fs.link("linked", "linked").unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs.link(".", "dir-link").unwrap_err().kind(),
            ErrorKind::PermissionDenied
        );
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
    fn memfs_xattrs_round_trip_and_remove() {
        let fs = MemFs::from_entries([("file", b"value".to_vec())]);
        set_xattr(&fs, "file", "user.mime_type", b"text/plain").unwrap();
        set_xattr(&fs, "file", "user.author", b"wanix").unwrap();

        assert_eq!(
            get_xattr(&fs, "file", "user.mime_type").unwrap(),
            b"text/plain"
        );
        assert_eq!(
            list_xattrs(&fs, "file").unwrap(),
            vec!["user.author".to_string(), "user.mime_type".to_string()]
        );
        remove_xattr(&fs, "file", "user.author").unwrap();
        assert_eq!(
            list_xattrs(&fs, "file").unwrap(),
            vec!["user.mime_type".to_string()]
        );
        assert_eq!(
            get_xattr(&fs, "file", "user.author").unwrap_err().kind(),
            ErrorKind::NotFound
        );
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

    #[test]
    fn tarfs_reads_files_dirs_and_symlinks() {
        let src = MemFs::from_entries([
            ("bin/tool", b"#!/bin/tool\n".to_vec()),
            ("share/doc/readme", b"read me\n".to_vec()),
        ]);
        src.symlink("../share/doc/readme", "bin/readme").unwrap();

        let mut buf = Vec::new();
        TarFs::archive_to_writer(&src, &mut buf).unwrap();
        let tar = TarFs::from_reader(Cursor::new(buf)).unwrap();

        assert_eq!(read_file(&tar, "bin/tool").unwrap(), b"#!/bin/tool\n");
        assert_eq!(read_file(&tar, "bin/readme").unwrap(), b"read me\n");
        assert!(lstat(&tar, "bin/readme").unwrap().mode.is_symlink());
        assert_eq!(tar.readlink("bin/readme").unwrap(), "../share/doc/readme");

        let root: Vec<_> = read_dir(&tar, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(root, vec!["bin", "share"]);
        let bin: Vec<_> = read_dir(&tar, "bin")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(bin, vec!["readme", "tool"]);
    }

    #[test]
    fn tarfs_is_read_only() {
        let src = MemFs::from_entries([("file", b"value".to_vec())]);
        let mut buf = Vec::new();
        TarFs::archive_to_writer(&src, &mut buf).unwrap();
        let tar = TarFs::from_reader(Cursor::new(buf)).unwrap();

        assert_eq!(read_file(&tar, "file").unwrap(), b"value");
        let create_err = match tar.create("new") {
            Ok(_) => panic!("tarfs unexpectedly created a file"),
            Err(err) => err,
        };
        assert_eq!(create_err.kind(), ErrorKind::NotSupported);
        assert_eq!(
            tar.mkdir("dir", FileMode::DIR | FileMode::from_perm(0o755))
                .unwrap_err()
                .kind(),
            ErrorKind::NotSupported
        );
    }

    #[derive(Default)]
    struct RecordingTransport {
        responses: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl RecordingTransport {
        fn push(&self, response: HttpResponse) {
            self.responses.lock().unwrap().push_back(response);
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl HttpTransport for RecordingTransport {
        fn request(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| Error::Message("missing test response".to_string()))
        }
    }

    fn http_file_response(body: &[u8]) -> HttpResponse {
        HttpResponse::new(200)
            .with_header("Content-Type", "application/octet-stream")
            .with_header("Content-Mode", "33188")
            .with_header("Content-Length", body.len().to_string())
            .with_body(body.to_vec())
    }

    fn http_directory_response(listing: &str) -> HttpResponse {
        HttpResponse::new(200)
            .with_header("Content-Type", "application/x-directory")
            .with_header("Content-Mode", "16877")
            .with_header("Content-Length", listing.len().to_string())
            .with_body(listing.as_bytes().to_vec())
    }

    fn http_symlink_response(target: &str) -> HttpResponse {
        HttpResponse::new(200)
            .with_header("Content-Type", "application/x-symlink")
            .with_header("Content-Mode", "41471")
            .with_header("Content-Length", target.len().to_string())
            .with_body(target.as_bytes().to_vec())
    }

    fn http_head_response(content_type: &str, mode: &str, len: usize) -> HttpResponse {
        HttpResponse::new(200)
            .with_header("Content-Type", content_type)
            .with_header("Content-Mode", mode)
            .with_header("Content-Length", len.to_string())
    }

    fn http_response_from_metadata(metadata: &Metadata, body: Vec<u8>) -> HttpResponse {
        let content_type = if metadata.mode.is_dir() {
            "application/x-directory"
        } else if metadata.mode.is_symlink() {
            "application/x-symlink"
        } else {
            "application/octet-stream"
        };
        HttpResponse::new(200)
            .with_header("Content-Type", content_type)
            .with_header("Content-Mode", format_http_mode(metadata.mode))
            .with_header("Content-Length", metadata.size.to_string())
            .with_header("Content-Modified", unix_secs(metadata.modified).to_string())
            .with_header(
                "Content-Ownership",
                format!("{}:{}", metadata.uid, metadata.gid),
            )
            .with_body(body)
    }

    fn http_response_from_fs(fsys: &dyn FileSystem, name: &str) -> Result<HttpResponse> {
        let metadata = lstat(fsys, name)?;
        if metadata.is_dir() {
            let body = read_dir(fsys, name)?
                .into_iter()
                .map(|entry| format!("{} {}\n", entry.name, format_http_mode(entry.metadata.mode)))
                .collect::<String>()
                .into_bytes();
            return Ok(http_response_from_metadata(&metadata, body));
        }
        let body = if metadata.mode.is_symlink() {
            fsys.readlink(name)?.into_bytes()
        } else {
            read_file(fsys, name)?
        };
        Ok(http_response_from_metadata(&metadata, body))
    }

    fn http_head_from_fs(fsys: &dyn FileSystem, name: &str) -> Result<HttpResponse> {
        let metadata = lstat(fsys, name)?;
        Ok(http_response_from_metadata(&metadata, Vec::new()))
    }

    fn multipart_directory_response(
        path: &str,
        root_listing: &str,
        parts: Vec<(BTreeMap<String, String>, Vec<u8>)>,
    ) -> HttpResponse {
        let boundary = "wanix-boundary";
        let mut body = Vec::new();
        let root_path = HttpFs::normalize_http_path(path);
        let root_headers = BTreeMap::from([
            ("Content-Location".to_string(), root_path),
            (
                "Content-Type".to_string(),
                "application/x-directory".to_string(),
            ),
            ("Content-Mode".to_string(), "16877".to_string()),
            (
                "Content-Length".to_string(),
                (2 + root_listing.lines().count()).to_string(),
            ),
            ("Content-Modified".to_string(), "1700000000".to_string()),
            ("Content-Ownership".to_string(), "0:0".to_string()),
        ]);
        append_multipart_part(&mut body, boundary, &root_headers, root_listing.as_bytes());
        for (headers, part_body) in parts {
            append_multipart_part(&mut body, boundary, &headers, &part_body);
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        HttpResponse::new(200)
            .with_header(
                "Content-Type",
                format!("multipart/mixed; boundary={boundary}"),
            )
            .with_body(body)
    }

    fn append_multipart_part(
        body: &mut Vec<u8>,
        boundary: &str,
        headers: &BTreeMap<String, String>,
        part_body: &[u8],
    ) {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        for (key, value) in headers {
            body.extend_from_slice(format!("{key}: {value}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(part_body);
        body.extend_from_slice(b"\r\n");
    }

    fn multipart_directory_response_from_fs(
        fsys: &dyn FileSystem,
        name: &str,
    ) -> Result<HttpResponse> {
        let metadata = lstat(fsys, name)?;
        let entries = read_dir(fsys, name)?;
        let root_listing = entries
            .iter()
            .map(|entry| format!("{} {}\n", entry.name, format_http_mode(entry.metadata.mode)))
            .collect::<String>();
        let mut parts = Vec::new();
        for entry in entries {
            let child_name = if name == "." {
                entry.name.clone()
            } else {
                format!("{name}/{}", entry.name)
            };
            let child_metadata = lstat(fsys, &child_name)?;
            let mut headers = BTreeMap::from([
                (
                    "Content-Location".to_string(),
                    HttpFs::normalize_http_path(&child_name),
                ),
                (
                    "Content-Type".to_string(),
                    if child_metadata.is_dir() {
                        "application/x-directory".to_string()
                    } else if child_metadata.mode.is_symlink() {
                        "application/x-symlink".to_string()
                    } else {
                        "application/octet-stream".to_string()
                    },
                ),
                (
                    "Content-Mode".to_string(),
                    format_http_mode(child_metadata.mode),
                ),
                (
                    "Content-Modified".to_string(),
                    unix_secs(child_metadata.modified).to_string(),
                ),
                (
                    "Content-Ownership".to_string(),
                    format!("{}:{}", child_metadata.uid, child_metadata.gid),
                ),
            ]);
            let part_body = if child_metadata.is_dir() {
                let body = read_dir(fsys, &child_name)?
                    .into_iter()
                    .map(|entry| {
                        format!("{} {}\n", entry.name, format_http_mode(entry.metadata.mode))
                    })
                    .collect::<String>()
                    .into_bytes();
                headers.insert(
                    "Content-Length".to_string(),
                    child_metadata.size.to_string(),
                );
                body
            } else {
                headers.insert(
                    "Content-Range".to_string(),
                    format!("bytes 0-0/{}", child_metadata.size),
                );
                Vec::new()
            };
            parts.push((headers, part_body));
        }
        Ok(multipart_directory_response(name, &root_listing, parts)
            .with_header("Content-Mode", format_http_mode(metadata.mode)))
    }

    fn tar_patch_bytes(
        entries: impl IntoIterator<Item = (impl Into<String>, impl Into<Vec<u8>>)>,
    ) -> Vec<u8> {
        let patch = MemFs::from_entries(entries);
        let mut body = Vec::new();
        TarFs::archive_to_writer(&patch, &mut body).unwrap();
        body
    }

    fn apply_tar_patch_to_fs(fsys: &dyn FileSystem, root: &str, body: &[u8]) -> Result<()> {
        let patch = TarFs::from_reader(Cursor::new(body.to_vec()))?;
        apply_tar_tree(&patch, ".", fsys, root)
    }

    fn apply_tar_tree(
        patch: &dyn FileSystem,
        patch_path: &str,
        target: &dyn FileSystem,
        target_root: &str,
    ) -> Result<()> {
        let metadata = lstat(patch, patch_path)?;
        let target_path = if patch_path == "." {
            clean_path(target_root)
        } else if target_root == "." {
            clean_path(patch_path)
        } else {
            clean_path(&format!("{target_root}/{patch_path}"))
        };

        if patch_path == "." || metadata.is_dir() {
            mkdir_all(
                target,
                &target_path,
                FileMode::from_perm(metadata.mode.perm()),
            )?;
        } else if metadata.mode.is_symlink() {
            target.symlink(&patch.readlink(patch_path)?, &target_path)?;
        } else {
            mkdir_all(
                target,
                &parent_path(&target_path),
                FileMode::from_perm(0o755),
            )?;
            write_file(
                target,
                &target_path,
                &read_file(patch, patch_path)?,
                FileMode::from_perm(metadata.mode.perm()),
            )?;
        }

        if metadata.is_dir() {
            for entry in read_dir(patch, patch_path)? {
                let child = if patch_path == "." {
                    entry.name
                } else {
                    format!("{patch_path}/{}", entry.name)
                };
                apply_tar_tree(patch, &child, target, target_root)?;
            }
        }
        Ok(())
    }

    struct PatchApplyingTransport {
        base_url: String,
        fs: Arc<MemFs>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl PatchApplyingTransport {
        fn new(base_url: &str, fs: MemFs) -> Self {
            Self {
                base_url: base_url.trim_end_matches('/').to_string(),
                fs: Arc::new(fs),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn request_name(&self, url: &str) -> String {
            let path = url.strip_prefix(&self.base_url).unwrap_or(url);
            let trimmed = path.trim_start_matches('/');
            if trimmed.is_empty() {
                ".".to_string()
            } else {
                clean_path(trimmed)
            }
        }
    }

    impl HttpTransport for PatchApplyingTransport {
        fn request(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().unwrap().push(request.clone());
            let name = self.request_name(&request.url);
            match request.method {
                Method::GET => {
                    if request
                        .headers
                        .get("Accept")
                        .is_some_and(|value| value == "multipart/mixed")
                        && is_dir(self.fs.as_ref(), &name).unwrap_or(false)
                    {
                        multipart_directory_response_from_fs(self.fs.as_ref(), &name)
                    } else {
                        http_response_from_fs(self.fs.as_ref(), &name)
                    }
                }
                Method::HEAD => http_head_from_fs(self.fs.as_ref(), &name),
                Method::PATCH
                    if request
                        .headers
                        .get("Content-Type")
                        .is_some_and(|value| value == "application/x-tar") =>
                {
                    apply_tar_patch_to_fs(self.fs.as_ref(), &name, &request.body)?;
                    Ok(HttpResponse::new(204))
                }
                Method::PATCH => Ok(HttpResponse::new(400)),
                _ => Ok(HttpResponse::new(405)),
            }
        }
    }

    #[test]
    fn httpfs_reads_files_and_directories() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(
            HttpResponse::new(200)
                .with_header("Content-Type", "application/octet-stream")
                .with_header("Content-Mode", "33188")
                .with_header("Content-Length", "5")
                .with_body(b"hello".to_vec()),
        );
        transport.push(
            HttpResponse::new(200)
                .with_header("Content-Type", "application/x-directory")
                .with_header("Content-Mode", "16877")
                .with_body(b"file 33188\nsub 16877\nlink 41471\n".to_vec()),
        );
        let fs = HttpFs::new("https://example.invalid/root", transport.clone());

        assert_eq!(read_file(&fs, "file").unwrap(), b"hello");
        let entries: Vec<_> = read_dir(&fs, ".")
            .unwrap()
            .into_iter()
            .map(|entry| (entry.name, entry.metadata.mode))
            .collect();
        assert_eq!(entries[0].0, "file");
        assert_eq!(entries[1].0, "link");
        assert_eq!(entries[2].0, "sub");
        assert!(entries[2].1.is_dir());

        let requests = transport.requests();
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[0].url, "https://example.invalid/root/file");
        assert_eq!(requests[1].url, "https://example.invalid/root/");
    }

    #[test]
    fn httpfs_reads_multipart_directory_listing_metadata() {
        let transport = Arc::new(RecordingTransport::default());
        transport.push(multipart_directory_response(
            ".",
            "alpha 33188\nbeta 16877\nlink 41471\n",
            vec![
                (
                    BTreeMap::from([
                        ("Content-Location".to_string(), "/alpha".to_string()),
                        (
                            "Content-Type".to_string(),
                            "application/octet-stream".to_string(),
                        ),
                        ("Content-Mode".to_string(), "33188".to_string()),
                        ("Content-Range".to_string(), "bytes 0-0/5".to_string()),
                        ("Content-Modified".to_string(), "1700000000".to_string()),
                        ("Content-Ownership".to_string(), "7:8".to_string()),
                    ]),
                    Vec::new(),
                ),
                (
                    BTreeMap::from([
                        (
                            "Content-Disposition".to_string(),
                            "attachment; filename=\"/beta\"".to_string(),
                        ),
                        (
                            "Content-Type".to_string(),
                            "application/x-directory".to_string(),
                        ),
                        ("Content-Mode".to_string(), "16877".to_string()),
                        ("Content-Length".to_string(), "3".to_string()),
                        ("Content-Modified".to_string(), "1700000001".to_string()),
                        ("Content-Ownership".to_string(), "1:2".to_string()),
                    ]),
                    b"nested 33188\n".to_vec(),
                ),
                (
                    BTreeMap::from([
                        ("Content-Location".to_string(), "/link".to_string()),
                        (
                            "Content-Type".to_string(),
                            "application/x-symlink".to_string(),
                        ),
                        ("Content-Mode".to_string(), "41471".to_string()),
                        ("Content-Range".to_string(), "bytes 0-0/4".to_string()),
                        ("Content-Modified".to_string(), "1700000002".to_string()),
                        ("Content-Ownership".to_string(), "3:4".to_string()),
                    ]),
                    Vec::new(),
                ),
            ],
        ));
        let fs = HttpFs::new("https://example.invalid/root", transport.clone());

        let entries: Vec<_> = read_dir(&fs, ".")
            .unwrap()
            .into_iter()
            .map(|entry| {
                (
                    entry.name,
                    entry.metadata.size,
                    entry.metadata.uid,
                    entry.metadata.gid,
                    entry.metadata.mode,
                )
            })
            .collect();

        assert_eq!(entries[0].0, "alpha");
        assert_eq!(entries[0].1, 5);
        assert_eq!(entries[0].2, 7);
        assert_eq!(entries[0].3, 8);
        assert_eq!(entries[1].0, "beta");
        assert!(entries[1].4.is_dir());
        assert_eq!(entries[1].1, 3);
        assert_eq!(entries[2].0, "link");
        assert!(entries[2].4.is_symlink());
        assert_eq!(entries[2].1, 4);

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].headers["Accept"], "multipart/mixed");
    }

    #[test]
    fn httpfs_cache_reuses_head_and_get_until_ttl_expires() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let transport = Arc::new(RecordingTransport::default());
        transport.push(http_head_response("application/octet-stream", "33188", 5));
        transport.push(http_file_response(b"hello"));
        transport.push(http_head_response("application/octet-stream", "33188", 6));
        transport.push(http_file_response(b"world!"));
        let fs = HttpFs::with_cache_ttl(
            "https://example.invalid/cache",
            transport.clone(),
            Duration::from_secs(10),
        );

        with_test_current_time(base, || {
            assert_eq!(stat(&fs, "file").unwrap().size, 5);
            assert_eq!(read_file(&fs, "file").unwrap(), b"hello");
            assert_eq!(stat(&fs, "file").unwrap().size, 5);
            assert_eq!(read_file(&fs, "file").unwrap(), b"hello");
        });

        with_test_current_time(base + Duration::from_secs(11), || {
            assert_eq!(stat(&fs, "file").unwrap().size, 6);
            assert_eq!(read_file(&fs, "file").unwrap(), b"world!");
        });

        let requests = transport.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method, Method::HEAD);
        assert_eq!(requests[1].method, Method::GET);
        assert_eq!(requests[2].method, Method::HEAD);
        assert_eq!(requests[3].method, Method::GET);
    }

    #[test]
    fn httpfs_cache_revalidates_stale_head_and_get_with_validators() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let transport = Arc::new(RecordingTransport::default());
        transport.push(
            http_head_response("application/octet-stream", "33188", 5)
                .with_header("ETag", "\"stat-v1\"")
                .with_header("Last-Modified", "1700000000"),
        );
        transport.push(
            http_file_response(b"hello")
                .with_header("ETag", "\"node-v1\"")
                .with_header("Last-Modified", "1700000000"),
        );
        transport.push(HttpResponse::new(304));
        transport.push(HttpResponse::new(304));
        let fs = HttpFs::with_cache_ttl(
            "https://example.invalid/cache",
            transport.clone(),
            Duration::from_secs(10),
        );

        with_test_current_time(base, || {
            assert_eq!(stat(&fs, "file").unwrap().size, 5);
            assert_eq!(read_file(&fs, "file").unwrap(), b"hello");
        });

        with_test_current_time(base + Duration::from_secs(11), || {
            assert_eq!(stat(&fs, "file").unwrap().size, 5);
            assert_eq!(read_file(&fs, "file").unwrap(), b"hello");
        });

        let requests = transport.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[2].method, Method::HEAD);
        assert_eq!(requests[2].headers["If-None-Match"], "\"stat-v1\"");
        assert_eq!(requests[2].headers["If-Modified-Since"], "1700000000");
        assert_eq!(requests[3].method, Method::GET);
        assert_eq!(requests[3].headers["If-None-Match"], "\"node-v1\"");
        assert_eq!(requests[3].headers["If-Modified-Since"], "1700000000");
    }

    #[test]
    fn httpfs_cache_caches_not_found_until_ttl_expires() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let transport = Arc::new(RecordingTransport::default());
        transport.push(HttpResponse::new(404));
        transport.push(HttpResponse::new(404));
        transport.push(http_head_response("application/octet-stream", "33188", 1));
        transport.push(http_file_response(b"x"));
        let fs = HttpFs::with_cache_ttl(
            "https://example.invalid/cache",
            transport.clone(),
            Duration::from_secs(10),
        );

        with_test_current_time(base, || {
            assert_eq!(
                stat(&fs, "missing").unwrap_err().kind(),
                ErrorKind::NotFound
            );
            assert_eq!(
                stat(&fs, "missing").unwrap_err().kind(),
                ErrorKind::NotFound
            );
            assert_eq!(
                read_file(&fs, "missing").unwrap_err().kind(),
                ErrorKind::NotFound
            );
            assert_eq!(
                read_file(&fs, "missing").unwrap_err().kind(),
                ErrorKind::NotFound
            );
        });

        with_test_current_time(base + Duration::from_secs(11), || {
            assert_eq!(stat(&fs, "missing").unwrap().size, 1);
            assert_eq!(read_file(&fs, "missing").unwrap(), b"x");
        });

        let requests = transport.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method, Method::HEAD);
        assert_eq!(requests[1].method, Method::GET);
        assert_eq!(requests[2].method, Method::HEAD);
        assert_eq!(requests[3].method, Method::GET);
    }

    #[test]
    fn httpfs_cache_invalidates_put_delete_and_parent_directory_entries() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        with_test_current_time(now, || {
            let transport = Arc::new(RecordingTransport::default());
            transport.push(http_directory_response("file 33188\n"));
            transport.push(http_file_response(b"old"));
            transport.push(HttpResponse::new(200));
            transport.push(http_file_response(b"new"));
            transport.push(http_directory_response("file 33188\n"));
            transport.push(HttpResponse::new(200));
            transport.push(http_directory_response("dir 16877\nfile 33188\n"));
            transport.push(HttpResponse::new(200));
            transport.push(http_symlink_response("file"));
            transport.push(http_directory_response(
                "dir 16877\nfile 33188\nlink 41471\n",
            ));
            transport.push(HttpResponse::new(200));
            transport.push(HttpResponse::new(404));
            transport.push(http_directory_response("dir 16877\nfile 33188\n"));

            let fs = HttpFs::with_cache_ttl(
                "https://example.invalid/fs",
                transport.clone(),
                Duration::from_secs(60),
            );

            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["file"]);
            assert_eq!(read_file(&fs, "file").unwrap(), b"old");

            write_file(&fs, "file", b"new", FileMode::from_perm(0o644)).unwrap();
            assert_eq!(read_file(&fs, "file").unwrap(), b"new");
            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["file"]);

            fs.mkdir("dir", FileMode::from_perm(0o755)).unwrap();
            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["dir", "file"]);

            fs.symlink("file", "link").unwrap();
            assert_eq!(fs.readlink("link").unwrap(), "file");
            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["dir", "file", "link"]);

            fs.remove("link").unwrap();
            assert_eq!(fs.readlink("link").unwrap_err().kind(), ErrorKind::NotFound);
            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["dir", "file"]);

            let requests = transport.requests();
            let methods: Vec<_> = requests
                .iter()
                .map(|request| request.method.as_str().to_string())
                .collect();
            assert_eq!(
                methods,
                vec![
                    "GET", "GET", "PUT", "GET", "GET", "PUT", "GET", "PUT", "GET", "GET", "DELETE",
                    "GET", "GET",
                ]
            );
        });
    }

    #[test]
    fn httpfs_cache_invalidates_old_new_and_parent_paths_on_rename() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        with_test_current_time(now, || {
            let transport = Arc::new(RecordingTransport::default());
            transport.push(http_directory_response("file 33188\n"));
            transport.push(http_directory_response(""));
            transport.push(http_file_response(b"old"));
            transport.push(HttpResponse::new(404));
            transport.push(HttpResponse::new(200));
            transport.push(http_directory_response(""));
            transport.push(http_directory_response("file 33188\n"));
            transport.push(HttpResponse::new(404));
            transport.push(http_file_response(b"old"));

            let fs = HttpFs::with_cache_ttl(
                "https://example.invalid/fs",
                transport.clone(),
                Duration::from_secs(60),
            );

            let src: Vec<_> = read_dir(&fs, "src")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(src, vec!["file"]);
            assert!(read_dir(&fs, "dst").unwrap().is_empty());
            assert_eq!(read_file(&fs, "src/file").unwrap(), b"old");
            assert_eq!(
                read_file(&fs, "dst/file").unwrap_err().kind(),
                ErrorKind::NotFound
            );

            fs.rename("src/file", "dst/file").unwrap();

            assert!(read_dir(&fs, "src").unwrap().is_empty());
            let dst: Vec<_> = read_dir(&fs, "dst")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(dst, vec!["file"]);
            assert_eq!(
                read_file(&fs, "src/file").unwrap_err().kind(),
                ErrorKind::NotFound
            );
            assert_eq!(read_file(&fs, "dst/file").unwrap(), b"old");

            let requests = transport.requests();
            let methods: Vec<_> = requests
                .iter()
                .map(|request| request.method.as_str().to_string())
                .collect();
            assert_eq!(
                methods,
                vec!["GET", "GET", "GET", "GET", "MOVE", "GET", "GET", "GET", "GET"]
            );
        });
    }

    #[test]
    fn httpfs_cache_invalidates_subtree_and_parent_on_patch_tar() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        with_test_current_time(now, || {
            let transport = Arc::new(RecordingTransport::default());
            transport.push(http_directory_response("dir 16877\n"));
            transport.push(http_directory_response("child 33188\n"));
            transport.push(http_file_response(b"old"));
            transport.push(HttpResponse::new(204));
            transport.push(http_directory_response("dir 16877\n"));
            transport.push(http_directory_response("child 33188\nnew 33188\n"));
            transport.push(http_file_response(b"new"));

            let fs = HttpFs::with_cache_ttl(
                "https://example.invalid/fs",
                transport.clone(),
                Duration::from_secs(60),
            );

            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["dir"]);
            let dir: Vec<_> = read_dir(&fs, "dir")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(dir, vec!["child"]);
            assert_eq!(read_file(&fs, "dir/child").unwrap(), b"old");

            fs.patch_tar("dir", b"patch".to_vec()).unwrap();

            let root: Vec<_> = read_dir(&fs, ".")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(root, vec!["dir"]);
            let dir: Vec<_> = read_dir(&fs, "dir")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(dir, vec!["child", "new"]);
            assert_eq!(read_file(&fs, "dir/child").unwrap(), b"new");

            let requests = transport.requests();
            let methods: Vec<_> = requests
                .iter()
                .map(|request| request.method.as_str().to_string())
                .collect();
            assert_eq!(
                methods,
                vec!["GET", "GET", "GET", "PATCH", "GET", "GET", "GET"]
            );
        });
    }

    #[test]
    fn httpfs_write_mkdir_symlink_rename_and_remove_use_protocol_headers() {
        let now = SystemTime::UNIX_EPOCH
            + Duration::from_secs(1_700_000_000)
            + Duration::from_micros(123_456);
        with_test_current_time(now, || {
            let transport = Arc::new(RecordingTransport::default());
            for _ in 0..5 {
                transport.push(HttpResponse::new(200));
            }
            let fs = HttpFs::new("https://example.invalid/fs", transport.clone());

            write_file(&fs, "new.txt", b"content", FileMode::from_perm(0o600)).unwrap();
            fs.mkdir("dir", FileMode::from_perm(0o755)).unwrap();
            fs.symlink("new.txt", "link").unwrap();
            fs.rename("new.txt", "dir/new.txt").unwrap();
            fs.remove("link").unwrap();

            let requests = transport.requests();
            let change_timestamp = "1700000000123456";
            let modified = "1700000000";

            assert_eq!(requests[0].method, Method::PUT);
            assert_eq!(requests[0].url, "https://example.invalid/fs/new.txt");
            assert_eq!(
                requests[0].headers["Content-Type"],
                "application/octet-stream"
            );
            assert_eq!(requests[0].headers["Content-Mode"], "33152");
            assert_eq!(requests[0].headers["Content-Modified"], modified);
            assert_eq!(requests[0].headers["Content-Ownership"], "0:0");
            assert_eq!(requests[0].headers["Content-Length"], "7");
            assert_eq!(requests[0].headers["Change-Timestamp"], change_timestamp);
            assert_eq!(requests[0].body, b"content");

            assert_eq!(requests[1].method, Method::PUT);
            assert_eq!(requests[1].url, "https://example.invalid/fs/dir");
            assert_eq!(
                requests[1].headers["Content-Type"],
                "application/x-directory"
            );
            assert_eq!(requests[1].headers["Content-Mode"], "16877");
            assert_eq!(requests[1].headers["Content-Modified"], modified);
            assert_eq!(requests[1].headers["Content-Ownership"], "0:0");
            assert_eq!(requests[1].headers["Content-Length"], "0");
            assert_eq!(requests[1].headers["Change-Timestamp"], change_timestamp);
            assert!(requests[1].body.is_empty());

            assert_eq!(requests[2].method, Method::PUT);
            assert_eq!(requests[2].url, "https://example.invalid/fs/link");
            assert_eq!(requests[2].headers["Content-Type"], "application/x-symlink");
            assert_eq!(requests[2].headers["Content-Mode"], "41471");
            assert_eq!(requests[2].headers["Content-Modified"], modified);
            assert_eq!(requests[2].headers["Content-Ownership"], "0:0");
            assert_eq!(requests[2].headers["Content-Length"], "7");
            assert_eq!(requests[2].headers["Change-Timestamp"], change_timestamp);
            assert_eq!(requests[2].body, b"new.txt");

            assert_eq!(requests[3].method.as_str(), "MOVE");
            assert_eq!(requests[3].url, "https://example.invalid/fs/new.txt");
            assert_eq!(requests[3].headers["Destination"], "/dir/new.txt");
            assert_eq!(requests[3].headers["Change-Timestamp"], change_timestamp);
            assert!(requests[3].body.is_empty());

            assert_eq!(requests[4].method, Method::DELETE);
            assert_eq!(requests[4].url, "https://example.invalid/fs/link");
            assert_eq!(requests[4].headers["Change-Timestamp"], change_timestamp);
            assert!(requests[4].body.is_empty());
        });
    }

    #[test]
    fn httpfs_patch_tar_uses_patch_method_headers_and_body() {
        let now = SystemTime::UNIX_EPOCH
            + Duration::from_secs(1_700_000_000)
            + Duration::from_micros(123_456);
        with_test_current_time(now, || {
            let transport = Arc::new(RecordingTransport::default());
            transport.push(HttpResponse::new(204));
            let fs = HttpFs::new("https://example.invalid/fs", transport.clone());

            fs.patch_tar("dir", b"tar-patch".to_vec()).unwrap();

            let requests = transport.requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method.as_str(), "PATCH");
            assert_eq!(requests[0].url, "https://example.invalid/fs/dir");
            assert_eq!(requests[0].headers.len(), 3);
            assert_eq!(requests[0].headers["Content-Type"], "application/x-tar");
            assert_eq!(requests[0].headers["Content-Length"], "9");
            assert_eq!(requests[0].headers["Change-Timestamp"], "1700000000123456");
            assert_eq!(requests[0].body, b"tar-patch");
        });
    }

    #[test]
    fn httpfs_patch_tar_maps_http_errors_like_other_requests() {
        let now = SystemTime::UNIX_EPOCH
            + Duration::from_secs(1_700_000_000)
            + Duration::from_micros(123_456);
        with_test_current_time(now, || {
            let transport = Arc::new(RecordingTransport::default());
            transport.push(HttpResponse::new(404));
            transport.push(HttpResponse::new(500));
            let fs = HttpFs::new("https://example.invalid/fs", transport.clone());

            let not_found = fs.patch_tar("dir", b"one".to_vec()).unwrap_err();
            assert_eq!(not_found.kind(), ErrorKind::NotFound);

            let server_error = fs.patch_tar("dir", b"two".to_vec()).unwrap_err();
            match server_error {
                Error::Message(message) => {
                    assert_eq!(message, "httpfs dir returned HTTP 500");
                }
                other => panic!("unexpected error: {other:?}"),
            }

            let requests = transport.requests();
            assert_eq!(requests.len(), 2);
            assert!(requests
                .iter()
                .all(|request| request.method.as_str() == "PATCH"));
            assert!(requests
                .iter()
                .all(|request| request.headers["Content-Type"] == "application/x-tar"));
            assert_eq!(requests[0].headers["Content-Length"], "3");
            assert_eq!(requests[1].headers["Content-Length"], "3");
            assert!(requests
                .iter()
                .all(|request| request.headers["Change-Timestamp"] == "1700000000123456"));
            assert_eq!(requests[0].body, b"one");
            assert_eq!(requests[1].body, b"two");
        });
    }

    #[test]
    fn httpfs_patch_tar_applies_tar_representation_with_fake_transport() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        with_test_current_time(now, || {
            let transport = Arc::new(PatchApplyingTransport::new(
                "https://example.invalid/fs",
                MemFs::from_entries([("dir/child", b"old".to_vec())]),
            ));
            let fs = HttpFs::with_cache_ttl(
                "https://example.invalid/fs",
                transport.clone(),
                Duration::from_secs(60),
            );
            let tar_body = tar_patch_bytes([
                ("child", b"new".to_vec()),
                ("nested/file", b"nested".to_vec()),
            ]);

            let before: Vec<_> = read_dir(&fs, "dir")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(before, vec!["child"]);
            assert_eq!(read_file(&fs, "dir/child").unwrap(), b"old");

            fs.patch_tar("dir", tar_body.clone()).unwrap();

            let after: Vec<_> = read_dir(&fs, "dir")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            assert_eq!(after, vec!["child", "nested"]);
            assert_eq!(read_file(&fs, "dir/child").unwrap(), b"new");
            assert_eq!(read_file(&fs, "dir/nested/file").unwrap(), b"nested");

            let requests = transport.requests();
            assert!(requests.iter().any(|request| {
                request.method == Method::PATCH
                    && request.headers["Content-Type"] == "application/x-tar"
                    && request.headers["Content-Length"] == tar_body.len().to_string()
                    && request.body == tar_body
            }));
        });
    }

    #[test]
    fn r2fs_creates_files_dirs_symlinks_and_directory_listings() {
        let store = Arc::new(InMemoryObjectStore::default());
        let fs = R2Fs::new(store.clone());

        fs.mkdir("etc", FileMode::from_perm(0o755)).unwrap();
        write_file(&fs, "etc/passwd", b"root:x\n", FileMode::from_perm(0o644)).unwrap();
        fs.symlink("passwd", "etc/passwd.link").unwrap();

        assert_eq!(read_file(&fs, "etc/passwd").unwrap(), b"root:x\n");
        assert_eq!(read_file(&fs, "etc/passwd.link").unwrap(), b"root:x\n");
        assert!(lstat(&fs, "etc/passwd.link").unwrap().mode.is_symlink());
        assert_eq!(fs.readlink("etc/passwd.link").unwrap(), "passwd");

        let root: Vec<_> = read_dir(&fs, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(root, vec!["etc"]);
        let etc: Vec<_> = read_dir(&fs, "etc")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(etc, vec!["passwd", "passwd.link"]);

        let objects = store.objects();
        assert!(objects.contains_key("/"));
        assert!(objects.contains_key("/etc"));
        assert_eq!(
            String::from_utf8_lossy(&objects["/etc"].body),
            "passwd 33188\npasswd.link 41471\n"
        );
    }

    #[test]
    fn r2fs_rename_and_remove_keep_parent_listings_consistent() {
        let store = Arc::new(InMemoryObjectStore::default());
        let fs = R2Fs::new(store.clone());

        fs.mkdir("a", FileMode::from_perm(0o755)).unwrap();
        fs.mkdir("b", FileMode::from_perm(0o755)).unwrap();
        write_file(&fs, "a/file", b"value", FileMode::from_perm(0o600)).unwrap();
        fs.rename("a/file", "b/moved").unwrap();
        assert!(read_file(&fs, "a/file").is_err());
        assert_eq!(read_file(&fs, "b/moved").unwrap(), b"value");
        assert!(read_dir(&fs, "a").unwrap().is_empty());
        assert_eq!(
            read_dir(&fs, "b")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["moved"]
        );

        assert_eq!(fs.remove("b").unwrap_err().kind(), ErrorKind::NotEmpty);
        fs.remove("b/moved").unwrap();
        assert!(read_dir(&fs, "b").unwrap().is_empty());
        fs.remove("b").unwrap();
    }

    #[test]
    fn r2fs_base_path_scopes_object_keys() {
        let store = Arc::new(InMemoryObjectStore::default());
        let fs = R2Fs::with_base_path(store.clone(), "tenant/root");
        fs.mkdir("data", FileMode::from_perm(0o755)).unwrap();
        write_file(&fs, "data/file", b"scoped", FileMode::from_perm(0o644)).unwrap();

        assert_eq!(read_file(&fs, "data/file").unwrap(), b"scoped");
        let objects = store.objects();
        assert!(objects.contains_key("/tenant/root"));
        assert!(objects.contains_key("/tenant/root/data"));
        assert!(objects.contains_key("/tenant/root/data/file"));
        assert!(!objects.contains_key("/data/file"));
    }

    struct ConflictOnceObjectStore {
        objects: RwLock<BTreeMap<String, Object>>,
        trigger_key: String,
        injected: Mutex<bool>,
    }

    impl ConflictOnceObjectStore {
        fn new(trigger_key: impl Into<String>) -> Self {
            Self {
                objects: RwLock::new(BTreeMap::new()),
                trigger_key: trigger_key.into(),
                injected: Mutex::new(false),
            }
        }

        fn inject_external_child(objects: &mut BTreeMap<String, Object>, parent_key: &str) {
            let sibling_key = format!("{parent_key}/peer");
            objects.insert(
                sibling_key,
                Object::new(
                    r2_metadata("application/octet-stream", FileMode::from_perm(0o644), 4),
                    b"peer".to_vec(),
                ),
            );
            objects.insert(
                parent_key.to_string(),
                directory_listing_object(vec![("peer".to_string(), FileMode::from_perm(0o644))]),
            );
        }
    }

    impl ObjectStore for ConflictOnceObjectStore {
        fn get(&self, key: &str) -> Result<Option<Object>> {
            Ok(self.objects.read().unwrap().get(key).cloned())
        }

        fn put(&self, key: &str, object: Object) -> Result<()> {
            self.objects
                .write()
                .unwrap()
                .insert(key.to_string(), object);
            Ok(())
        }

        fn compare_and_swap(
            &self,
            key: &str,
            expected: Option<&Object>,
            new: Option<Object>,
        ) -> Result<bool> {
            let mut objects = self.objects.write().unwrap();
            let mut injected = self.injected.lock().unwrap();
            if !*injected && key == self.trigger_key {
                *injected = true;
                Self::inject_external_child(&mut objects, key);
            }
            if objects.get(key) != expected {
                return Ok(false);
            }
            match new {
                Some(object) => {
                    objects.insert(key.to_string(), object);
                }
                None => {
                    objects.remove(key);
                }
            }
            Ok(true)
        }

        fn delete(&self, key: &str) -> Result<()> {
            self.objects.write().unwrap().remove(key);
            Ok(())
        }

        fn list_prefix(&self, prefix: &str) -> Result<Vec<(String, Object)>> {
            Ok(self
                .objects
                .read()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(key, object)| (key.clone(), object.clone()))
                .collect())
        }
    }

    struct AlwaysConflictObjectStore {
        objects: RwLock<BTreeMap<String, Object>>,
        trigger_key: String,
    }

    impl AlwaysConflictObjectStore {
        fn new(trigger_key: impl Into<String>) -> Self {
            Self {
                objects: RwLock::new(BTreeMap::new()),
                trigger_key: trigger_key.into(),
            }
        }
    }

    impl ObjectStore for AlwaysConflictObjectStore {
        fn get(&self, key: &str) -> Result<Option<Object>> {
            Ok(self.objects.read().unwrap().get(key).cloned())
        }

        fn put(&self, key: &str, object: Object) -> Result<()> {
            self.objects
                .write()
                .unwrap()
                .insert(key.to_string(), object);
            Ok(())
        }

        fn compare_and_swap(
            &self,
            key: &str,
            expected: Option<&Object>,
            new: Option<Object>,
        ) -> Result<bool> {
            if key == self.trigger_key {
                return Ok(false);
            }
            let mut objects = self.objects.write().unwrap();
            if objects.get(key) != expected {
                return Ok(false);
            }
            match new {
                Some(object) => {
                    objects.insert(key.to_string(), object);
                }
                None => {
                    objects.remove(key);
                }
            }
            Ok(true)
        }

        fn delete(&self, key: &str) -> Result<()> {
            self.objects.write().unwrap().remove(key);
            Ok(())
        }

        fn list_prefix(&self, prefix: &str) -> Result<Vec<(String, Object)>> {
            Ok(self
                .objects
                .read()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(key, object)| (key.clone(), object.clone()))
                .collect())
        }
    }

    #[test]
    fn r2fs_retries_parent_listing_compare_and_swap_conflicts() {
        let store = Arc::new(ConflictOnceObjectStore::new("/dir"));
        let fs = R2Fs::new(store);

        fs.mkdir("dir", FileMode::from_perm(0o755)).unwrap();
        write_file(&fs, "dir/file", b"value", FileMode::from_perm(0o644)).unwrap();

        assert_eq!(read_file(&fs, "dir/file").unwrap(), b"value");
        assert_eq!(read_file(&fs, "dir/peer").unwrap(), b"peer");
        assert_eq!(
            read_dir(&fs, "dir")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["file", "peer"]
        );
    }

    #[test]
    fn r2fs_reports_parent_listing_compare_and_swap_conflicts() {
        let store = Arc::new(AlwaysConflictObjectStore::new("/dir"));
        let fs = R2Fs::new(store);

        fs.mkdir("dir", FileMode::from_perm(0o755)).unwrap();
        let err = fs
            .update_parent_listing("dir/file", Some(FileMode::from_perm(0o644)))
            .unwrap_err();
        assert_eq!(err.to_string(), "r2fs compare-and-swap conflict for dir");
    }
}
