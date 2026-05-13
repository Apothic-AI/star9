//! Wanix runtime composition.

use std::sync::Arc;

mod devices;
mod worker;

use wanix_core::{FileMode, FsContext, Result};
use wanix_fs::{fs_ref, read_file, write_file, FileSystem, FsRef, MapFs, Node};
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
    devices::bind_devices(root)
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
