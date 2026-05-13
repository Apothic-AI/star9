use std::collections::BTreeMap;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::{
    clean_path, exists, lstat, mkdir_all, parent_path, read_dir, read_file, remove_all, write_file,
    BoxFile, DirEntry, FileHandle, FileMode, FileSystem, FsContext, FsRef, Metadata, OpenFlags,
    Result,
};

pub type RemoteSyncRef = Arc<dyn RemoteSyncBackend>;

pub trait RemoteSyncBackend: Send + Sync {
    fn index(&self) -> Result<FsRef>;
    fn apply_patch(&self, patch: &[u8]) -> Result<()>;
}

#[derive(Clone)]
pub struct SyncFs {
    local: FsRef,
    remote: RemoteSyncRef,
    state: Arc<Mutex<SyncState>>,
    sync_lock: Arc<Mutex<()>>,
}

struct SyncState {
    next_generation: u64,
    dirty: BTreeMap<String, DirtyEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyEntry {
    pub change: DirtyChange,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirtyChange {
    Upsert,
    Remove { recursive: bool },
}

impl SyncFs {
    pub fn new(local: FsRef, remote: RemoteSyncRef) -> Self {
        Self {
            local,
            remote,
            state: Arc::new(Mutex::new(SyncState {
                next_generation: 0,
                dirty: BTreeMap::new(),
            })),
            sync_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn local(&self) -> FsRef {
        self.local.clone()
    }

    pub fn dirty(&self) -> BTreeMap<String, DirtyChange> {
        self.state
            .lock()
            .unwrap()
            .dirty
            .iter()
            .map(|(path, entry)| (path.clone(), entry.change))
            .collect()
    }

    pub fn push(&self) -> Result<()> {
        let _guard = self.sync_lock.lock().unwrap();
        let dirty = self.state.lock().unwrap().dirty.clone();
        if dirty.is_empty() {
            return Ok(());
        }

        let patch = self.build_patch(&dirty)?;
        if patch.is_empty() {
            return Ok(());
        }

        self.remote.apply_patch(&patch)?;

        let mut state = self.state.lock().unwrap();
        for (path, snapshot) in dirty {
            if state
                .dirty
                .get(&path)
                .is_some_and(|current| current.generation == snapshot.generation)
            {
                state.dirty.remove(&path);
            }
        }
        Ok(())
    }

    pub fn pull(&self) -> Result<()> {
        let _guard = self.sync_lock.lock().unwrap();
        let remote = self.remote.index()?;
        let dirty = self.state.lock().unwrap().dirty.clone();
        let dirty_paths: BTreeMap<_, _> = dirty
            .iter()
            .map(|(path, entry)| (path.clone(), entry.change))
            .collect();

        let remote_paths = collect_paths(remote.as_ref())?;
        let local_paths = collect_paths(self.local.as_ref())?;

        for path in local_paths.iter().rev() {
            if path_is_dirty(&dirty_paths, path) || remote_paths.contains(path) {
                continue;
            }
            remove_local_path(self.local.as_ref(), path)?;
        }

        for path in &remote_paths {
            if path_is_dirty(&dirty_paths, path) {
                continue;
            }
            sync_path_from_remote(remote.as_ref(), self.local.as_ref(), path)?;
        }

        Ok(())
    }

    pub fn sync(&self) -> Result<()> {
        self.push()?;
        self.pull()
    }

    fn mark_dirty(&self, path: &str, change: DirtyChange) {
        let path = clean_path(path);
        let mut state = self.state.lock().unwrap();
        state.next_generation += 1;
        let generation = state.next_generation;
        state.dirty.insert(path, DirtyEntry { change, generation });
    }

    fn build_patch(&self, dirty: &BTreeMap<String, DirtyEntry>) -> Result<Vec<u8>> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, entry) in dirty {
            match entry.change {
                DirtyChange::Upsert => append_patch_path(&mut builder, self.local.as_ref(), path)?,
                DirtyChange::Remove { recursive } => {
                    append_delete_marker(&mut builder, path, recursive)?
                }
            }
        }
        builder.finish()?;
        Ok(builder.into_inner()?)
    }
}

impl FileSystem for SyncFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        self.local.open(ctx, name)
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.local.stat(ctx, name)
    }

