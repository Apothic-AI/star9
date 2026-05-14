//! Star 9 runtime composition.

use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::sync::{Arc, RwLock};

mod devices;
mod execution;
mod wasi;
mod worker;

use star9_core::{
    base_name, clean_path, valid_path, DirEntry, Error, ErrorKind, FileMode, FsContext, Metadata,
    Result,
};
use star9_fs::{
    directory_file, fs_ref, read_file, write_file, BoxFile, FileHandle, FileSystem, FsRef, MapFs,
    MemFs, Node,
};
#[cfg(not(target_arch = "wasm32"))]
use star9_protocol::p9::TcpStreamTransport;
use star9_protocol::p9::{LoopbackTransport, NinePClientFs, NinePServer, NinePTransport};
use star9_task::{Task, TaskFs};
use star9_vfs::{BindMode, Namespace};

pub use devices::{DeterministicVmProvider, VmProvider, VmProviderResource, VmProviderUpdate};
#[cfg(not(target_arch = "wasm32"))]
pub use execution::NativePtyExecutionHandler;
pub use execution::{ExecutionRegistry, FnExecutionHandler, NativeExecutionHandler};
pub use wasi::WasmiWasiHandler;
pub use worker::{RuntimeProtocolHost, WorkerHost};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Default)]
pub struct EnvRegistry {
    entries: Arc<RwLock<BTreeMap<String, Vec<u8>>>>,
}

impl EnvRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        self.entries.read().unwrap().clone()
    }

    pub fn replace_all(&self, entries: BTreeMap<String, Vec<u8>>) {
        *self.entries.write().unwrap() = entries;
    }

    fn set(&self, name: String, data: Vec<u8>) {
        self.entries.write().unwrap().insert(name, data);
    }

    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.entries.read().unwrap().get(name).cloned()
    }
}

impl FileSystem for EnvRegistry {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        let name = clean_path(name);
        if name == "." {
            let entries = self
                .entries
                .read()
                .unwrap()
                .iter()
                .map(|(name, data)| {
                    DirEntry::new(name.clone(), Metadata::file(name, 0o666, data.len() as u64))
                })
                .collect();
            return Ok(directory_file(Metadata::dir("env", 0o777), entries));
        }
        let key = env_key(&name)?;
        let data = self
            .get(&key)
            .ok_or_else(|| Error::path("open", &key, ErrorKind::NotFound))?;
        Ok(Box::new(EnvFile {
            registry: self.clone(),
            name: key,
            data,
            offset: 0,
            dirty: false,
        }))
    }

    fn stat(&self, _ctx: &FsContext, name: &str) -> Result<Metadata> {
        let name = clean_path(name);
        if name == "." {
            return Ok(Metadata::dir("env", 0o777));
        }
        let key = env_key(&name)?;
        let len = self
            .get(&key)
            .map(|data| data.len())
            .ok_or_else(|| Error::path("stat", &key, ErrorKind::NotFound))?;
        Ok(Metadata::file(base_name(&key), 0o666, len as u64))
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        let key = env_key(name)?;
        Ok(Box::new(EnvFile {
            registry: self.clone(),
            name: key,
            data: Vec::new(),
            offset: 0,
            dirty: true,
        }))
    }

    fn remove(&self, name: &str) -> Result<()> {
        let key = env_key(name)?;
        if self.entries.write().unwrap().remove(&key).is_some() {
            Ok(())
        } else {
            Err(Error::path("remove", key, ErrorKind::NotFound))
        }
    }

    fn chmod(&self, name: &str, _mode: FileMode) -> Result<()> {
        let key = env_key(name)?;
        if self.get(&key).is_some() {
            Ok(())
        } else {
            Err(Error::path("chmod", key, ErrorKind::NotFound))
        }
    }
}

struct EnvFile {
    registry: EnvRegistry,
    name: String,
    data: Vec<u8>,
    offset: usize,
    dirty: bool,
}

