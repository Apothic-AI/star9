//! Wanix task/resource filesystem.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use wanix_core::{
    base_name, clean_path, valid_path, DirEntry, Error, ErrorKind, FileMode, FsContext, Metadata,
    Result,
};
use wanix_fs::{
    directory_file, fs_ref, read_file, BoxFile, ControlFile, FileHandle, FileSystem, MapFs, Node,
};
use wanix_vfs::{BindMode, Namespace};

pub trait TaskDriver: Send + Sync {
    fn check(&self, _task: &Task) -> bool {
        false
    }

    fn start(&self, _task: &Task) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct FnDriver {
    check: TaskCheck,
    start: TaskStart,
}

type TaskCheck = Arc<dyn Fn(&Task) -> bool + Send + Sync>;
type TaskStart = Arc<dyn Fn(&Task) -> Result<()> + Send + Sync>;

impl FnDriver {
    pub fn new(
        check: impl Fn(&Task) -> bool + Send + Sync + 'static,
        start: impl Fn(&Task) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            check: Arc::new(check),
            start: Arc::new(start),
        }
    }
}

impl TaskDriver for FnDriver {
    fn check(&self, task: &Task) -> bool {
        (self.check)(task)
    }

    fn start(&self, task: &Task) -> Result<()> {
        (self.start)(task)
    }
}

type DriverRef = Arc<dyn TaskDriver>;

#[derive(Clone)]
pub struct Task {
    inner: Arc<TaskInner>,
}

struct TaskInner {
    fsys: TaskFs,
    driver: DriverRef,
    parent: Mutex<Option<Task>>,
    ns: Arc<Namespace>,
    id: u32,
    state: Mutex<TaskState>,
    fds: Mutex<FdTable>,
    worker: Mutex<Option<String>>,
}

#[derive(Clone, Default)]
struct TaskState {
    alias: String,
    kind: String,
    cmd: String,
    env: Vec<String>,
    exit: String,
    dir: String,
}

#[derive(Default)]
struct FdTable {
    files: BTreeMap<u32, OpenFile>,
    next: u32,
}

struct OpenFile {
    file: BoxFile,
    path: String,
}

impl Task {
    fn new(fsys: TaskFs, id: u32, kind: String, driver: DriverRef, parent: Option<Task>) -> Self {
        let ns = if let Some(parent) = &parent {
            Arc::new(parent.namespace().clone_namespace())
        } else {
            Arc::new(Namespace::new())
        };
        Self {
            inner: Arc::new(TaskInner {
                fsys,
                driver,
                parent: Mutex::new(parent),
                ns,
                id,
                state: Mutex::new(TaskState {
                    kind,
                    dir: ".".to_string(),
                    ..TaskState::default()
                }),
                fds: Mutex::new(FdTable {
                    files: BTreeMap::new(),
                    next: 3,
                }),
                worker: Mutex::new(None),
            }),
        }
    }

    pub fn id(&self) -> String {
        self.inner.id.to_string()
    }

    pub fn namespace(&self) -> Arc<Namespace> {
        self.inner.ns.clone()
    }

    pub fn fsys(&self) -> TaskFs {
        self.inner.fsys.clone()
    }

    pub fn parent(&self) -> Option<Task> {
        self.inner.parent.lock().unwrap().clone()
    }

    pub fn kind(&self) -> String {
        self.inner.state.lock().unwrap().kind.clone()
    }

    pub fn cmd(&self) -> String {
        self.inner.state.lock().unwrap().cmd.clone()
    }

    pub fn arg(&self, index: usize) -> String {
        self.cmd()
            .split(' ')
            .nth(index)
            .unwrap_or_default()
            .to_string()
    }

    pub fn env(&self) -> Vec<String> {
        self.inner.state.lock().unwrap().env.clone()
    }

    pub fn exit(&self) -> String {
        self.inner.state.lock().unwrap().exit.clone()
    }