    fn lstat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.local.lstat(ctx, name)
    }

    fn read_dir(&self, ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
        self.local.read_dir(ctx, name)
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        let file = self.local.create(name)?;
        Ok(Box::new(SyncFile::new(self.clone(), name, file, true)))
    }

    fn open_file(&self, name: &str, flags: OpenFlags, perm: FileMode) -> Result<BoxFile> {
        let file = self.local.open_file(name, flags, perm)?;
        let mark_on_close = flags.contains(OpenFlags::CREATE) || flags.contains(OpenFlags::TRUNC);
        Ok(Box::new(SyncFile::new(
            self.clone(),
            name,
            file,
            mark_on_close,
        )))
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        self.local.mkdir(name, perm)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        let recursive = lstat(self.local.as_ref(), name)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        self.local.remove(name)?;
        self.mark_dirty(name, DirtyChange::Remove { recursive });
        Ok(())
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        let recursive = lstat(self.local.as_ref(), old)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        self.local.rename(old, new)?;
        self.mark_dirty(old, DirtyChange::Remove { recursive });
        self.mark_dirty(new, DirtyChange::Upsert);
        Ok(())
    }

    fn chmod(&self, name: &str, mode: FileMode) -> Result<()> {
        self.local.chmod(name, mode)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn chown(&self, name: &str, uid: u32, gid: u32) -> Result<()> {
        self.local.chown(name, uid, gid)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn chtimes(&self, name: &str, mtime: SystemTime) -> Result<()> {
        self.local.chtimes(name, mtime)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        self.local.truncate(name, size)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        self.local.symlink(old, new)?;
        self.mark_dirty(new, DirtyChange::Upsert);
        Ok(())
    }

    fn readlink(&self, name: &str) -> Result<String> {
        self.local.readlink(name)
    }

    fn set_xattr(&self, name: &str, attr: &str, data: &[u8]) -> Result<()> {
        self.local.set_xattr(name, attr, data)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn get_xattr(&self, name: &str, attr: &str) -> Result<Vec<u8>> {
        self.local.get_xattr(name, attr)
    }

    fn list_xattrs(&self, name: &str) -> Result<Vec<String>> {
        self.local.list_xattrs(name)
    }

    fn remove_xattr(&self, name: &str, attr: &str) -> Result<()> {
        self.local.remove_xattr(name, attr)?;
        self.mark_dirty(name, DirtyChange::Upsert);
        Ok(())
    }

    fn watch(&self, name: &str) -> Result<BoxFile> {
        self.local.watch(name)
    }

    fn sync_fs(&self) -> Result<()> {
        self.sync()
    }
}

struct SyncFile {
    owner: SyncFs,
    path: String,
    inner: BoxFile,
    dirty: bool,
    mark_on_close: bool,
}

impl SyncFile {
    fn new(owner: SyncFs, path: &str, inner: BoxFile, mark_on_close: bool) -> Self {
        Self {
            owner,
            path: clean_path(path),
            inner,
            dirty: false,
            mark_on_close,
        }
    }
}

impl FileHandle for SyncFile {
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
        self.inner.stat()
    }

    fn read_dir(&mut self, count: isize) -> Result<Vec<DirEntry>> {
        self.inner.read_dir(count)
    }

    fn sync(&mut self) -> Result<()> {
        self.inner.sync()
    }

    fn close(&mut self) -> Result<()> {
        let result = self.inner.close();
        if result.is_ok() && (self.mark_on_close || self.dirty) {
            self.owner.mark_dirty(&self.path, DirtyChange::Upsert);
        }
        result
    }
}

fn collect_paths(fsys: &dyn FileSystem) -> Result<Vec<String>> {
    let mut out = Vec::new();
    collect_paths_from(fsys, ".", &mut out)?;
    out.sort_by(|a, b| {
        a.split('/')
            .count()
            .cmp(&b.split('/').count())
            .then_with(|| a.cmp(b))
    });
    Ok(out)
}

fn collect_paths_from(fsys: &dyn FileSystem, path: &str, out: &mut Vec<String>) -> Result<()> {
    if path != "." {
        out.push(path.to_string());
    }
    let meta = lstat(fsys, path)?;
    if !meta.is_dir() {
        return Ok(());
    }
    for entry in read_dir(fsys, path)? {
        let child = if path == "." {
            entry.name
        } else {
            format!("{path}/{}", entry.name)
        };
        collect_paths_from(fsys, &child, out)?;
    }
    Ok(())
}

fn path_is_dirty(dirty: &BTreeMap<String, DirtyChange>, path: &str) -> bool {
    let path = clean_path(path);
    if dirty.contains_key(&path) {
        return true;
    }
    let mut parent = parent_path(&path);
    while parent != "." {
        if dirty.contains_key(&parent) {
            return true;
        }
        parent = parent_path(&parent);
    }
    dirty.contains_key(".")
}

fn sync_path_from_remote(
    remote: &dyn FileSystem,
    local: &dyn FileSystem,
    path: &str,
) -> Result<()> {
    let remote_meta = lstat(remote, path)?;
    if remote_meta.is_dir() {
        if !exists(local, path)? {
            mkdir_all(local, path, remote_meta.mode)?;
        } else if !lstat(local, path)?.is_dir() {
            remove_local_path(local, path)?;
            mkdir_all(local, path, remote_meta.mode)?;
        }
        return Ok(());
    }

    if remote_meta.mode.is_symlink() {
        if exists(local, path)? {
            remove_local_path(local, path)?;
        }
        mkdir_all(local, &parent_path(path), FileMode::from_perm(0o755))?;
        return local.symlink(&remote.readlink(path)?, path);
    }

    let data = read_file(remote, path)?;
    if exists(local, path)? {
        let local_meta = lstat(local, path)?;
        if local_meta.is_dir() || local_meta.mode.is_symlink() {
            remove_local_path(local, path)?;
        } else if read_file(local, path)? == data {
            return Ok(());
        }
    } else {
        mkdir_all(local, &parent_path(path), FileMode::from_perm(0o755))?;
    }
    write_file(local, path, &data, remote_meta.mode)
}

fn remove_local_path(local: &dyn FileSystem, path: &str) -> Result<()> {
    let meta = lstat(local, path)?;
    if meta.is_dir() {
        remove_all(local, path)
    } else {
        local.remove(path)
    }
}

fn append_patch_path<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    fsys: &dyn FileSystem,
    path: &str,
) -> Result<()> {
    let metadata = lstat(fsys, path)?;
    let archive_path = clean_path(path);
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
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_link_name(fsys.readlink(path)?)?;
        header.set_cksum();
        builder.append_data(&mut header, &archive_path, Cursor::new(Vec::new()))?;
    } else if metadata.is_dir() {
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_cksum();
        builder.append_data(&mut header, &archive_path, Cursor::new(Vec::new()))?;
        for entry in read_dir(fsys, path)? {
            let child = if path == "." {
                entry.name
            } else {
                format!("{path}/{}", entry.name)
            };
            append_patch_path(builder, fsys, &child)?;
        }
    } else {
        let data = read_file(fsys, path)?;
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, &archive_path, Cursor::new(data))?;
    }
    Ok(())
}

