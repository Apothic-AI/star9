use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{
    clean_path, parent_path, BoxFile, DirEntry, FileHandle, FileMode, FileSystem, FsContext, FsRef,
    Metadata, OpenFlags, Result,
};

pub type CacheFs = MetaCacheFs;

const DEFAULT_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct MetaCacheFs {
    inner: FsRef,
    state: Arc<Mutex<MetaCacheState>>,
}

struct MetaCacheState {
    ttl: Duration,
    cache: BTreeMap<CacheKey, CacheEntry>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CacheKey {
    path: String,
    kind: CacheKind,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CacheKind {
    Stat,
    Lstat,
    ReadDir,
}

#[derive(Clone)]
struct CacheEntry {
    payload: CachePayload,
    renew_at: Option<Instant>,
    expires_at: Instant,
    refreshing: bool,
}

#[derive(Clone)]
enum CachePayload {
    Metadata(Result<Metadata>),
    ReadDir(Result<Vec<DirEntry>>),
}

struct CachedLookup {
    payload: CachePayload,
    refresh: bool,
}

impl Default for MetaCacheFs {
    fn default() -> Self {
        Self::with_ttl(crate::fs_ref(crate::MemFs::new()), DEFAULT_TTL)
    }
}

impl MetaCacheFs {
    pub fn new(inner: FsRef) -> Self {
        Self::with_ttl(inner, DEFAULT_TTL)
    }

    pub fn with_ttl(inner: FsRef, ttl: Duration) -> Self {
        Self {
            inner,
            state: Arc::new(Mutex::new(MetaCacheState {
                ttl,
                cache: BTreeMap::new(),
            })),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.state.lock().unwrap().ttl
    }

    pub fn set_ttl(&self, ttl: Duration) {
        self.state.lock().unwrap().ttl = ttl;
    }

    pub fn invalidate(&self, path: &str) {
        let path = normalize_cache_path(path);
        let mut state = self.state.lock().unwrap();
        state
            .cache
            .remove(&CacheKey::new(path.clone(), CacheKind::Stat));
        state
            .cache
            .remove(&CacheKey::new(path.clone(), CacheKind::Lstat));
        state.cache.remove(&CacheKey::new(path, CacheKind::ReadDir));
    }

    pub fn invalidate_dir(&self, path: &str) {
        let path = normalize_cache_path(path);
        let mut state = self.state.lock().unwrap();
        state.cache.retain(|key, _| {
            !(key.path == path || (path == "." || key.path.starts_with(&format!("{path}/"))))
        });
    }

    pub fn invalidate_all(&self) {
        self.state.lock().unwrap().cache.clear();
    }

    fn key(path: &str, kind: CacheKind) -> CacheKey {
        CacheKey::new(normalize_cache_path(path), kind)
    }

    fn cache_ttl(&self) -> Duration {
        self.state.lock().unwrap().ttl
    }

    fn invalidate_parent_listing(&self, path: &str) {
        let parent = parent_path(&normalize_cache_path(path));
        self.state
            .lock()
            .unwrap()
            .cache
            .remove(&CacheKey::new(parent, CacheKind::ReadDir));
    }

    fn get_cached(&self, key: &CacheKey) -> Option<CachedLookup> {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        let entry = state.cache.get_mut(key)?;
        if now >= entry.expires_at {
            state.cache.remove(key);
            return None;
        }
        let refresh = entry.renew_at.is_some_and(|renew_at| now >= renew_at) && !entry.refreshing;
        if refresh {
            entry.refreshing = true;
        }
        Some(CachedLookup {
            payload: entry.payload.clone(),
            refresh,
        })
    }

    fn set_cached(&self, key: CacheKey, payload: CachePayload) {
        let ttl = self.cache_ttl();
        if ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let is_error = match &payload {
            CachePayload::Metadata(result) => result.is_err(),
            CachePayload::ReadDir(result) => result.is_err(),
        };
        let entry = CacheEntry {
            payload,
            renew_at: (!is_error).then_some(now + ttl / 2),
            expires_at: now + if is_error { ttl / 2 } else { ttl },
            refreshing: false,
        };
        self.state.lock().unwrap().cache.insert(key, entry);
    }

    fn extend_expiry(&self, key: &CacheKey) {
        let mut state = self.state.lock().unwrap();
        let ttl = state.ttl;
        if ttl.is_zero() {
            state.cache.remove(key);
            return;
        }
        if let Some(entry) = state.cache.get_mut(key) {
            let now = Instant::now();
            entry.renew_at = Some(now + ttl / 2);
            entry.expires_at = now + ttl;
            entry.refreshing = false;
        }
    }

    fn stat_context(&self, ctx: &FsContext, name: &str, lstat: bool) -> Result<Metadata> {
        let key = Self::key(
            name,
            if lstat {
                CacheKind::Lstat
            } else {
                CacheKind::Stat
            },
        );
        if let Some(lookup) = self.get_cached(&key) {
            if lookup.refresh {
                self.schedule_refresh_metadata(name.to_string(), key.clone(), lstat);
            }
            if let CachePayload::Metadata(result) = lookup.payload {
                return result;
            }
        }
        let result = if lstat {
            self.inner.lstat(ctx, name)
        } else {
            self.inner.stat(ctx, name)
        };
        self.set_cached(key, CachePayload::Metadata(result.clone()));
        result
    }

    fn read_dir_context(&self, ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
        let key = Self::key(name, CacheKind::ReadDir);
        if let Some(lookup) = self.get_cached(&key) {
            if lookup.refresh {
                self.schedule_refresh_dir(name.to_string(), key.clone());
            }
            if let CachePayload::ReadDir(result) = lookup.payload {
                return result;
            }
        }
        let result = self.inner.read_dir(ctx, name);
        self.set_cached(key, CachePayload::ReadDir(result.clone()));
        result
    }

    fn refresh_metadata(&self, path: String, key: CacheKey, lstat: bool) {
        let ctx = if lstat {
            FsContext::new()
                .no_follow()
                .with_origin(path.clone(), "stat")
        } else {
            FsContext::new().with_origin(path.clone(), "stat")
        };
        let result = if lstat {
            self.inner.lstat(&ctx, &path)
        } else {
            self.inner.stat(&ctx, &path)
        };
        match result {
            Ok(metadata) => self.set_cached(key, CachePayload::Metadata(Ok(metadata))),
            Err(_) => self.extend_expiry(&key),
        }
    }

    fn refresh_dir(&self, path: String, key: CacheKey) {
        let ctx = FsContext::new()
            .read_only()
            .with_origin(path.clone(), "readdir");
        match self.inner.read_dir(&ctx, &path) {
            Ok(entries) => self.set_cached(key, CachePayload::ReadDir(Ok(entries))),
            Err(_) => self.extend_expiry(&key),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn schedule_refresh_metadata(&self, path: String, key: CacheKey, lstat: bool) {
        let fs = self.clone();
        std::thread::spawn(move || fs.refresh_metadata(path, key, lstat));
    }

    #[cfg(target_arch = "wasm32")]
    fn schedule_refresh_metadata(&self, path: String, key: CacheKey, lstat: bool) {
        self.refresh_metadata(path, key, lstat);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn schedule_refresh_dir(&self, path: String, key: CacheKey) {
        let fs = self.clone();
        std::thread::spawn(move || fs.refresh_dir(path, key));
    }

    #[cfg(target_arch = "wasm32")]
    fn schedule_refresh_dir(&self, path: String, key: CacheKey) {
        self.refresh_dir(path, key);
    }
}

impl CacheKey {
    fn new(path: String, kind: CacheKind) -> Self {
        Self { path, kind }
    }
}

impl FileSystem for MetaCacheFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        let file = self.inner.open(ctx, name)?;
        Ok(Box::new(MetaCacheFile::new(self.clone(), name, file)))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.stat_context(ctx, name, false)
    }

    fn lstat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.stat_context(ctx, name, true)
    }

    fn read_dir(&self, ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
        self.read_dir_context(ctx, name)
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        let file = self.inner.create(name)?;
        self.invalidate(name);
        self.invalidate_parent_listing(name);
        Ok(Box::new(MetaCacheFile::new(self.clone(), name, file)))
    }

    fn open_file(&self, name: &str, flags: OpenFlags, perm: FileMode) -> Result<BoxFile> {
        let file = self.inner.open_file(name, flags, perm)?;
        if flags.contains(OpenFlags::CREATE) || flags.contains(OpenFlags::TRUNC) {
            self.invalidate(name);
            self.invalidate_parent_listing(name);
        }
        Ok(Box::new(MetaCacheFile::new(self.clone(), name, file)))
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        self.inner.mkdir(name, perm)?;
        self.invalidate(name);
        self.invalidate_parent_listing(name);
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.inner.remove(name)?;
        self.invalidate_dir(name);
        self.invalidate_parent_listing(name);
        Ok(())
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        self.inner.rename(old, new)?;
        self.invalidate_dir(old);
        self.invalidate_dir(new);
        self.invalidate_parent_listing(old);
        self.invalidate_parent_listing(new);
        Ok(())
    }

    fn link(&self, old: &str, new: &str) -> Result<()> {
        self.inner.link(old, new)?;
        self.invalidate(new);
        self.invalidate_parent_listing(new);
        Ok(())
    }

    fn chmod(&self, name: &str, mode: FileMode) -> Result<()> {
        self.inner.chmod(name, mode)?;
        self.invalidate(name);
        self.invalidate_parent_listing(name);
        Ok(())
    }

    fn chown(&self, name: &str, uid: u32, gid: u32) -> Result<()> {
        self.inner.chown(name, uid, gid)?;
        self.invalidate(name);
        self.invalidate_parent_listing(name);
        Ok(())
    }

    fn chtimes(&self, name: &str, mtime: std::time::SystemTime) -> Result<()> {
        self.inner.chtimes(name, mtime)?;
        self.invalidate(name);
        self.invalidate_parent_listing(name);
        Ok(())
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        self.inner.truncate(name, size)?;
        self.invalidate(name);
        self.invalidate_parent_listing(name);
        Ok(())
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        self.inner.symlink(old, new)?;
        self.invalidate(new);
        self.invalidate_parent_listing(new);
        Ok(())
    }

    fn readlink(&self, name: &str) -> Result<String> {
        self.inner.readlink(name)
    }

    fn set_xattr(&self, name: &str, attr: &str, data: &[u8]) -> Result<()> {
        self.inner.set_xattr(name, attr, data)
    }

    fn get_xattr(&self, name: &str, attr: &str) -> Result<Vec<u8>> {
        self.inner.get_xattr(name, attr)
    }

    fn list_xattrs(&self, name: &str) -> Result<Vec<String>> {
        self.inner.list_xattrs(name)
    }

    fn remove_xattr(&self, name: &str, attr: &str) -> Result<()> {
        self.inner.remove_xattr(name, attr)
    }

    fn watch(&self, name: &str) -> Result<BoxFile> {
        self.inner.watch(name)
    }

    fn sync_fs(&self) -> Result<()> {
        self.inner.sync_fs()
    }
}

struct MetaCacheFile {
    fs: MetaCacheFs,
    path: String,
    inner: BoxFile,
    dirty: bool,
    dir_entries: Option<Vec<DirEntry>>,
    dir_offset: usize,
}

impl MetaCacheFile {
    fn new(fs: MetaCacheFs, path: &str, inner: BoxFile) -> Self {
        Self {
            fs,
            path: normalize_cache_path(path),
            inner,
            dirty: false,
            dir_entries: None,
            dir_offset: 0,
        }
    }
}

impl FileHandle for MetaCacheFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let written = self.inner.write(data)?;
        if written > 0 {
            self.dirty = true;
        }
        Ok(written)
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.inner.read_at(buf, offset)
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        let written = self.inner.write_at(data, offset)?;
        if written > 0 {
            self.dirty = true;
        }
        Ok(written)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.inner.seek(pos)
    }

    fn stat(&self) -> Result<Metadata> {
        self.fs.stat(
            &FsContext::new().with_origin(self.path.clone(), "stat"),
            &self.path,
        )
    }

    fn read_dir(&mut self, count: isize) -> Result<Vec<DirEntry>> {
        if self.dir_entries.is_none() {
            let entries = self.fs.read_dir(
                &FsContext::new()
                    .read_only()
                    .with_origin(self.path.clone(), "readdir"),
                &self.path,
            )?;
            self.dir_entries = Some(entries);
            self.dir_offset = 0;
        }
        let entries = self.dir_entries.as_ref().unwrap();
        if count <= 0 {
            let remaining = entries[self.dir_offset..].to_vec();
            self.dir_offset = entries.len();
            return Ok(remaining);
        }
        if self.dir_offset >= entries.len() {
            return Ok(Vec::new());
        }
        let end = (self.dir_offset + count as usize).min(entries.len());
        let out = entries[self.dir_offset..end].to_vec();
        self.dir_offset = end;
        Ok(out)
    }

    fn sync(&mut self) -> Result<()> {
        self.inner.sync()
    }

    fn close(&mut self) -> Result<()> {
        let result = self.inner.close();
        if self.dirty {
            self.fs.invalidate(&self.path);
            self.fs.invalidate_parent_listing(&self.path);
        }
        result
    }
}

fn normalize_cache_path(path: &str) -> String {
    clean_path(path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{CacheFs, MetaCacheFs};
    use crate::{
        fs_ref, read_dir, read_file, write_file, DirEntry, Error, ErrorKind, FileMode, FileSystem,
        FsContext, MemFs, Metadata, Result,
    };

    #[derive(Clone, Default)]
    struct CountingFs {
        inner: MemFs,
        stat_calls: Arc<AtomicUsize>,
        lstat_calls: Arc<AtomicUsize>,
        read_dir_calls: Arc<AtomicUsize>,
        stat_failures: Arc<AtomicUsize>,
    }

    impl CountingFs {
        fn new() -> Self {
            Self::default()
        }

        fn stat_calls(&self) -> usize {
            self.stat_calls.load(Ordering::SeqCst)
        }

        fn lstat_calls(&self) -> usize {
            self.lstat_calls.load(Ordering::SeqCst)
        }

        fn read_dir_calls(&self) -> usize {
            self.read_dir_calls.load(Ordering::SeqCst)
        }

        fn fail_next_stats(&self, count: usize) {
            self.stat_failures.store(count, Ordering::SeqCst);
        }
    }

    impl FileSystem for CountingFs {
        fn open(&self, ctx: &FsContext, name: &str) -> Result<crate::BoxFile> {
            self.inner.open(ctx, name)
        }

        fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
            self.stat_calls.fetch_add(1, Ordering::SeqCst);
            if self
                .stat_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    if count > 0 {
                        Some(count - 1)
                    } else {
                        None
                    }
                })
                .is_ok()
            {
                return Err(Error::path("stat", name, ErrorKind::Other));
            }
            self.inner.stat(ctx, name)
        }