    pub fn alias(&self) -> String {
        self.inner.state.lock().unwrap().alias.clone()
    }

    pub fn dir(&self) -> String {
        self.inner.state.lock().unwrap().dir.clone()
    }

    pub fn set_worker(&self, worker: impl Into<String>) {
        *self.inner.worker.lock().unwrap() = Some(worker.into());
    }

    pub fn worker(&self) -> Option<String> {
        self.inner.worker.lock().unwrap().clone()
    }

    pub fn set_cmd(&self, cmd: impl Into<String>) {
        self.inner.state.lock().unwrap().cmd = cmd.into();
    }

    pub fn set_env(&self, env: impl IntoIterator<Item = impl Into<String>>) {
        self.inner.state.lock().unwrap().env = env.into_iter().map(Into::into).collect();
    }

    pub fn set_dir(&self, dir: impl Into<String>) {
        self.inner.state.lock().unwrap().dir = dir.into();
    }

    pub fn set_exit(&self, exit: impl Into<String>) {
        self.inner.state.lock().unwrap().exit = exit.into();
    }

    pub fn bind(&self, src_path: &str, dst_path: &str) -> Result<()> {
        self.namespace().bind(
            fs_ref((*self.namespace()).clone()),
            src_path,
            dst_path,
            BindMode::After,
        )
    }

    pub fn unbind(&self, src_path: &str, dst_path: &str) -> Result<()> {
        let ns_ref = fs_ref((*self.namespace()).clone());
        self.namespace().unbind(&ns_ref, src_path, dst_path)
    }

    pub fn register(&self, kind: impl Into<String>, driver: impl TaskDriver + 'static) {
        self.inner.fsys.register(kind, driver);
    }

    pub fn start(&self) -> Result<()> {
        self.inner.driver.start(self)
    }

    pub fn lookup(&self, rid: &str) -> Result<Task> {
        self.inner.fsys.lookup(rid)
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.inner.fsys.tasks()
    }

    pub fn open_fd(&self, file: BoxFile, path: impl Into<String>) -> u32 {
        let mut fds = self.inner.fds.lock().unwrap();
        let fd = fds.next;
        fds.next += 1;
        fds.files.insert(
            fd,
            OpenFile {
                file,
                path: path.into(),
            },
        );
        fd
    }

    pub fn set_fd(&self, fd: u32, file: BoxFile, path: impl Into<String>) {
        let mut fds = self.inner.fds.lock().unwrap();
        if fd >= fds.next {
            fds.next = fd + 1;
        }
        fds.files.insert(
            fd,
            OpenFile {
                file,
                path: path.into(),
            },
        );
    }

    pub fn fd_entries(&self) -> Vec<(u32, String)> {
        self.inner
            .fds
            .lock()
            .unwrap()
            .files
            .iter()
            .map(|(fd, file)| (*fd, file.path.clone()))
            .collect()
    }

    pub fn close_fd(&self, fd: u32) -> Result<()> {
        let mut fds = self.inner.fds.lock().unwrap();
        let mut file = fds
            .files
            .remove(&fd)
            .ok_or_else(|| Error::path("closefd", fd.to_string(), ErrorKind::Invalid))?;
        file.file.close()
    }

    pub fn renumber_fd(&self, from: u32, to: u32) -> Result<()> {
        let mut fds = self.inner.fds.lock().unwrap();
        if from == to {
            return fds
                .files
                .contains_key(&from)
                .then_some(())
                .ok_or_else(|| Error::path("renumberfd", from.to_string(), ErrorKind::Invalid));
        }
        if !fds.files.contains_key(&from) {
            return Err(Error::path(
                "renumberfd",
                from.to_string(),
                ErrorKind::Invalid,
            ));
        }
        {
            let target = fds
                .files
                .get_mut(&to)
                .ok_or_else(|| Error::path("renumberfd", to.to_string(), ErrorKind::Invalid))?;
            target.file.close()?;
        }
        let source = fds.files.remove(&from).expect("source fd checked above");
        fds.files.insert(to, source);
        Ok(())
    }