impl FileHandle for EnvFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.offset >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - self.offset);
        buf[..n].copy_from_slice(&self.data[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let end = self.offset + data.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[self.offset..end].copy_from_slice(data);
        self.offset = end;
        self.dirty = true;
        Ok(data.len())
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let next = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::End(offset) => self.data.len() as i64 + offset,
            SeekFrom::Current(offset) => self.offset as i64 + offset,
        };
        if next < 0 {
            return Err(ErrorKind::Invalid.into());
        }
        self.offset = next as usize;
        Ok(self.offset as u64)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file(
            base_name(&self.name),
            0o666,
            self.data.len() as u64,
        ))
    }

    fn close(&mut self) -> Result<()> {
        if self.dirty {
            self.registry.set(self.name.clone(), self.data.clone());
            self.dirty = false;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ServiceEntry {
    fs: FsRef,
    description: String,
}

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    entries: Arc<RwLock<BTreeMap<String, ServiceEntry>>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, name: &str, fs: FsRef, description: impl Into<String>) -> Result<()> {
        let name = service_key(name)?;
        self.entries.write().unwrap().insert(
            name,
            ServiceEntry {
                fs,
                description: description.into(),
            },
        );
        Ok(())
    }

    pub fn unregister(&self, name: &str) -> Result<()> {
        let name = service_key(name)?;
        if self.entries.write().unwrap().remove(&name).is_some() {
            Ok(())
        } else {
            Err(Error::path("srv", name, ErrorKind::NotFound))
        }
    }

    pub fn get(&self, name: &str) -> Result<FsRef> {
        let name = service_key(name)?;
        self.entries
            .read()
            .unwrap()
            .get(&name)
            .map(|entry| entry.fs.clone())
            .ok_or_else(|| Error::path("srv", name, ErrorKind::NotFound))
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.read().unwrap().keys().cloned().collect()
    }
}

impl FileSystem for ServiceRegistry {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        let name = clean_path(name);
        if name == "." {
            let entries = self
                .entries
                .read()
                .unwrap()
                .iter()
                .map(|(service, entry)| {
                    DirEntry::new(
                        service.clone(),
                        Metadata::file(service, 0o444, entry.description.len() as u64),
                    )
                })
                .collect();
            return Ok(directory_file(Metadata::dir("srv", 0o555), entries));
        }
        let key = service_key(&name)?;
        let description = self
            .entries
            .read()
            .unwrap()
            .get(&key)
            .map(|entry| entry.description.clone())
            .ok_or_else(|| Error::path("open", &key, ErrorKind::NotFound))?;
        Node::file(
            base_name(&key),
            description.into_bytes(),
            FileMode::from_perm(0o444),
        )
        .open(ctx, ".")
    }

    fn stat(&self, _ctx: &FsContext, name: &str) -> Result<Metadata> {
        let name = clean_path(name);
        if name == "." {
            return Ok(Metadata::dir("srv", 0o555));
        }
        let key = service_key(&name)?;
        let description_len = self
            .entries
            .read()
            .unwrap()
            .get(&key)
            .map(|entry| entry.description.len())
            .ok_or_else(|| Error::path("stat", &key, ErrorKind::NotFound))?;
        Ok(Metadata::file(
            base_name(&key),
            0o444,
            description_len as u64,
        ))
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.unregister(name)
    }
}

#[derive(Clone)]
pub struct Runtime {
    root: Task,
    task_fs: TaskFs,
    env_registry: EnvRegistry,
    service_registry: ServiceRegistry,
    devices: devices::RuntimeDevices,
    execution_registry: ExecutionRegistry,
    protocol_host: RuntimeProtocolHost,
}

