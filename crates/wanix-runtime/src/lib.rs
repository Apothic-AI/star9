//! Wanix runtime composition.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

mod worker;

use wanix_core::{
    clean_path, valid_path, DirEntry, Error, ErrorKind, FileMode, FsContext, Metadata, Result,
};
use wanix_fs::{
    directory_file, fs_ref, read_file, write_file, BoxFile, ControlFile, FileHandle, FileSystem,
    FsRef, MapFs, MemFs, Node, PipeFs, SignalFs,
};
use wanix_protocol::p9::{LoopbackTransport, NinePClientFs, NinePServer, NinePTransport};
use wanix_task::{Task, TaskFs};
use wanix_vfs::{BindMode, Namespace};

pub use worker::{RuntimeProtocolHost, WorkerHost};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct Runtime {
    root: Task,
    task_fs: TaskFs,
    protocol_host: RuntimeProtocolHost,
}

impl Runtime {
    pub fn new() -> Result<Self> {
        let task_fs = TaskFs::new();
        let root = task_fs.alloc("auto", None)?;
        bind_core(&root, task_fs.clone())?;
        bind_devices(&root)?;
        let protocol_host = RuntimeProtocolHost::new(root.clone(), task_fs.clone());
        Ok(Self {
            root,
            task_fs,
            protocol_host,
        })
    }

    pub fn root(&self) -> Task {
        self.root.clone()
    }

    pub fn task_fs(&self) -> TaskFs {
        self.task_fs.clone()
    }

    pub fn namespace(&self) -> Arc<Namespace> {
        self.root.namespace()
    }

    pub fn protocol_host(&self) -> RuntimeProtocolHost {
        self.protocol_host.clone()
    }

    pub fn handle_runtime_request(
        &self,
        request: wanix_protocol::runtime::RuntimeRequest,
    ) -> Result<wanix_protocol::runtime::RuntimeResponse> {
        self.protocol_host.handle_request(request)
    }

    pub fn export_9p(&self) -> Arc<NinePServer> {
        self.export_task_9p(&self.root.id())
            .expect("root task exists for runtime")
    }

    pub fn export_task_9p(&self, task_id: &str) -> Result<Arc<NinePServer>> {
        let task = self.task_fs.lookup(task_id)?;
        Ok(Arc::new(NinePServer::new(fs_ref(
            (*task.namespace()).clone(),
        ))))
    }

    pub fn import_9p(
        &self,
        dst: &str,
        transport: Arc<dyn NinePTransport>,
        mode: BindMode,
    ) -> Result<NinePClientFs> {
        let client = NinePClientFs::connect(transport)?;
        self.root
            .namespace()
            .bind(fs_ref(client.clone()), ".", dst, mode)?;
        Ok(client)
    }

    pub fn import_9p_loopback(
        &self,
        dst: &str,
        server: Arc<NinePServer>,
        mode: BindMode,
    ) -> Result<NinePClientFs> {
        self.import_9p(dst, Arc::new(LoopbackTransport::new(server)), mode)
    }

    pub fn loopback_9p_client(&self) -> Result<NinePClientFs> {
        NinePClientFs::connect(Arc::new(LoopbackTransport::new(self.export_9p())))
    }
}

pub fn new_root() -> Result<Task> {
    Runtime::new().map(|runtime| runtime.root())
}

fn bind_core(root: &Task, task_fs: TaskFs) -> Result<()> {
    let wanix = MapFs::new();
    wanix.insert(
        "version",
        fs_ref(Node::file(
            "version",
            format!("{VERSION}\n").into_bytes(),
            FileMode::from_perm(0o644),
        )),
    );
    root.namespace()
        .bind(fs_ref(wanix), ".", "#wanix", BindMode::Replace)?;
    root.namespace()
        .bind(fs_ref(task_fs), ".", "#task", BindMode::Replace)?;
    Ok(())
}