    pub fn fd_path(&self, fd: u32) -> Result<String> {
        let fds = self.inner.fds.lock().unwrap();
        fds.files
            .get(&fd)
            .map(|file| file.path.clone())
            .ok_or_else(|| Error::path("fd", fd.to_string(), ErrorKind::Invalid))
    }

    pub fn with_fd_mut<T>(
        &self,
        fd: u32,
        f: impl FnOnce(&mut dyn FileHandle) -> Result<T>,
    ) -> Result<T> {
        let mut fds = self.inner.fds.lock().unwrap();
        let file = fds
            .files
            .get_mut(&fd)
            .ok_or_else(|| Error::path("fd", fd.to_string(), ErrorKind::Invalid))?;
        f(file.file.as_mut())
    }

    fn set_field(&self, name: &str, value: String) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        match name {
            "cmd" => state.cmd = value,
            "alias" => {
                let old = std::mem::replace(&mut state.alias, value.clone());
                drop(state);
                self.inner.fsys.update_alias(self, old, value);
                return Ok(());
            }
            "env" => state.env = value.lines().map(ToOwned::to_owned).collect(),
            "dir" => state.dir = value,
            "exit" => state.exit = value,
            _ => return Err(ErrorKind::Invalid.into()),
        }
        Ok(())
    }

    fn field_value(&self, name: &str) -> Result<String> {
        let state = self.inner.state.lock().unwrap();
        let value = match name {
            "id" => self.id(),
            "kind" => state.kind.clone(),
            "cmd" => state.cmd.clone(),
            "alias" => state.alias.clone(),
            "env" => state.env.join("\n"),
            "dir" => state.dir.clone(),
            "exit" => state.exit.clone(),
            _ => return Err(ErrorKind::Invalid.into()),
        };
        Ok(value)
    }
}

impl FileSystem for Task {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if name == "." {
            let fields = [
                "ctl", "id", "kind", "cmd", "alias", "env", "dir", "exit", "fd", "ns",
            ];
            let entries = fields
                .into_iter()
                .map(|field| {
                    let meta = if field == "fd" || field == "ns" {
                        Metadata::dir(field, 0o555)
                    } else {
                        Metadata::file(field, 0o666, 0)
                    };
                    DirEntry::new(field, meta)
                })
                .collect();
            return Ok(directory_file(Metadata::dir(".", 0o555), entries));
        }
        if name == "ns" || name.starts_with("ns/") {
            let rel = name
                .strip_prefix("ns/")
                .map(clean_path)
                .unwrap_or_else(|| ".".to_string());
            return self.namespace().open(ctx, &rel);
        }
        if name == "fd" {
            let entries = self
                .inner
                .fds
                .lock()
                .unwrap()
                .files
                .keys()
                .map(|fd| DirEntry::new(fd.to_string(), Metadata::file(fd.to_string(), 0o666, 0)))
                .collect();
            return Ok(directory_file(Metadata::dir("fd", 0o555), entries));
        }
        if let Some(fd_name) = name.strip_prefix("fd/") {
            let fd = fd_name
                .parse::<u32>()
                .map_err(|_| Error::path("open", name.clone(), ErrorKind::Invalid))?;
            return Ok(Box::new(FdProxy {
                task: self.clone(),
                fd,
            }));
        }
        if name == "ctl" {
            let task = self.clone();
            return ControlFile::new("ctl", move |cmd| {
                let args: Vec<_> = cmd.split_whitespace().collect();
                match args.as_slice() {
                    ["bind", src, dst] => task.bind(src, dst),
                    ["unbind", src, dst] => task.unbind(src, dst),
                    ["start"] => task.start(),
                    _ => Err(ErrorKind::Invalid.into()),
                }
            })
            .open(ctx, ".");
        }
        if matches!(
            name.as_str(),
            "id" | "kind" | "cmd" | "alias" | "env" | "dir" | "exit"
        ) {
            return Ok(Box::new(TaskFieldHandle {
                task: self.clone(),
                name: name.clone(),
                data: format!("{}\n", self.field_value(&name)?).into_bytes(),
                written: Vec::new(),
                offset: 0,
            }));
        }
        Err(Error::path("open", name, ErrorKind::NotFound))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct TaskFieldHandle {
    task: Task,
    name: String,
    data: Vec<u8>,
    written: Vec<u8>,
    offset: u64,
}

impl FileHandle for TaskFieldHandle {
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
        if matches!(self.name.as_str(), "id" | "kind") {
            return Err(ErrorKind::PermissionDenied.into());
        }
        self.written.extend_from_slice(data);
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file(
            self.name.clone(),
            if matches!(self.name.as_str(), "id" | "kind") {
                0o444
            } else {
                0o666
            },
            self.data.len() as u64,
        ))
    }

    fn close(&mut self) -> Result<()> {
        if !self.written.is_empty() {
            self.task.set_field(
                &self.name,
                String::from_utf8_lossy(&self.written).trim().to_string(),
            )?;
        }
        Ok(())
    }
}

