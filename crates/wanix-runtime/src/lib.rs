//! Wanix runtime composition.

use std::sync::Arc;

mod devices;
mod execution;
mod wasi;
mod worker;

use wanix_core::{FileMode, FsContext, Result};
use wanix_fs::{fs_ref, read_file, write_file, FileSystem, FsRef, MapFs, Node};
use wanix_protocol::p9::{LoopbackTransport, NinePClientFs, NinePServer, NinePTransport};
use wanix_task::{Task, TaskFs};
use wanix_vfs::{BindMode, Namespace};

pub use devices::{DeterministicVmProvider, VmProvider, VmProviderResource, VmProviderUpdate};
#[cfg(not(target_arch = "wasm32"))]
pub use execution::NativePtyExecutionHandler;
pub use execution::{ExecutionRegistry, FnExecutionHandler, NativeExecutionHandler};
pub use wasi::WasmiWasiHandler;
pub use worker::{RuntimeProtocolHost, WorkerHost};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct Runtime {
    root: Task,
    task_fs: TaskFs,
    devices: devices::RuntimeDevices,
    execution_registry: ExecutionRegistry,
    protocol_host: RuntimeProtocolHost,
}

impl Runtime {
    pub fn new() -> Result<Self> {
        let task_fs = TaskFs::new();
        let root = task_fs.alloc("auto", None)?;
        bind_core(&root, task_fs.clone())?;
        let devices = bind_devices(&root)?;
        let execution_registry = ExecutionRegistry::new();
        let protocol_host = RuntimeProtocolHost::new(root.clone(), task_fs.clone());
        Ok(Self {
            root,
            task_fs,
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

fn bind_devices(root: &Task) -> Result<devices::RuntimeDevices> {
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
    use wanix_fs::{fs_ref, open, read_dir, read_file, FileHandle, MemFs};
    use wanix_protocol::runtime::{
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
                        }),
                        stdout: StreamDescriptor::Fd(FdDescriptor {
                            fd: 1,
                            kind: FdKind::File,
                            path: Some("workspace/stdout.txt".into()),
                            read: false,
                            write: true,
                        }),
                        stderr: StreamDescriptor::Fd(FdDescriptor {
                            fd: 2,
                            kind: FdKind::File,
                            path: Some("workspace/stderr.txt".into()),
                            read: false,
                            write: true,
                        }),
                    },
                    fds: vec![FdDescriptor {
                        fd: 7,
                        kind: FdKind::File,
                        path: Some("workspace/extra.txt".into()),
                        read: true,
                        write: false,
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
            wanix_fs::stat(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/guest")).is_err()
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
                    module: "/definitely/missing/wanix-command".into(),
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