fn bind_devices(root: &Task) -> Result<()> {
    let devices: [(&str, FsRef); 10] = [
        (
            "#pipe",
            fs_ref(DeviceAllocator::new("pipe", || fs_ref(PipeFs::new(false)))),
        ),
        (
            "#signal",
            fs_ref(DeviceAllocator::new("signal", || {
                fs_ref(SignalFs::default())
            })),
        ),
        (
            "#ramfs",
            fs_ref(DeviceAllocator::new("ramfs", || fs_ref(MemFs::new()))),
        ),
        ("#term", fs_ref(DeviceAllocator::new("term", terminal_fs))),
        ("#vm", fs_ref(DeviceAllocator::new("vm", vm_fs))),
        ("#worker", fs_ref(DeviceAllocator::new("worker", worker_fs))),
        ("#web", fs_ref(web_fs())),
        ("#js", fs_ref(js_value_fs())),
        (
            "#cache",
            fs_ref(DeviceAllocator::new("cache", || fs_ref(MemFs::new()))),
        ),
        (
            "#download",
            fs_ref(DeviceAllocator::new("download", download_fs)),
        ),
    ];
    for (dst, fs) in devices {
        root.namespace().bind(fs, ".", dst, BindMode::Replace)?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct DeviceAllocator {
    kind: String,
    state: Arc<DeviceAllocatorState>,
}

struct DeviceAllocatorState {
    next_id: Mutex<u32>,
    resources: RwLock<BTreeMap<String, FsRef>>,
    factory: Box<dyn Fn() -> FsRef + Send + Sync>,
}

impl DeviceAllocator {
    pub fn new(
        kind: impl Into<String>,
        factory: impl Fn() -> FsRef + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind: kind.into(),
            state: Arc::new(DeviceAllocatorState {
                next_id: Mutex::new(0),
                resources: RwLock::new(BTreeMap::new()),
                factory: Box::new(factory),
            }),
        }
    }

    pub fn get(&self, id: &str) -> Option<FsRef> {
        self.state.resources.read().unwrap().get(id).cloned()
    }

    pub fn alloc(&self) -> String {
        let mut next = self.state.next_id.lock().unwrap();
        *next += 1;
        let id = next.to_string();
        drop(next);
        let resource = (self.state.factory)();
        self.state
            .resources
            .write()
            .unwrap()
            .insert(id.clone(), resource);
        id
    }
}

impl FileSystem for DeviceAllocator {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if name == "." {
            let mut entries = vec![DirEntry::new("new", Metadata::file("new", 0o555, 0))];
            entries.extend(
                self.state
                    .resources
                    .read()
                    .unwrap()
                    .keys()
                    .map(|id| DirEntry::new(id.clone(), Metadata::dir(id.clone(), 0o555))),
            );
            return Ok(directory_file(Metadata::dir(".", 0o555), entries));
        }
        if name == "new" {
            return Ok(Box::new(NewDeviceHandle {
                allocator: self.clone(),
                data: None,
                offset: 0,
            }));
        }
        let (head, rest) = name.split_once('/').unwrap_or((name.as_str(), "."));
        let resource = self
            .get(head)
            .ok_or_else(|| Error::path("open", head, ErrorKind::NotFound))?;
        resource.open(ctx, rest)
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct NewDeviceHandle {
    allocator: DeviceAllocator,
    data: Option<Vec<u8>>,
    offset: u64,
}

impl FileHandle for NewDeviceHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.data.is_none() {
            self.data = Some(format!("{}\n", self.allocator.alloc()).into_bytes());
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
        Ok(Metadata::file(
            format!("new-{}", self.allocator.kind),
            0o555,
            0,
        ))
    }
}

fn terminal_fs() -> FsRef {
    let map = MapFs::new();
    let pipe = PipeFs::new(false);
    map.insert("data", fs_ref(pipe.clone()));
    map.insert("program", fs_ref(pipe));
    map.insert("winch", fs_ref(SignalFs::default()));
    map.insert("ctl", fs_ref(ControlFile::new("ctl", |_| Ok(()))));
    fs_ref(map)
}

fn vm_fs() -> FsRef {
    let fs = MemFs::from_entries([
        ("ctl", b"".to_vec()),
        ("state", b"created\n".to_vec()),
        ("console", b"".to_vec()),
    ]);
    fs_ref(fs)
}

fn worker_fs() -> FsRef {
    let fs = MemFs::from_entries([
        ("ctl", b"".to_vec()),
        ("kind", b"worker\n".to_vec()),
        ("state", b"created\n".to_vec()),
    ]);
    fs_ref(fs)
}

fn web_fs() -> MemFs {
    MemFs::from_entries([
        ("dom/ctl", b"".to_vec()),
        ("caches/new", b"".to_vec()),
        ("download/ctl", b"".to_vec()),
        ("worker/new", b"".to_vec()),
        ("opfs/new", b"".to_vec()),
    ])
}

fn js_value_fs() -> MemFs {
    MemFs::from_entries([
        ("global", b"[object global]\n".to_vec()),
        ("values", b"".to_vec()),
    ])
}

fn download_fs() -> FsRef {
    let fs = MemFs::from_entries([("ctl", b"".to_vec()), ("files", b"".to_vec())]);
    fs_ref(fs)
}

#[derive(Clone, Debug)]
pub enum ExecutionKind {
    Wasi,
    GoJs,
}

#[derive(Clone)]
pub struct ExecutionAdapter {
    pub kind: ExecutionKind,
    pub command: String,
}

impl ExecutionAdapter {
    pub fn wasi(command: impl Into<String>) -> Self {
        Self {
            kind: ExecutionKind::Wasi,
            command: command.into(),
        }
    }

    pub fn go_js(command: impl Into<String>) -> Self {
        Self {
            kind: ExecutionKind::GoJs,
            command: command.into(),
        }
    }

    pub fn start(&self, task: &Task) -> Result<()> {
        task.set_cmd(self.command.clone());
        task.set_worker(format!("{:?}:{}", self.kind, self.command));
        task.set_exit("running");
        install_standard_fds(task)?;
        task.set_exit("started");
        Ok(())
    }

    pub fn finish(&self, task: &Task, status: i32) {
        task.set_exit(format!("{status}"));
    }
}

fn install_standard_fds(task: &Task) -> Result<()> {
    for (fd, name) in [(0, "stdin"), (1, "stdout"), (2, "stderr")] {
        let node = Node::file(name, Vec::new(), FileMode::from_perm(0o666));
        task.set_fd(fd, node.open(&FsContext::new(), ".")?, name);
    }
    Ok(())
}

pub fn setup_namespace(
    task: &Task,
    bindings: impl IntoIterator<Item = (FsRef, String, String, BindMode)>,
) -> Result<()> {
    for (fs, src, dst, mode) in bindings {
        task.namespace().bind(fs, &src, &dst, mode)?;
    }
    Ok(())
}

pub fn smoke_file_api(runtime: &Runtime) -> Result<Vec<String>> {
    let ns = runtime.namespace();
    write_file(ns.as_ref(), "#ramfs/new", b"", FileMode::from_perm(0o644)).ok();
    let root_listing = wanix_fs::read_dir(ns.as_ref(), ".")?
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    let _ = read_file(ns.as_ref(), "#wanix/version")?;
    Ok(root_listing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{open, read_dir, read_file};

    #[test]
    fn root_binds_wanix_task_and_devices() {
        let runtime = Runtime::new().unwrap();
        assert_eq!(
            read_file(runtime.namespace().as_ref(), "#wanix/version").unwrap(),
            b"0.1.0\n"
        );
        let root_entries: Vec<_> = read_dir(runtime.namespace().as_ref(), ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert!(
            root_entries.is_empty(),
            "hidden device binds should not list at root"
        );
        assert!(read_dir(runtime.namespace().as_ref(), "#task").is_ok());
        assert!(read_dir(runtime.namespace().as_ref(), "#pipe").is_ok());
    }

    #[test]
    fn device_allocators_create_resources() {
        let runtime = Runtime::new().unwrap();
        let id = String::from_utf8(read_file(runtime.namespace().as_ref(), "#pipe/new").unwrap())
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(id, "1");
        let mut writer = open(runtime.namespace().as_ref(), "#pipe/1/data").unwrap();
        writer.write(b"ping").unwrap();
        let mut reader = open(runtime.namespace().as_ref(), "#pipe/1/data1").unwrap();
        let mut buf = [0_u8; 8];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
    }

    #[test]
    fn execution_adapters_are_task_drivers() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        ExecutionAdapter::wasi("repl.wasm").start(&task).unwrap();
        assert_eq!(task.worker(), Some("Wasi:repl.wasm".to_string()));
        assert_eq!(task.cmd(), "repl.wasm");
        assert_eq!(task.exit(), "started");
        assert_eq!(
            task.fd_entries(),
            vec![
                (0, "stdin".to_string()),
                (1, "stdout".to_string()),
                (2, "stderr".to_string())
            ]
        );
        ExecutionAdapter::wasi("repl.wasm").finish(&task, 0);
        assert_eq!(task.exit(), "0");
    }

    #[test]
    fn runtime_exports_and_imports_namespace_over_9p_loopback() {
        let runtime = Runtime::new().unwrap();
        runtime
            .namespace()
            .bind(
                wanix_fs::fs_ref(wanix_fs::MemFs::from_entries([(
                    "file",
                    b"over-9p".to_vec(),
                )])),
                ".",
                "tmp",
                BindMode::Replace,
            )
            .unwrap();

        let server = runtime.export_9p();
        runtime
            .import_9p_loopback("remote", server, BindMode::Replace)
            .unwrap();

        assert_eq!(
            read_file(runtime.namespace().as_ref(), "remote/tmp/file").unwrap(),
            b"over-9p"
        );
    }
}