struct FdProxy {
    task: Task,
    fd: u32,
}

impl FileHandle for FdProxy {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.task.with_fd_mut(self.fd, |file| file.read(buf))
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.task.with_fd_mut(self.fd, |file| file.write(data))
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.task
            .with_fd_mut(self.fd, |file| file.read_at(buf, offset))
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.task
            .with_fd_mut(self.fd, |file| file.write_at(data, offset))
    }

    fn seek(&mut self, pos: std::io::SeekFrom) -> Result<u64> {
        self.task.with_fd_mut(self.fd, |file| file.seek(pos))
    }

    fn stat(&self) -> Result<Metadata> {
        self.task.with_fd_mut(self.fd, |file| file.stat())
    }

    fn sync(&mut self) -> Result<()> {
        self.task.with_fd_mut(self.fd, |file| file.sync())
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct TaskFs {
    inner: Arc<TaskFsInner>,
}

struct TaskFsInner {
    drivers: RwLock<BTreeMap<String, DriverRef>>,
    resources: RwLock<BTreeMap<String, Task>>,
    aliases: RwLock<BTreeMap<String, Task>>,
    next_id: Mutex<u32>,
}

impl Default for TaskFs {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskFs {
    pub fn new() -> Self {
        let fs = Self {
            inner: Arc::new(TaskFsInner {
                drivers: RwLock::new(BTreeMap::new()),
                resources: RwLock::new(BTreeMap::new()),
                aliases: RwLock::new(BTreeMap::new()),
                next_id: Mutex::new(0),
            }),
        };
        let auto_fs = fs.clone();
        fs.register(
            "auto",
            FnDriver::new(
                |_| false,
                move |task| {
                    let drivers = auto_fs.inner.drivers.read().unwrap().clone();
                    for (kind, driver) in drivers {
                        if kind != "auto" && driver.check(task) {
                            task.inner.state.lock().unwrap().kind = kind;
                            return driver.start(task);
                        }
                    }
                    Ok(())
                },
            ),
        );
        fs
    }

    pub fn register(&self, kind: impl Into<String>, driver: impl TaskDriver + 'static) {
        self.inner
            .drivers
            .write()
            .unwrap()
            .insert(kind.into(), Arc::new(driver));
    }

    pub fn alloc(&self, kind: &str, parent: Option<Task>) -> Result<Task> {
        let driver = self
            .inner
            .drivers
            .read()
            .unwrap()
            .get(kind)
            .cloned()
            .ok_or(ErrorKind::NotFound)?;
        let mut next = self.inner.next_id.lock().unwrap();
        *next += 1;
        let id = *next;
        drop(next);
        let task = Task::new(self.clone(), id, kind.to_string(), driver, parent);
        self.inner
            .resources
            .write()
            .unwrap()
            .insert(id.to_string(), task.clone());
        Ok(task)
    }

    pub fn lookup(&self, rid: &str) -> Result<Task> {
        if let Some(task) = self.inner.resources.read().unwrap().get(rid).cloned() {
            return Ok(task);
        }
        self.inner
            .aliases
            .read()
            .unwrap()
            .get(rid)
            .cloned()
            .ok_or_else(|| Error::path("lookup", rid, ErrorKind::NotFound))
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.inner
            .resources
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    fn update_alias(&self, task: &Task, old: String, new: String) {
        let mut aliases = self.inner.aliases.write().unwrap();
        if !old.is_empty() {
            aliases.remove(&old);
        }
        if !new.is_empty() {
            aliases.insert(new, task.clone());
        }
    }

    fn new_dir_entries(&self) -> Vec<DirEntry> {
        self.inner
            .drivers
            .read()
            .unwrap()
            .keys()
            .map(|kind| DirEntry::new(kind.clone(), Metadata::file(kind.clone(), 0o555, 0)))
            .collect()
    }
}

impl FileSystem for TaskFs {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if name == "." {
            let mut entries = vec![DirEntry::new("new", Metadata::dir("new", 0o555))];
            entries.extend(
                self.inner
                    .resources
                    .read()
                    .unwrap()
                    .keys()
                    .map(|id| DirEntry::new(id.clone(), Metadata::dir(id.clone(), 0o555))),
            );
            entries.extend(
                self.inner
                    .aliases
                    .read()
                    .unwrap()
                    .keys()
                    .map(|alias| DirEntry::new(alias.clone(), Metadata::dir(alias.clone(), 0o555))),
            );
            return Ok(directory_file(Metadata::dir(".", 0o555), entries));
        }
        if name == "new" {
            return Ok(directory_file(
                Metadata::dir("new", 0o555),
                self.new_dir_entries(),
            ));
        }
        if let Some(kind) = name.strip_prefix("new/") {
            let kind = kind.to_string();
            let fs = self.clone();
            return Ok(Box::new(NewTaskHandle {
                fs,
                kind,
                data: None,
                offset: 0,
            }));
        }
        let (head, rest) = name.split_once('/').unwrap_or((name.as_str(), "."));
        let task = self.lookup(head)?;
        task.open(ctx, rest)
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct NewTaskHandle {
    fs: TaskFs,
    kind: String,
    data: Option<Vec<u8>>,
    offset: u64,
}

impl FileHandle for NewTaskHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.data.is_none() {
            let task = self.fs.alloc(&self.kind, None)?;
            self.data = Some(format!("{}\n", task.id()).into_bytes());
        }
        let data = self.data.as_ref().unwrap();
        let start = self.offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file(base_name(&self.kind), 0o555, 0))
    }
}

pub fn task_map(task: &Task) -> MapFs {
    let map = MapFs::new();
    for name in ["id", "kind", "cmd", "alias", "env", "dir", "exit"] {
        if let Ok(value) = read_file(task, name) {
            map.insert(
                name,
                fs_ref(Node::file(name, value, FileMode::from_perm(0o444))),
            );
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{open, read_file, MemFs};

    #[test]
    fn allocates_tasks_and_exposes_fields() {
        let fs = TaskFs::new();
        let task = fs.alloc("auto", None).unwrap();
        assert_eq!(task.id(), "1");
        assert_eq!(read_file(&task, "id").unwrap(), b"1\n");
        assert!(read_file(&fs, "1/id").is_ok());
    }

    #[test]
    fn alias_updates_taskfs_lookup() {
        let fs = TaskFs::new();
        let task = fs.alloc("auto", None).unwrap();
        let mut alias = open(&task, "alias").unwrap();
        alias.write(b"shell").unwrap();
        alias.close().unwrap();
        assert_eq!(fs.lookup("shell").unwrap().id(), task.id());
    }

    #[test]
    fn fd_lifecycle_routes_to_open_file() {
        let task = TaskFs::new().alloc("auto", None).unwrap();
        let mem = MemFs::from_entries([("file", b"abc".to_vec())]);
        let fd = task.open_fd(wanix_fs::open(&mem, "file").unwrap(), "file");
        let mut buf = [0_u8; 2];
        let n = task.with_fd_mut(fd, |file| file.read(&mut buf)).unwrap();
        assert_eq!(&buf[..n], b"ab");
        task.close_fd(fd).unwrap();
        assert!(task.with_fd_mut(fd, |_| Ok(())).is_err());
    }

    #[test]
    fn renumber_fd_moves_source_into_existing_target() {
        let task = TaskFs::new().alloc("auto", None).unwrap();
        let mem = MemFs::from_entries([
            ("source", b"source".to_vec()),
            ("target", b"target".to_vec()),
        ]);
        let source = task.open_fd(wanix_fs::open(&mem, "source").unwrap(), "source");
        let target = task.open_fd(wanix_fs::open(&mem, "target").unwrap(), "target");

        task.renumber_fd(source, target).unwrap();
        assert!(task.with_fd_mut(source, |_| Ok(())).is_err());
        assert_eq!(task.fd_path(target).unwrap(), "source");

        let mut buf = [0_u8; 8];
        let n = task
            .with_fd_mut(target, |file| file.read(&mut buf))
            .unwrap();
        assert_eq!(&buf[..n], b"source");
    }

    #[test]
    fn explicit_fd_setup_supports_standard_streams() {
        let task = TaskFs::new().alloc("auto", None).unwrap();
        let stdin = wanix_fs::Node::file("stdin", b"input".to_vec(), FileMode::from_perm(0o666));
        task.set_fd(0, stdin.open(&FsContext::new(), ".").unwrap(), "stdin");
        assert_eq!(task.fd_entries(), vec![(0, "stdin".to_string())]);

        let mut buf = [0_u8; 8];
        let n = task.with_fd_mut(0, |file| file.read(&mut buf)).unwrap();
        assert_eq!(&buf[..n], b"input");

        let next = task.open_fd(
            wanix_fs::Node::file("next", Vec::new(), FileMode::from_perm(0o666))
                .open(&FsContext::new(), ".")
                .unwrap(),
            "next",
        );
        assert_eq!(next, 3);
    }

    #[test]
    fn public_task_state_setters_update_fields() {
        let task = TaskFs::new().alloc("auto", None).unwrap();
        task.set_cmd("run --flag");
        task.set_env(["A=1", "B=2"]);
        task.set_dir("work");
        task.set_exit("running");
        assert_eq!(task.cmd(), "run --flag");
        assert_eq!(task.env(), vec!["A=1".to_string(), "B=2".to_string()]);
        assert_eq!(task.dir(), "work");
        assert_eq!(task.exit(), "running");
        assert_eq!(read_file(&task, "cmd").unwrap(), b"run --flag\n");
        assert_eq!(read_file(&task, "exit").unwrap(), b"running\n");
    }

    #[test]
    fn task_namespace_clones_parent_bindings() {
        let fs = TaskFs::new();
        let parent = fs.alloc("auto", None).unwrap();
        let mem = wanix_fs::fs_ref(MemFs::from_entries([("file", b"value".to_vec())]));
        parent
            .namespace()
            .bind(mem, ".", "mnt", BindMode::Replace)
            .unwrap();
        let child = fs.alloc("auto", Some(parent)).unwrap();
        assert_eq!(
            wanix_fs::read_file(child.namespace().as_ref(), "mnt/file").unwrap(),
            b"value"
        );
    }
}