impl Runtime {
    pub fn new() -> Result<Self> {
        let task_fs = TaskFs::new();
        let root = task_fs.alloc("auto", None)?;
        bind_core(&root, task_fs.clone())?;
        let env_registry = EnvRegistry::new();
        let service_registry = ServiceRegistry::new();
        bind_services_and_compatibility_dirs(
            &root,
            env_registry.clone(),
            service_registry.clone(),
        )?;
        let devices = bind_devices(&root)?;
        let execution_registry = ExecutionRegistry::new();
        let protocol_host = RuntimeProtocolHost::new(root.clone(), task_fs.clone());
        Ok(Self {
            root,
            task_fs,
            env_registry,
            service_registry,
            devices,
            execution_registry,
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

    pub fn execution_registry(&self) -> ExecutionRegistry {
        self.execution_registry.clone()
    }

    pub fn env_registry(&self) -> EnvRegistry {
        self.env_registry.clone()
    }

    pub fn service_registry(&self) -> ServiceRegistry {
        self.service_registry.clone()
    }

    pub fn handle_runtime_request(
        &self,
        request: star9_protocol::runtime::RuntimeRequest,
    ) -> Result<star9_protocol::runtime::RuntimeResponse> {
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

    pub fn register_service(
        &self,
        name: &str,
        fs: FsRef,
        description: impl Into<String>,
    ) -> Result<()> {
        self.service_registry.register(name, fs, description)
    }

    pub fn unregister_service(&self, name: &str) -> Result<()> {
        self.service_registry.unregister(name)
    }

    pub fn register_loopback_service(&self, name: &str) -> Result<()> {
        let client = self.loopback_9p_client()?;
        self.register_service(
            name,
            fs_ref(client),
            format!("loopback root namespace export: {}\n", service_key(name)?),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn register_tcp_9p_service(&self, source: &str, name: &str) -> Result<()> {
        let addr = tcp_service_addr(source)?;
        let stream = std::net::TcpStream::connect(&addr)
            .map_err(|err| Error::Message(format!("connect {addr}: {err}")))?;
        let client = NinePClientFs::connect(Arc::new(TcpStreamTransport::new(stream)))?;
        self.register_service(
            name,
            fs_ref(client),
            format!("tcp 9p service {source} as {}\n", service_key(name)?),
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub fn register_tcp_9p_service(&self, source: &str, _name: &str) -> Result<()> {
        Err(Error::path("srv", source, ErrorKind::NotSupported))
    }

    pub fn mount_service(&self, service: &str, dst: &str, mode: BindMode) -> Result<()> {
        let fs = self.service_registry.get(service)?;
        self.root.namespace().bind(fs, ".", dst, mode)
    }

    pub fn set_task_export(&self, task_id: &str, fs: FsRef) -> Result<()> {
        self.task_fs.lookup(task_id)?.set_export(fs);
        Ok(())
    }

    pub fn set_vm_guest(&self, vm_id: &str, fs: FsRef) -> Result<()> {
        self.devices.set_vm_guest(vm_id, fs)
    }

    pub fn set_vm_provider(&self, provider: Arc<dyn VmProvider>) {
        self.devices.set_vm_provider(provider);
    }
}

pub fn new_root() -> Result<Task> {
    Runtime::new().map(|runtime| runtime.root())
}

fn bind_core(root: &Task, task_fs: TaskFs) -> Result<()> {
    let star9 = MapFs::new();
    star9.insert(
        "version",
        fs_ref(Node::file(
            "version",
            format!("{VERSION}\n").into_bytes(),
            FileMode::from_perm(0o644),
        )),
    );
    root.namespace()
        .bind(fs_ref(star9), ".", "#star9", BindMode::Replace)?;
    root.namespace()
        .bind(fs_ref(task_fs), ".", "#task", BindMode::Replace)?;
    Ok(())
}

fn bind_services_and_compatibility_dirs(
    root: &Task,
    env_registry: EnvRegistry,
    service_registry: ServiceRegistry,
) -> Result<()> {
    root.namespace()
        .bind(fs_ref(env_registry.clone()), ".", "#env", BindMode::Replace)?;
    root.namespace()
        .bind(fs_ref(env_registry), ".", "env", BindMode::Replace)?;
    root.namespace().bind(
        fs_ref(service_registry.clone()),
        ".",
        "#srv",
        BindMode::Replace,
    )?;
    root.namespace()
        .bind(fs_ref(service_registry), ".", "srv", BindMode::Replace)?;
    root.namespace()
        .bind(fs_ref(MemFs::new()), ".", "n", BindMode::Replace)?;
    root.namespace()
        .bind(fs_ref(MemFs::new()), ".", "mnt", BindMode::Replace)?;
    Ok(())
}

fn bind_devices(root: &Task) -> Result<devices::RuntimeDevices> {
    devices::bind_devices(root)
}

fn service_key(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let path = trimmed.trim_start_matches('/');
    let key = path
        .strip_prefix("#srv/")
        .or_else(|| path.strip_prefix("srv/"))
        .unwrap_or(path);
    if key.is_empty() || key == "." || key.contains('/') || !valid_path(key) {
        return Err(Error::path("srv", input, ErrorKind::Invalid));
    }
    Ok(key.to_string())
}

fn env_key(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let path = trimmed.trim_start_matches('/');
    let key = path
        .strip_prefix("#env/")
        .or_else(|| path.strip_prefix("env/"))
        .unwrap_or(path);
    if key.is_empty() || key == "." || key.contains('/') || !valid_path(key) {
        return Err(Error::path("env", input, ErrorKind::Invalid));
    }
    Ok(key.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn tcp_service_addr(source: &str) -> Result<String> {
    let Some(rest) = source.strip_prefix("tcp!") else {
        return Err(Error::path("srv", source, ErrorKind::NotSupported));
    };
    let parts = rest.split('!').collect::<Vec<_>>();
    match parts.as_slice() {
        [host, port] if !host.is_empty() && !port.is_empty() => Ok(format!("{host}:{port}")),
        [host] if !host.is_empty() => Err(Error::path("srv", source, ErrorKind::NotSupported)),
        _ => Err(Error::path("srv", source, ErrorKind::Invalid)),
    }
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
    let root_listing = star9_fs::read_dir(ns.as_ref(), ".")?
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    let _ = read_file(ns.as_ref(), "#star9/version")?;
    Ok(root_listing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use star9_fs::{fs_ref, open, read_dir, read_file, FileHandle, MemFs};
    use star9_protocol::runtime::{
        EnvironmentEntry, ExecutionKind as ProtocolExecutionKind, ExecutionSpec, ExitStatus,
        FdDescriptor, FdKind, StdioSet, StreamDescriptor,
    };

    fn read_handle(file: &mut dyn FileHandle) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let mut buf = [0_u8; 64];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                return Ok(data);
            }
            data.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn root_binds_star9_task_and_devices() {
        let runtime = Runtime::new().unwrap();
        assert_eq!(
            read_file(runtime.namespace().as_ref(), "#star9/version").unwrap(),
            b"0.1.0\n"
        );
        let root_entries: Vec<_> = read_dir(runtime.namespace().as_ref(), ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(root_entries, vec!["env", "mnt", "n", "srv"]);
        assert!(read_dir(runtime.namespace().as_ref(), "#task").is_ok());
        assert!(read_dir(runtime.namespace().as_ref(), "#pipe").is_ok());
        assert!(read_dir(runtime.namespace().as_ref(), "#term").is_ok());
        assert!(read_dir(runtime.namespace().as_ref(), "#vm").is_ok());
        assert!(read_dir(runtime.namespace().as_ref(), "#net").is_ok());
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
    fn execution_registry_reports_missing_handler_after_starting_task_state() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();

        let err = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ProtocolExecutionKind::Wasi,
                    module: "missing.wasm".into(),
                    args: vec!["alpha".into()],
                    env: vec![EnvironmentEntry {
                        name: "MODE".into(),
                        value: "test".into(),
                    }],
                    cwd: Some("sandbox".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("operation not supported: no execution handler registered"));
        assert_eq!(task.cmd(), "missing.wasm alpha");
        assert_eq!(task.env(), vec!["MODE=test".to_string()]);
        assert_eq!(task.dir(), "sandbox");
        assert_eq!(task.exit(), "started");
        assert_eq!(
            task.fd_entries(),
            vec![
                (0, "stdin".to_string()),
                (1, "stdout".to_string()),
                (2, "stderr".to_string())
            ]
        );
    }

    #[test]
    fn execution_registry_runs_wasi_handlers_against_task_namespace() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("stdin.txt", b"stdin-bytes".to_vec()),
                    ("stdout.txt", Vec::new()),
                    ("stderr.txt", Vec::new()),
                    ("extra.txt", b"side-channel".to_vec()),
                    ("result.txt", Vec::new()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();

        let registry = runtime.execution_registry();
        registry.register_fn(ProtocolExecutionKind::Wasi, "echo.wasm", |task, _spec| {
            assert_eq!(task.worker(), Some("native:wasi:echo.wasm".to_string()));
            assert_eq!(task.cmd(), "echo.wasm alpha beta");
            assert_eq!(task.arg(0), "echo.wasm");
            assert_eq!(task.arg(1), "alpha");
            assert_eq!(task.arg(2), "beta");
            assert_eq!(
                task.env(),
                vec!["GREETING=hello".to_string(), "MODE=test".to_string()]
            );
            assert_eq!(task.dir(), "workspace");
            assert_eq!(task.fd_path(0).unwrap(), "workspace/stdin.txt");
            assert_eq!(task.fd_path(1).unwrap(), "workspace/stdout.txt");
            assert_eq!(task.fd_path(2).unwrap(), "workspace/stderr.txt");
            assert_eq!(task.fd_path(7).unwrap(), "workspace/extra.txt");

            let stdin = task.with_fd_mut(0, |file| read_handle(file))?;
            let extra = task.with_fd_mut(7, |file| read_handle(file))?;
            task.with_fd_mut(1, |file| {
                file.write(
                    format!("stdout:{}:{}", task.arg(1), String::from_utf8_lossy(&stdin))
                        .as_bytes(),
                )?;
                file.sync()?;
                Ok(())
            })?;
            task.with_fd_mut(2, |file| {
                file.write(
                    format!("stderr:{}:{}", task.dir(), String::from_utf8_lossy(&extra)).as_bytes(),
                )?;
                file.sync()?;
                Ok(())
            })?;
            write_file(
                task.namespace().as_ref(),
                "workspace/result.txt",
                format!("cmd={};env={}", task.cmd(), task.env().join(",")).as_bytes(),
                FileMode::from_perm(0o644),
            )?;
            Ok(ExitStatus::ExitCode(23))
        });

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ProtocolExecutionKind::Wasi,
                    module: "echo.wasm".into(),
                    args: vec!["alpha".into(), "beta".into()],
                    env: vec![
                        EnvironmentEntry {
                            name: "GREETING".into(),
                            value: "hello".into(),
                        },
                        EnvironmentEntry {
                            name: "MODE".into(),
                            value: "test".into(),
                        },
                    ],
                    cwd: Some("workspace".into()),
                    stdio: StdioSet {
                        stdin: StreamDescriptor::Fd(FdDescriptor {
                            fd: 0,
                            kind: FdKind::File,
                            path: Some("workspace/stdin.txt".into()),
                            read: true,
                            write: false,
                            append: false,
                        }),
                        stdout: StreamDescriptor::Fd(FdDescriptor {
                            fd: 1,
                            kind: FdKind::File,
                            path: Some("workspace/stdout.txt".into()),
                            read: false,
                            write: true,
                            append: false,
                        }),
                        stderr: StreamDescriptor::Fd(FdDescriptor {
                            fd: 2,
                            kind: FdKind::File,
                            path: Some("workspace/stderr.txt".into()),
                            read: false,
                            write: true,
                            append: false,
                        }),
                    },
                    fds: vec![FdDescriptor {
                        fd: 7,
                        kind: FdKind::File,
                        path: Some("workspace/extra.txt".into()),
                        read: true,
                        write: false,
                        append: false,
                    }],
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(23));
        assert_eq!(task.exit(), "23");
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/stdout.txt").unwrap(),
            b"stdout:alpha:stdin-bytes"
        );
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/stderr.txt").unwrap(),
            b"stderr:workspace:side-channel"
        );
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/result.txt").unwrap(),
            b"cmd=echo.wasm alpha beta;env=GREETING=hello,MODE=test"
        );
    }

    #[test]
    fn execution_registry_runs_js_wasm_handlers() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("stdout.txt", Vec::new()),
                    ("result.txt", Vec::new()),
                ])),
                ".",
                "sandbox",
                BindMode::Replace,
            )
            .unwrap();

        runtime.execution_registry().register_fn(
            ProtocolExecutionKind::JsWasm,
            "ui.wasm",
            |task, spec| {
                assert_eq!(spec.kind, ProtocolExecutionKind::JsWasm);
                assert_eq!(task.worker(), Some("native:js_wasm:ui.wasm".to_string()));
                assert_eq!(task.cmd(), "ui.wasm --hydrate");
                assert_eq!(task.dir(), "sandbox");
                task.with_fd_mut(1, |file| {
                    file.write(b"console:ready")?;
                    file.sync()?;
                    Ok(())
                })?;
                write_file(
                    task.namespace().as_ref(),
                    "sandbox/result.txt",
                    b"rendered",
                    FileMode::from_perm(0o644),
                )?;
                Ok(ExitStatus::ExitCode(0))
            },
        );

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ProtocolExecutionKind::JsWasm,
                    module: "ui.wasm".into(),
                    args: vec!["--hydrate".into()],
                    env: Vec::new(),
                    cwd: Some("sandbox".into()),
                    stdio: StdioSet {
                        stdin: StreamDescriptor::Null,
                        stdout: StreamDescriptor::Fd(FdDescriptor {
                            fd: 1,
                            kind: FdKind::File,
                            path: Some("sandbox/stdout.txt".into()),
                            read: false,
                            write: true,
                            append: false,
                        }),
                        stderr: StreamDescriptor::Null,
                    },
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(task.exit(), "0");
        assert_eq!(
            read_file(task.namespace().as_ref(), "sandbox/stdout.txt").unwrap(),
            b"console:ready"
        );
        assert_eq!(
            read_file(task.namespace().as_ref(), "sandbox/result.txt").unwrap(),
            b"rendered"
        );
    }

    #[test]
    fn runtime_exports_and_imports_namespace_over_9p_loopback() {
        let runtime = Runtime::new().unwrap();
        runtime
            .namespace()
            .bind(
                star9_fs::fs_ref(star9_fs::MemFs::from_entries([(
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

    #[test]
    fn runtime_installs_service_registry_and_compatibility_dirs() {
        let runtime = Runtime::new().unwrap();
        let ns = runtime.namespace();
        let root_entries = read_dir(ns.as_ref(), ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert!(
            root_entries.contains(&"srv".to_string()),
            "{root_entries:?}"
        );
        assert!(
            root_entries.contains(&"env".to_string()),
            "{root_entries:?}"
        );
        assert!(root_entries.contains(&"n".to_string()), "{root_entries:?}");
        assert!(
            root_entries.contains(&"mnt".to_string()),
            "{root_entries:?}"
        );

        runtime.register_loopback_service("rootfs").unwrap();
        let services = read_dir(ns.as_ref(), "#srv")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert_eq!(services, vec!["rootfs"]);
        assert!(
            String::from_utf8_lossy(&read_file(ns.as_ref(), "srv/rootfs").unwrap())
                .contains("loopback root namespace export")
        );
    }

    #[test]
    fn runtime_installs_env_registry() {
        let runtime = Runtime::new().unwrap();
        runtime
            .env_registry()
            .replace_all(BTreeMap::from([("name".to_string(), b"one\0two".to_vec())]));
        assert_eq!(
            read_file(runtime.namespace().as_ref(), "#env/name").unwrap(),
            b"one\0two"
        );
        write_file(
            runtime.namespace().as_ref(),
            "env/color",
            b"blue",
            FileMode::from_perm(0o666),
        )
        .unwrap();
        assert_eq!(
            runtime.env_registry().snapshot().get("color").cloned(),
            Some(b"blue".to_vec())
        );
    }

    #[test]
    fn runtime_mounts_registered_services() {
        let runtime = Runtime::new().unwrap();
        runtime
            .namespace()
            .bind(
                fs_ref(MemFs::from_entries([(
                    "tmp-service-file",
                    b"served".to_vec(),
                )])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();

        runtime.register_loopback_service("rootfs").unwrap();
        runtime
            .mount_service("rootfs", "n/rootfs", BindMode::Replace)
            .unwrap();

        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                "n/rootfs/workspace/tmp-service-file"
            )
            .unwrap(),
            b"served"
        );
    }

    #[test]
    fn runtime_installs_task_exports_and_vm_guests() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        runtime
            .set_task_export(
                &task.id(),
                fs_ref(MemFs::from_entries([(
                    "exported.txt",
                    b"task-export".to_vec(),
                )])),
            )
            .unwrap();
        assert_eq!(
            read_file(task.namespace().as_ref(), "#task/2/export/exported.txt").unwrap(),
            b"task-export"
        );

        let vm_id =
            String::from_utf8(read_file(runtime.namespace().as_ref(), "#vm/new/v86").unwrap())
                .unwrap()
                .trim()
                .to_string();
        assert!(
            star9_fs::stat(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/guest")).is_err()
        );
        runtime
            .set_vm_guest(
                &vm_id,
                fs_ref(MemFs::from_entries([("guest.txt", b"vm-guest".to_vec())])),
            )
            .unwrap();
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#vm/{vm_id}/guest/guest.txt")
            )
            .unwrap(),
            b"vm-guest"
        );
        let entries = read_dir(runtime.namespace().as_ref(), &format!("#vm/{vm_id}"))
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert!(entries.contains(&"guest".to_string()));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_pty_execution_handler_routes_output_and_exit_state() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        runtime.execution_registry().register_kind(
            ProtocolExecutionKind::Native,
            NativePtyExecutionHandler::new(),
        );

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ProtocolExecutionKind::Native,
                    module: "/bin/sh".into(),
                    args: vec!["-c".into(), "printf native-ok".into()],
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(task.exit(), "0");
        let stdout = task
            .with_fd_mut(1, |file| {
                file.seek(std::io::SeekFrom::Start(0))?;
                read_handle(file)
            })
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap();
        assert!(stdout.contains("native-ok"), "{stdout:?}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_pty_execution_handler_reports_nonzero_and_spawn_errors() {
        let runtime = Runtime::new().unwrap();
        runtime.execution_registry().register_kind(
            ProtocolExecutionKind::Native,
            NativePtyExecutionHandler::new(),
        );

        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ProtocolExecutionKind::Native,
                    module: "/bin/sh".into(),
                    args: vec!["-c".into(), "exit 7".into()],
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(status, ExitStatus::ExitCode(7));
        assert_eq!(task.exit(), "7");

        let missing = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let err = runtime
            .execution_registry()
            .execute(
                &missing,
                &ExecutionSpec {
                    kind: ProtocolExecutionKind::Native,
                    module: "/definitely/missing/star9-command".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("native pty spawn failed"));
    }
}