        fn lstat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
            self.lstat_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.lstat(ctx, name)
        }

        fn read_dir(&self, ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
            self.read_dir_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.read_dir(ctx, name)
        }

        fn create(&self, name: &str) -> Result<crate::BoxFile> {
            self.inner.create(name)
        }

        fn open_file(
            &self,
            name: &str,
            flags: crate::OpenFlags,
            perm: FileMode,
        ) -> Result<crate::BoxFile> {
            self.inner.open_file(name, flags, perm)
        }

        fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
            self.inner.mkdir(name, perm)
        }

        fn remove(&self, name: &str) -> Result<()> {
            self.inner.remove(name)
        }

        fn rename(&self, old: &str, new: &str) -> Result<()> {
            self.inner.rename(old, new)
        }

        fn chmod(&self, name: &str, mode: FileMode) -> Result<()> {
            self.inner.chmod(name, mode)
        }

        fn chown(&self, name: &str, uid: u32, gid: u32) -> Result<()> {
            self.inner.chown(name, uid, gid)
        }

        fn chtimes(&self, name: &str, mtime: std::time::SystemTime) -> Result<()> {
            self.inner.chtimes(name, mtime)
        }

        fn truncate(&self, name: &str, size: u64) -> Result<()> {
            self.inner.truncate(name, size)
        }

        fn symlink(&self, old: &str, new: &str) -> Result<()> {
            self.inner.symlink(old, new)
        }

        fn readlink(&self, name: &str) -> Result<String> {
            self.inner.readlink(name)
        }
    }

    #[test]
    fn metacache_cached_successes_do_not_hit_inner_repeatedly() {
        let inner = CountingFs::new();
        write_file(&inner, "note.txt", b"hello", FileMode::from_perm(0o644)).unwrap();
        let cache = CacheFs::new(fs_ref(inner.clone()));

        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);
        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 1);

        assert_eq!(
            cache
                .lstat(&FsContext::new().no_follow(), "note.txt")
                .unwrap()
                .size,
            5
        );
        assert_eq!(
            cache
                .lstat(&FsContext::new().no_follow(), "note.txt")
                .unwrap()
                .size,
            5
        );
        assert_eq!(inner.lstat_calls(), 1);

        let names: Vec<_> = read_dir(&cache, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["note.txt"]);
        let names: Vec<_> = read_dir(&cache, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["note.txt"]);
        assert_eq!(inner.read_dir_calls(), 1);
    }

    #[test]
    fn metacache_caches_errors_until_error_ttl_expires() {
        let inner = CountingFs::new();
        let cache = MetaCacheFs::with_ttl(fs_ref(inner.clone()), Duration::from_millis(80));

        let err = cache.stat(&FsContext::new(), "missing.txt").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        let err = cache.stat(&FsContext::new(), "missing.txt").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert_eq!(inner.stat_calls(), 1);

        thread::sleep(Duration::from_millis(55));
        let err = cache.stat(&FsContext::new(), "missing.txt").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert_eq!(inner.stat_calls(), 2);
    }

    #[test]
    fn metacache_expires_success_entries_after_ttl() {
        let inner = CountingFs::new();
        write_file(&inner, "note.txt", b"hello", FileMode::from_perm(0o644)).unwrap();
        let cache = MetaCacheFs::with_ttl(fs_ref(inner.clone()), Duration::from_millis(50));

        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 1);

        thread::sleep(Duration::from_millis(70));
        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 2);
    }

    #[test]
    fn metacache_refreshes_ahead_without_dropping_cached_value() {
        let inner = CountingFs::new();
        write_file(&inner, "note.txt", b"hello", FileMode::from_perm(0o644)).unwrap();
        let cache = MetaCacheFs::with_ttl(fs_ref(inner.clone()), Duration::from_millis(120));

        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 1);

        thread::sleep(Duration::from_millis(70));
        inner.fail_next_stats(1);
        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);

        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline && inner.stat_calls() < 2 {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(inner.stat_calls(), 2);

        thread::sleep(Duration::from_millis(80));
        assert_eq!(cache.stat(&FsContext::new(), "note.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 2);
    }

    #[test]
    fn metacache_invalidates_parent_listing_and_path_on_mutation() {
        let inner = CountingFs::new();
        write_file(&inner, "data.txt", b"hello", FileMode::from_perm(0o644)).unwrap();
        let cache = MetaCacheFs::new(fs_ref(inner.clone()));

        let root_names: Vec<_> = read_dir(&cache, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(root_names, vec!["data.txt"]);
        assert_eq!(inner.read_dir_calls(), 1);

        assert_eq!(cache.stat(&FsContext::new(), "data.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 1);

        let mut file = cache
            .open_file(
                "data.txt",
                crate::OpenFlags::WRONLY,
                FileMode::from_perm(0o644),
            )
            .unwrap();
        file.write(b"xy").unwrap();

        assert_eq!(cache.stat(&FsContext::new(), "data.txt").unwrap().size, 5);
        assert_eq!(inner.stat_calls(), 1);

        let cached_again = cache.stat(&FsContext::new(), "data.txt").unwrap();
        assert_eq!(cached_again.size, 5);
        assert_eq!(inner.stat_calls(), 1);

        file.close().unwrap();

        let stat_after_close = cache.stat(&FsContext::new(), "data.txt").unwrap();
        assert_eq!(stat_after_close.size, 5);
        assert_eq!(inner.stat_calls(), 2);

        let refreshed_names: Vec<_> = read_dir(&cache, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(refreshed_names, vec!["data.txt"]);
        assert_eq!(inner.read_dir_calls(), 2);
        assert_eq!(read_file(&cache, "data.txt").unwrap(), b"xyllo");
    }

    #[test]
    fn metacache_create_remove_and_rename_invalidate_directory_views() {
        let inner = CountingFs::new();
        let cache = MetaCacheFs::new(fs_ref(inner.clone()));

        assert!(read_dir(&cache, ".").unwrap().is_empty());
        assert_eq!(inner.read_dir_calls(), 1);

        let mut created = cache.create("first.txt").unwrap();
        created.write(b"a").unwrap();
        created.close().unwrap();

        let after_create: Vec<_> = read_dir(&cache, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(after_create, vec!["first.txt"]);
        assert_eq!(inner.read_dir_calls(), 2);

        cache.rename("first.txt", "second.txt").unwrap();
        let after_rename: Vec<_> = read_dir(&cache, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(after_rename, vec!["second.txt"]);
        assert_eq!(
            cache
                .stat(&FsContext::new(), "first.txt")
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );

        cache.remove("second.txt").unwrap();
        assert!(read_dir(&cache, ".").unwrap().is_empty());
    }
}