fn append_delete_marker<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    recursive: bool,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0);
    header.set_size(0);
    let mut pax = vec![("delete", b"".as_slice())];
    if recursive {
        pax.push(("recursive", b"1".as_slice()));
    }
    builder.append_pax_extensions(pax)?;
    header.set_cksum();
    builder.append_data(&mut header, clean_path(path), Cursor::new(Vec::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::{DirtyChange, RemoteSyncBackend, SyncFs};
    use crate::{
        clean_path, fs_ref, parent_path, read_dir, read_file, write_file, FileMode, FileSystem,
        MemFs, Result, TarFs,
    };

    #[derive(Default)]
    struct MemoryRemoteSync {
        fs: MemFs,
        index_calls: AtomicUsize,
        patch_calls: AtomicUsize,
    }

    impl MemoryRemoteSync {
        fn fs(&self) -> MemFs {
            self.fs.clone()
        }
    }

    impl RemoteSyncBackend for MemoryRemoteSync {
        fn index(&self) -> Result<crate::FsRef> {
            self.index_calls.fetch_add(1, Ordering::SeqCst);
            let mut buf = Vec::new();
            TarFs::archive_to_writer(&self.fs, &mut buf)?;
            Ok(fs_ref(TarFs::from_reader(Cursor::new(buf))?))
        }

        fn apply_patch(&self, patch: &[u8]) -> Result<()> {
            self.patch_calls.fetch_add(1, Ordering::SeqCst);
            let mut archive = tar::Archive::new(Cursor::new(patch.to_vec()));
            for entry in archive.entries()? {
                let mut entry = entry?;
                let mut is_delete = false;
                let mut recursive = false;
                if let Some(records) = entry.pax_extensions()? {
                    for record in records {
                        let record = record?;
                        if record.key_bytes() == b"delete" {
                            is_delete = true;
                        } else if record.key_bytes() == b"recursive" {
                            recursive = record.value_bytes() == b"1";
                        }
                    }
                }
                let header = entry.header().clone();
                let path = clean_path(&header.path()?.to_string_lossy());
                if is_delete {
                    if recursive {
                        if crate::exists(&self.fs, &path)? {
                            crate::remove_all(&self.fs, &path)?;
                        }
                    } else if crate::exists(&self.fs, &path)? {
                        self.fs.remove(&path)?;
                    }
                    continue;
                }

                if header.entry_type().is_dir() {
                    crate::mkdir_all(&self.fs, &path, FileMode::from_perm(0o755))?;
                    continue;
                }

                if header.entry_type().is_symlink() {
                    if crate::exists(&self.fs, &path)? {
                        if crate::lstat(&self.fs, &path)?.is_dir() {
                            crate::remove_all(&self.fs, &path)?;
                        } else {
                            self.fs.remove(&path)?;
                        }
                    }
                    crate::mkdir_all(&self.fs, &parent_path(&path), FileMode::from_perm(0o755))?;
                    self.fs.symlink(
                        header.link_name()?.unwrap().to_string_lossy().as_ref(),
                        &path,
                    )?;
                    continue;
                }

                let mut data = Vec::new();
                entry.read_to_end(&mut data)?;
                write_file(
                    &self.fs,
                    &path,
                    &data,
                    FileMode::from_perm(header.mode().unwrap_or(0o644)),
                )?;
            }
            Ok(())
        }
    }

    #[test]
    fn syncfs_pushes_local_writes_and_removes() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());

        sync.mkdir("docs", FileMode::from_perm(0o755)).unwrap();
        let mut file = sync.create("docs/readme.txt").unwrap();
        file.write(b"hello").unwrap();
        file.close().unwrap();
        sync.mkdir("tmp", FileMode::from_perm(0o755)).unwrap();
        sync.push().unwrap();

        assert_eq!(
            read_file(&remote.fs(), "docs/readme.txt").unwrap(),
            b"hello"
        );
        assert!(crate::exists(&remote.fs(), "tmp").unwrap());
        assert!(sync.dirty().is_empty());

        sync.remove("docs/readme.txt").unwrap();
        sync.push().unwrap();
        assert!(!crate::exists(&remote.fs(), "docs/readme.txt").unwrap());
        assert_eq!(remote.patch_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn syncfs_pull_imports_remote_files_into_local() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        remote
            .fs()
            .mkdir("srv", FileMode::from_perm(0o755))
            .unwrap();
        write_file(
            &remote.fs(),
            "srv/config.txt",
            b"remote",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        remote
            .fs()
            .symlink("config.txt", "srv/current.txt")
            .unwrap();

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        sync.pull().unwrap();

        assert_eq!(read_file(&local, "srv/config.txt").unwrap(), b"remote");
        assert_eq!(local.readlink("srv/current.txt").unwrap(), "config.txt");
        let names: Vec<_> = read_dir(&local, "srv")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["config.txt", "current.txt"]);
        assert_eq!(remote.index_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn syncfs_pull_keeps_locally_dirty_paths() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        write_file(&local, "notes.txt", b"local", FileMode::from_perm(0o644)).unwrap();
        write_file(
            &remote.fs(),
            "notes.txt",
            b"remote",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let mut file = sync
            .open_file(
                "notes.txt",
                crate::OpenFlags::WRONLY,
                FileMode::from_perm(0o644),
            )
            .unwrap();
        file.write_at(b"LOCAL", 0).unwrap();
        file.close().unwrap();

        assert_eq!(sync.dirty()["notes.txt"], DirtyChange::Upsert);
        sync.pull().unwrap();
        assert_eq!(read_file(&local, "notes.txt").unwrap(), b"LOCAL");
    }

    #[test]
    fn syncfs_writable_open_without_write_does_not_mark_dirty() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        write_file(&local, "notes.txt", b"local", FileMode::from_perm(0o644)).unwrap();

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let mut file = sync
            .open_file(
                "notes.txt",
                crate::OpenFlags::WRONLY,
                FileMode::from_perm(0o644),
            )
            .unwrap();
        file.close().unwrap();

        assert!(sync.dirty().is_empty());
    }

    #[test]
    fn syncfs_sync_fs_pushes_and_pulls() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        write_file(&local, "local.txt", b"mine", FileMode::from_perm(0o644)).unwrap();
        write_file(
            &remote.fs(),
            "remote.txt",
            b"theirs",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let mut file = sync
            .open_file(
                "local.txt",
                crate::OpenFlags::WRONLY,
                FileMode::from_perm(0o644),
            )
            .unwrap();
        file.write_at(b"MINE", 0).unwrap();
        file.close().unwrap();

        sync.sync_fs().unwrap();

        assert_eq!(read_file(&remote.fs(), "local.txt").unwrap(), b"MINE");
        assert_eq!(read_file(&local, "remote.txt").unwrap(), b"theirs");
    }

    #[test]
    fn syncfs_sync_pushes_then_pulls_clean_remote_changes() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        write_file(&local, "local.txt", b"mine", FileMode::from_perm(0o644)).unwrap();
        write_file(
            &remote.fs(),
            "remote.txt",
            b"theirs",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let mut file = sync.create("local.txt").unwrap();
        file.write(b"mine").unwrap();
        file.close().unwrap();

        sync.sync().unwrap();

        assert_eq!(read_file(&remote.fs(), "local.txt").unwrap(), b"mine");
        assert_eq!(read_file(&local, "remote.txt").unwrap(), b"theirs");
    }
}
