use std::collections::BTreeMap;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    sync::Condvar,
    thread::{self, JoinHandle},
};

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
    pull_conflict_policy: PullConflictPolicy,
    retain_pull_conflicts: bool,
    state: Arc<Mutex<SyncState>>,
    sync_lock: Arc<Mutex<()>>,
}

struct SyncState {
    next_generation: u64,
    dirty: BTreeMap<String, DirtyEntry>,
    pull_conflicts: BTreeMap<String, DirtyChange>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullConflictPolicy {
    KeepLocal,
    PreferRemote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncScheduleSnapshot {
    pub pending: bool,
    pub requested_at: Option<SystemTime>,
    pub due_at: Option<SystemTime>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct DebouncedSyncScheduler {
    sync: SyncFs,
    debounce: Duration,
    state: Arc<DebouncedSyncStateCell>,
}

#[derive(Debug, Default)]
struct DebouncedSyncStateCell {
    inner: Mutex<DebouncedSyncState>,
    #[cfg(not(target_arch = "wasm32"))]
    wake: Condvar,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct DebouncedSyncState {
    requested_at: Option<SystemTime>,
    last_error: Option<String>,
    request_serial: u64,
    #[cfg(not(target_arch = "wasm32"))]
    failed_request_serial: Option<u64>,
    #[cfg(not(target_arch = "wasm32"))]
    background_running: bool,
    #[cfg(not(target_arch = "wasm32"))]
    shutdown_requested: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PullPathAction {
    DeleteLocal,
    SyncRemoteDir,
    SyncRemoteLeaf,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct DebouncedSyncBackgroundHandle {
    scheduler: DebouncedSyncScheduler,
    join: Option<JoinHandle<()>>,
}

impl SyncFs {
    pub fn new(local: FsRef, remote: RemoteSyncRef) -> Self {
        Self {
            local,
            remote,
            pull_conflict_policy: PullConflictPolicy::KeepLocal,
            retain_pull_conflicts: false,
            state: Arc::new(Mutex::new(SyncState {
                next_generation: 0,
                dirty: BTreeMap::new(),
                pull_conflicts: BTreeMap::new(),
            })),
            sync_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn with_pull_conflicts_keep_local(mut self) -> Self {
        self.pull_conflict_policy = PullConflictPolicy::KeepLocal;
        self
    }

    pub fn with_pull_conflicts_prefer_remote(mut self) -> Self {
        self.pull_conflict_policy = PullConflictPolicy::PreferRemote;
        self
    }

    pub fn with_pull_conflict_policy(mut self, policy: PullConflictPolicy) -> Self {
        self.pull_conflict_policy = policy;
        self
    }

    pub fn with_pull_conflicts_retained(mut self, retain: bool) -> Self {
        self.retain_pull_conflicts = retain;
        self
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

    pub fn pull_conflicts(&self) -> BTreeMap<String, DirtyChange> {
        self.state.lock().unwrap().pull_conflicts.clone()
    }

    pub fn clear_pull_conflicts(&self) {
        self.state.lock().unwrap().pull_conflicts.clear();
    }

    pub fn scheduler(&self, debounce: Duration) -> DebouncedSyncScheduler {
        DebouncedSyncScheduler::new(self.clone(), debounce)
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
                state.pull_conflicts.remove(&path);
            }
        }
        Ok(())
    }

    pub fn pull(&self) -> Result<()> {
        let _guard = self.sync_lock.lock().unwrap();
        let remote = self.remote.index()?;
        let original_dirty = self.state.lock().unwrap().dirty.clone();
        let mut dirty = original_dirty.clone();
        let mut retained_conflicts = BTreeMap::new();

        let remote_paths = collect_paths(remote.as_ref())?;
        let local_paths = collect_paths(self.local.as_ref())?;

        for path in local_paths.iter().rev() {
            if remote_paths.contains(path) {
                continue;
            }
            let conflicts = conflicting_dirty_paths(
                &dirty,
                path,
                PullPathAction::DeleteLocal,
                self.local.as_ref(),
            )?;
            if conflicts.is_empty() {
                remove_local_path(self.local.as_ref(), path)?;
                continue;
            }

            if self.pull_conflict_policy == PullConflictPolicy::KeepLocal {
                record_conflicts(&mut retained_conflicts, &dirty, &conflicts);
                continue;
            }

            remove_local_path(self.local.as_ref(), path)?;
            clear_dirty_subtree(&mut dirty, path);
        }

        for path in &remote_paths {
            let action = pull_path_action(remote.as_ref(), self.local.as_ref(), path)?;
            let conflicts = conflicting_dirty_paths(&dirty, path, action, self.local.as_ref())?;
            if conflicts.is_empty() {
                sync_path_from_remote(remote.as_ref(), self.local.as_ref(), path)?;
                continue;
            }

            if self.pull_conflict_policy == PullConflictPolicy::KeepLocal {
                record_conflicts(&mut retained_conflicts, &dirty, &conflicts);
                continue;
            }

            sync_path_from_remote(remote.as_ref(), self.local.as_ref(), path)?;
            clear_dirty_paths_for_remote_sync(&mut dirty, path, action);
        }

        let mut state = self.state.lock().unwrap();
        for (path, snapshot) in &original_dirty {
            if dirty.contains_key(path) {
                continue;
            }
            if state
                .dirty
                .get(path)
                .is_some_and(|current| current.generation == snapshot.generation)
            {
                state.dirty.remove(path);
            }
        }
        if self.retain_pull_conflicts {
            state.pull_conflicts = retained_conflicts;
        } else {
            state.pull_conflicts.clear();
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

impl DebouncedSyncScheduler {
    pub fn new(sync: SyncFs, debounce: Duration) -> Self {
        Self {
            sync,
            debounce,
            state: Arc::new(DebouncedSyncStateCell::default()),
        }
    }

    pub fn sync_fs(&self) -> SyncFs {
        self.sync.clone()
    }

    pub fn request(&self, now: SystemTime) {
        let mut state = self.state.inner.lock().unwrap();
        state.requested_at = Some(now);
        state.last_error = None;
        state.request_serial = state.request_serial.wrapping_add(1);
        #[cfg(not(target_arch = "wasm32"))]
        {
            state.failed_request_serial = None;
        }
        drop(state);
        self.notify_waiters();
    }

    pub fn pending(&self) -> bool {
        self.state.inner.lock().unwrap().requested_at.is_some()
    }

    pub fn is_due(&self, now: SystemTime) -> bool {
        let state = self.state.inner.lock().unwrap();
        state
            .requested_at
            .map(|requested_at| self.due_at(requested_at))
            .is_some_and(|due_at| now.duration_since(due_at).is_ok())
    }

    pub fn snapshot(&self) -> SyncScheduleSnapshot {
        let state = self.state.inner.lock().unwrap();
        SyncScheduleSnapshot {
            pending: state.requested_at.is_some(),
            requested_at: state.requested_at,
            due_at: state
                .requested_at
                .map(|requested_at| self.due_at(requested_at)),
            last_error: state.last_error.clone(),
        }
    }

    pub fn run_due(&self, now: SystemTime) -> Result<bool> {
        let request_serial = {
            let state = self.state.inner.lock().unwrap();
            match state.requested_at {
                Some(requested_at) if now.duration_since(self.due_at(requested_at)).is_ok() => {
                    state.request_serial
                }
                _ => return Ok(false),
            }
        };

        self.run_sync_attempt(request_serial)?;
        Ok(true)
    }

    pub fn flush(&self) -> Result<()> {
        let request_serial = self.state.inner.lock().unwrap().request_serial;
        self.run_sync_attempt(request_serial)
    }

    fn due_at(&self, requested_at: SystemTime) -> SystemTime {
        requested_at
            .checked_add(self.debounce)
            .unwrap_or(requested_at)
    }

    fn run_sync_attempt(&self, request_serial: u64) -> Result<()> {
        let result = self.sync.sync();
        self.finish_sync_attempt(request_serial, &result);
        result
    }

    fn finish_sync_attempt(&self, request_serial: u64, result: &Result<()>) {
        let mut state = self.state.inner.lock().unwrap();
        if state.request_serial == request_serial {
            match result {
                Ok(()) => {
                    state.requested_at = None;
                    state.last_error = None;
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        state.failed_request_serial = None;
                    }
                }
                Err(err) => {
                    state.last_error = Some(err.to_string());
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        state.failed_request_serial = Some(request_serial);
                    }
                }
            }
        }
        drop(state);
        self.notify_waiters();
    }

    fn notify_waiters(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.state.wake.notify_all();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl DebouncedSyncScheduler {
    pub fn start_background(&self) -> Result<DebouncedSyncBackgroundHandle> {
        {
            let mut state = self.state.inner.lock().unwrap();
            if state.background_running {
                return Err(crate::Error::Message(
                    "background scheduler already running".to_string(),
                ));
            }
            state.background_running = true;
            state.shutdown_requested = false;
        }

        let scheduler = self.clone();
        let join = match thread::Builder::new()
            .name("wanix-syncfs-scheduler".to_string())
            .spawn(move || scheduler.run_background())
        {
            Ok(join) => join,
            Err(err) => {
                let mut state = self.state.inner.lock().unwrap();
                state.background_running = false;
                state.shutdown_requested = false;
                return Err(crate::Error::from(err));
            }
        };

        Ok(DebouncedSyncBackgroundHandle {
            scheduler: self.clone(),
            join: Some(join),
        })
    }

    fn run_background(&self) {
        loop {
            let request_serial = {
                let mut state = self.state.inner.lock().unwrap();
                loop {
                    if state.shutdown_requested {
                        state.background_running = false;
                        state.shutdown_requested = false;
                        self.state.wake.notify_all();
                        return;
                    }

                    let Some(requested_at) = state.requested_at else {
                        state = self.state.wake.wait(state).unwrap();
                        continue;
                    };

                    if state.failed_request_serial == Some(state.request_serial) {
                        state = self.state.wake.wait(state).unwrap();
                        continue;
                    }

                    let delay = self
                        .due_at(requested_at)
                        .duration_since(SystemTime::now())
                        .unwrap_or(Duration::ZERO);
                    if delay.is_zero() {
                        break state.request_serial;
                    }

                    let (next_state, _) = self.state.wake.wait_timeout(state, delay).unwrap();
                    state = next_state;
                    // Recompute the due time after every wake. A new request may have arrived
                    // while the worker was reacquiring the lock after a timeout.
                }
            };

            let result = self.sync.sync();
            self.finish_sync_attempt(request_serial, &result);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl DebouncedSyncBackgroundHandle {
    pub fn shutdown(mut self) -> thread::Result<()> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> thread::Result<()> {
        {
            let mut state = self.scheduler.state.inner.lock().unwrap();
            state.shutdown_requested = true;
        }
        self.scheduler.state.wake.notify_all();

        if let Some(join) = self.join.take() {
            join.join()
        } else {
            Ok(())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for DebouncedSyncBackgroundHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
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

fn path_contains(ancestor: &str, descendant: &str) -> bool {
    let ancestor = clean_path(ancestor);
    let descendant = clean_path(descendant);
    descendant == ancestor || descendant.strip_prefix(&(ancestor.clone() + "/")).is_some()
}

fn paths_overlap(left: &str, right: &str) -> bool {
    path_contains(left, right) || path_contains(right, left)
}

fn conflicting_dirty_paths(
    dirty: &BTreeMap<String, DirtyEntry>,
    path: &str,
    action: PullPathAction,
    local: &dyn FileSystem,
) -> Result<Vec<String>> {
    let path = clean_path(path);
    let conflicts = match action {
        PullPathAction::DeleteLocal => dirty
            .keys()
            .filter(|dirty_path| paths_overlap(dirty_path, &path))
            .cloned()
            .collect(),
        PullPathAction::SyncRemoteLeaf => dirty
            .keys()
            .filter(|dirty_path| paths_overlap(dirty_path, &path))
            .cloned()
            .collect(),
        PullPathAction::SyncRemoteDir => {
            if exists(local, &path)? && lstat(local, &path)?.is_dir() {
                dirty
                    .keys()
                    .filter(|dirty_path| path_contains(dirty_path, &path))
                    .cloned()
                    .collect()
            } else {
                dirty
                    .keys()
                    .filter(|dirty_path| paths_overlap(dirty_path, &path))
                    .cloned()
                    .collect()
            }
        }
    };
    Ok(conflicts)
}

fn record_conflicts(
    retained_conflicts: &mut BTreeMap<String, DirtyChange>,
    dirty: &BTreeMap<String, DirtyEntry>,
    conflicts: &[String],
) {
    for path in conflicts {
        if let Some(entry) = dirty.get(path) {
            retained_conflicts.insert(path.clone(), entry.change);
        }
    }
}

fn clear_dirty_subtree(dirty: &mut BTreeMap<String, DirtyEntry>, path: &str) {
    let to_remove: Vec<_> = dirty
        .keys()
        .filter(|dirty_path| path_contains(path, dirty_path))
        .cloned()
        .collect();
    for path in to_remove {
        dirty.remove(&path);
    }
}

fn clear_dirty_paths_for_remote_sync(
    dirty: &mut BTreeMap<String, DirtyEntry>,
    path: &str,
    action: PullPathAction,
) {
    match action {
        PullPathAction::DeleteLocal | PullPathAction::SyncRemoteLeaf => {
            clear_dirty_subtree(dirty, path)
        }
        PullPathAction::SyncRemoteDir => {
            dirty.remove(path);
        }
    }
}

fn pull_path_action(
    remote: &dyn FileSystem,
    local: &dyn FileSystem,
    path: &str,
) -> Result<PullPathAction> {
    let remote_meta = lstat(remote, path)?;
    if remote_meta.is_dir() {
        return Ok(PullPathAction::SyncRemoteDir);
    }
    if remote_meta.mode.is_symlink() {
        return Ok(PullPathAction::SyncRemoteLeaf);
    }
    if exists(local, path)? && lstat(local, path)?.is_dir() {
        return Ok(PullPathAction::SyncRemoteLeaf);
    }
    Ok(PullPathAction::SyncRemoteLeaf)
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
    use std::collections::BTreeMap;
    use std::io::{Cursor, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};
    #[cfg(not(target_arch = "wasm32"))]
    use std::{
        sync::{Condvar, Mutex},
        thread,
        time::Instant,
    };

    use super::{DirtyChange, PullConflictPolicy, RemoteSyncBackend, SyncFs};
    use crate::{
        clean_path, fs_ref, parent_path, read_dir, read_file, write_file, Error, FileMode,
        FileSystem, MemFs, Result, TarFs,
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

    #[derive(Default)]
    struct FailOnceRemoteSync {
        inner: MemoryRemoteSync,
        remaining_patch_failures: AtomicUsize,
    }

    impl FailOnceRemoteSync {
        fn new() -> Self {
            Self {
                inner: MemoryRemoteSync::default(),
                remaining_patch_failures: AtomicUsize::new(1),
            }
        }

        fn fs(&self) -> MemFs {
            self.inner.fs()
        }
    }

    impl RemoteSyncBackend for FailOnceRemoteSync {
        fn index(&self) -> Result<crate::FsRef> {
            self.inner.index()
        }

        fn apply_patch(&self, patch: &[u8]) -> Result<()> {
            if self
                .remaining_patch_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(Error::Message("injected patch failure".to_string()));
            }
            self.inner.apply_patch(patch)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Default)]
    struct PatchAttemptRecorder {
        attempts: Mutex<Vec<Instant>>,
        ready: Condvar,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl PatchAttemptRecorder {
        fn record(&self) {
            let mut attempts = self.attempts.lock().unwrap();
            attempts.push(Instant::now());
            self.ready.notify_all();
        }

        fn wait_for_count(&self, expected: usize, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            let mut attempts = self.attempts.lock().unwrap();
            while attempts.len() < expected {
                let now = Instant::now();
                if now >= deadline {
                    return false;
                }
                let (next_attempts, result) =
                    self.ready.wait_timeout(attempts, deadline - now).unwrap();
                attempts = next_attempts;
                if result.timed_out() && attempts.len() < expected {
                    return false;
                }
            }
            true
        }

        fn count(&self) -> usize {
            self.attempts.lock().unwrap().len()
        }

        fn nth(&self, index: usize) -> Instant {
            self.attempts.lock().unwrap()[index]
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(1));
        }
        predicate()
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct ObservedMemoryRemoteSync {
        inner: MemoryRemoteSync,
        attempts: Arc<PatchAttemptRecorder>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl ObservedMemoryRemoteSync {
        fn new() -> Self {
            Self {
                inner: MemoryRemoteSync::default(),
                attempts: Arc::new(PatchAttemptRecorder::default()),
            }
        }

        fn fs(&self) -> MemFs {
            self.inner.fs()
        }

        fn attempts(&self) -> Arc<PatchAttemptRecorder> {
            self.attempts.clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl RemoteSyncBackend for ObservedMemoryRemoteSync {
        fn index(&self) -> Result<crate::FsRef> {
            self.inner.index()
        }

        fn apply_patch(&self, patch: &[u8]) -> Result<()> {
            self.attempts.record();
            self.inner.apply_patch(patch)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct ObservedFailOnceRemoteSync {
        inner: FailOnceRemoteSync,
        attempts: Arc<PatchAttemptRecorder>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl ObservedFailOnceRemoteSync {
        fn new() -> Self {
            Self {
                inner: FailOnceRemoteSync::new(),
                attempts: Arc::new(PatchAttemptRecorder::default()),
            }
        }

        fn fs(&self) -> MemFs {
            self.inner.fs()
        }

        fn attempts(&self) -> Arc<PatchAttemptRecorder> {
            self.attempts.clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl RemoteSyncBackend for ObservedFailOnceRemoteSync {
        fn index(&self) -> Result<crate::FsRef> {
            self.inner.index()
        }

        fn apply_patch(&self, patch: &[u8]) -> Result<()> {
            self.attempts.record();
            self.inner.apply_patch(patch)
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
        assert!(sync.pull_conflicts().is_empty());
    }

    #[test]
    fn syncfs_pull_prefer_remote_overwrites_dirty_local_paths() {
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

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone())
            .with_pull_conflict_policy(PullConflictPolicy::PreferRemote);
        let mut file = sync
            .open_file(
                "notes.txt",
                crate::OpenFlags::WRONLY,
                FileMode::from_perm(0o644),
            )
            .unwrap();
        file.write_at(b"LOCAL", 0).unwrap();
        file.close().unwrap();

        sync.pull().unwrap();

        assert_eq!(read_file(&local, "notes.txt").unwrap(), b"remote");
        assert!(sync.dirty().is_empty());
        assert!(sync.pull_conflicts().is_empty());
    }

    #[test]
    fn syncfs_pull_retains_conflicts_for_dirty_descendants() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        local.mkdir("docs", FileMode::from_perm(0o755)).unwrap();
        write_file(
            &local,
            "docs/readme.txt",
            b"local",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone())
            .with_pull_conflicts_keep_local()
            .with_pull_conflicts_retained(true);
        let mut file = sync
            .open_file(
                "docs/readme.txt",
                crate::OpenFlags::WRONLY,
                FileMode::from_perm(0o644),
            )
            .unwrap();
        file.write_at(b"LOCAL", 0).unwrap();
        file.close().unwrap();

        sync.pull().unwrap();

        assert_eq!(read_file(&local, "docs/readme.txt").unwrap(), b"LOCAL");
        assert_eq!(
            sync.pull_conflicts(),
            BTreeMap::from([("docs/readme.txt".to_string(), DirtyChange::Upsert)])
        );
        assert_eq!(sync.dirty()["docs/readme.txt"], DirtyChange::Upsert);

        sync.clear_pull_conflicts();
        assert!(sync.pull_conflicts().is_empty());
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

    #[test]
    fn syncfs_scheduler_debounces_dirty_sync() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let scheduler = sync.scheduler(Duration::from_secs(2));
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        write_file(&sync, "notes.txt", b"local", FileMode::from_perm(0o644)).unwrap();
        scheduler.request(start);

        assert!(scheduler.pending());
        assert_eq!(
            scheduler.snapshot().due_at,
            Some(start + Duration::from_secs(2))
        );
        assert!(!scheduler.run_due(start + Duration::from_secs(1)).unwrap());
        assert_eq!(remote.patch_calls.load(Ordering::SeqCst), 0);

        assert!(scheduler.run_due(start + Duration::from_secs(2)).unwrap());

        assert!(!scheduler.pending());
        assert_eq!(read_file(&remote.fs(), "notes.txt").unwrap(), b"local");
        assert_eq!(remote.patch_calls.load(Ordering::SeqCst), 1);
        assert!(scheduler.snapshot().last_error.is_none());
    }

    #[test]
    fn syncfs_scheduler_flushes_before_due() {
        let local = MemFs::new();
        let remote = Arc::new(MemoryRemoteSync::default());
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let scheduler = sync.scheduler(Duration::from_secs(60));
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(20);

        write_file(&sync, "early.txt", b"now", FileMode::from_perm(0o644)).unwrap();
        scheduler.request(start);
        scheduler.flush().unwrap();

        assert!(!scheduler.pending());
        assert_eq!(read_file(&remote.fs(), "early.txt").unwrap(), b"now");
        assert_eq!(remote.patch_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn syncfs_scheduler_keeps_pending_after_failed_sync() {
        let local = MemFs::new();
        let remote = Arc::new(FailOnceRemoteSync::new());
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let scheduler = sync.scheduler(Duration::ZERO);
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(30);

        write_file(&sync, "retry.txt", b"later", FileMode::from_perm(0o644)).unwrap();
        scheduler.request(start);

        let err = scheduler.run_due(start).unwrap_err();
        assert!(err.to_string().contains("injected patch failure"));
        assert!(scheduler.pending());
        assert_eq!(
            scheduler.snapshot().last_error.as_deref(),
            Some("injected patch failure")
        );

        assert!(scheduler.run_due(start + Duration::from_secs(1)).unwrap());
        assert!(!scheduler.pending());
        assert_eq!(read_file(&remote.fs(), "retry.txt").unwrap(), b"later");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn syncfs_scheduler_background_debounces_requests() {
        let local = MemFs::new();
        let remote = Arc::new(ObservedMemoryRemoteSync::new());
        let attempts = remote.attempts();
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let scheduler = sync.scheduler(Duration::from_millis(80));
        let handle = scheduler.start_background().unwrap();

        write_file(&sync, "notes.txt", b"local", FileMode::from_perm(0o644)).unwrap();

        scheduler.request(SystemTime::now());
        thread::sleep(Duration::from_millis(30));
        let second_request_at = Instant::now();
        scheduler.request(SystemTime::now());

        assert!(attempts.wait_for_count(1, Duration::from_millis(400)));
        assert!(wait_until(Duration::from_millis(200), || !scheduler.pending()));
        assert_eq!(attempts.count(), 1);
        assert!(attempts.nth(0).duration_since(second_request_at) >= Duration::from_millis(70));
        assert_eq!(read_file(&remote.fs(), "notes.txt").unwrap(), b"local");

        handle.shutdown().unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn syncfs_scheduler_background_shutdown_stops_pending_work() {
        let local = MemFs::new();
        let remote = Arc::new(ObservedMemoryRemoteSync::new());
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let scheduler = sync.scheduler(Duration::from_secs(60));
        let handle = scheduler.start_background().unwrap();

        write_file(&sync, "notes.txt", b"local", FileMode::from_perm(0o644)).unwrap();
        scheduler.request(SystemTime::now());

        handle.shutdown().unwrap();

        assert!(scheduler.pending());
        assert_eq!(remote.inner.patch_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn syncfs_scheduler_background_waits_for_new_request_after_failure() {
        let local = MemFs::new();
        let remote = Arc::new(ObservedFailOnceRemoteSync::new());
        let attempts = remote.attempts();
        let sync = SyncFs::new(fs_ref(local.clone()), remote.clone());
        let scheduler = sync.scheduler(Duration::ZERO);
        let handle = scheduler.start_background().unwrap();

        write_file(&sync, "retry.txt", b"later", FileMode::from_perm(0o644)).unwrap();
        scheduler.request(SystemTime::now());

        assert!(attempts.wait_for_count(1, Duration::from_millis(300)));
        assert!(wait_until(Duration::from_millis(200), || scheduler
            .snapshot()
            .last_error
            .as_deref()
            == Some("injected patch failure")));
        assert!(scheduler.pending());
        assert!(!attempts.wait_for_count(2, Duration::from_millis(60)));

        scheduler.request(SystemTime::now());

        assert!(attempts.wait_for_count(2, Duration::from_millis(300)));
        assert!(wait_until(Duration::from_millis(200), || !scheduler.pending()));
        assert!(!scheduler.pending());
        assert_eq!(read_file(&remote.fs(), "retry.txt").unwrap(), b"later");

        handle.shutdown().unwrap();
    }
}
