use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, SeekFrom, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use star9_core::ErrorKind;
use star9_core::{Error, FileMode, FsContext, Result};
use star9_fs::{BoxFile, FileSystem, Node};
use star9_protocol::runtime::{
    EnvironmentEntry, ExecutionKind, ExecutionSpec, ExitStatus, FdDescriptor, FdKind, StdioSet,
    StreamDescriptor,
};
use star9_task::Task;

pub trait NativeExecutionHandler: Send + Sync {
    fn execute(&self, task: &Task, spec: &ExecutionSpec) -> Result<ExitStatus>;
}

#[derive(Clone)]
pub struct FnExecutionHandler {
    handler: Arc<ExecutionHandlerFn>,
}

type ExecutionHandlerFn = dyn Fn(&Task, &ExecutionSpec) -> Result<ExitStatus> + Send + Sync;

impl FnExecutionHandler {
    pub fn new(
        handler: impl Fn(&Task, &ExecutionSpec) -> Result<ExitStatus> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Arc::new(handler),
        }
    }
}

impl NativeExecutionHandler for FnExecutionHandler {
    fn execute(&self, task: &Task, spec: &ExecutionSpec) -> Result<ExitStatus> {
        (self.handler)(task, spec)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
pub struct NativePtyExecutionHandler {
    rows: u16,
    cols: u16,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativePtyExecutionHandler {
    pub fn new() -> Self {
        Self { rows: 24, cols: 80 }
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for NativePtyExecutionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeExecutionHandler for NativePtyExecutionHandler {
    fn execute(&self, task: &Task, spec: &ExecutionSpec) -> Result<ExitStatus> {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};

        if spec.module.trim().is_empty() {
            return Err(Error::path("exec", &spec.module, ErrorKind::Invalid));
        }

        let mut command = CommandBuilder::new(&spec.module);
        command.args(&spec.args);
        for entry in &spec.env {
            command.env(&entry.name, &entry.value);
        }
        if let Some(cwd) = spec.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
            command.cwd(cwd);
        }

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: self.rows,
                cols: self.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| Error::Message(format!("native pty open failed: {err}")))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| Error::Message(format!("native pty reader failed: {err}")))?;
        let (output_tx, output_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if output_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| Error::Message(format!("native pty spawn failed: {err}")))?;

        {
            let mut writer = pair
                .master
                .take_writer()
                .map_err(|err| Error::Message(format!("native pty writer failed: {err}")))?;
            let stdin = read_task_fd(task, 0)?;
            if !stdin.is_empty() {
                writer
                    .write_all(&stdin)
                    .map_err(|err| Error::Message(format!("native pty stdin failed: {err}")))?;
            }
        }

        let status = child
            .wait()
            .map_err(|err| Error::Message(format!("native pty wait failed: {err}")))?;
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            match output_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(chunk) => output.extend_from_slice(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) if !output.is_empty() => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(chunk) = output_rx.try_recv() {
                output.extend_from_slice(&chunk);
            }
        }
        if !output.is_empty() {
            append_task_fd(task, 1, &output)?;
        }

        Ok(match status.signal() {
            Some(signal) => ExitStatus::Signal(signal.to_string()),
            None => ExitStatus::ExitCode(status.exit_code() as i32),
        })
    }
}

#[derive(Clone, Default)]
pub struct ExecutionRegistry {
    handlers: Arc<RwLock<BTreeMap<ExecutionKey, Arc<dyn NativeExecutionHandler>>>>,
    kind_handlers: Arc<RwLock<BTreeMap<&'static str, Arc<dyn NativeExecutionHandler>>>>,
}

impl ExecutionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        kind: ExecutionKind,
        module: impl Into<String>,
        handler: impl NativeExecutionHandler + 'static,
    ) {
        self.register_arc(kind, module, Arc::new(handler));
    }

    pub fn register_fn(
        &self,
        kind: ExecutionKind,
        module: impl Into<String>,
        handler: impl Fn(&Task, &ExecutionSpec) -> Result<ExitStatus> + Send + Sync + 'static,
    ) {
        self.register(kind, module, FnExecutionHandler::new(handler));
    }

    pub fn register_kind(
        &self,
        kind: ExecutionKind,
        handler: impl NativeExecutionHandler + 'static,
    ) {
        self.register_kind_arc(kind, Arc::new(handler));
    }

    pub fn register_kind_fn(
        &self,
        kind: ExecutionKind,
        handler: impl Fn(&Task, &ExecutionSpec) -> Result<ExitStatus> + Send + Sync + 'static,
    ) {
        self.register_kind(kind, FnExecutionHandler::new(handler));
    }

    pub fn execute(&self, task: &Task, spec: &ExecutionSpec) -> Result<ExitStatus> {
        apply_execution_spec(task, spec)?;
        let handler = self
            .handler(spec.kind, &spec.module)
            .ok_or_else(|| unsupported_execution(spec))?;
        let status = handler.execute(task, spec)?;
        task.set_exit(render_exit_status(&status));
        Ok(status)
    }

    pub fn has_handler(&self, kind: ExecutionKind, module: &str) -> bool {
        self.handler(kind, module).is_some()
    }

    fn register_arc(
        &self,
        kind: ExecutionKind,
        module: impl Into<String>,
        handler: Arc<dyn NativeExecutionHandler>,
    ) {
        self.handlers
            .write()
            .unwrap()
            .insert(ExecutionKey::new(kind, module), handler);
    }

    fn register_kind_arc(&self, kind: ExecutionKind, handler: Arc<dyn NativeExecutionHandler>) {
        self.kind_handlers
            .write()
            .unwrap()
            .insert(execution_kind_name(kind), handler);
    }

    fn handler(
        &self,
        kind: ExecutionKind,
        module: &str,
    ) -> Option<Arc<dyn NativeExecutionHandler>> {
        self.handlers
            .read()
            .unwrap()
            .get(&ExecutionKey::new(kind, module))
            .cloned()
            .or_else(|| {
                self.kind_handlers
                    .read()
                    .unwrap()
                    .get(execution_kind_name(kind))
                    .cloned()
            })
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ExecutionKey {
    kind: &'static str,
    module: String,
}

impl ExecutionKey {
    fn new(kind: ExecutionKind, module: impl Into<String>) -> Self {
        Self {
            kind: execution_kind_name(kind),
            module: module.into(),
        }
    }
}

fn apply_execution_spec(task: &Task, spec: &ExecutionSpec) -> Result<()> {
    task.set_cmd(render_command(&spec.module, &spec.args));
    task.set_env(render_env(&spec.env));
    if let Some(cwd) = &spec.cwd {
        task.set_dir(cwd.clone());
    }
    task.set_worker(format!(
        "native:{}:{}",
        execution_kind_name(spec.kind),
        spec.module
    ));
    task.set_exit("running");
    install_stdio(task, &spec.stdio)?;
    for fd in &spec.fds {
        let (file, path) = open_fd_descriptor(task, fd, Some(fd.fd))?;
        task.set_fd(fd.fd, file, path);
    }
    task.set_exit("started");
    Ok(())
}

fn render_command(module: &str, args: &[String]) -> String {
    if args.is_empty() {
        module.to_string()
    } else {
        format!("{module} {}", args.join(" "))
    }
}

fn render_env(entries: &[EnvironmentEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| format!("{}={}", entry.name, entry.value))
        .collect()
}

fn install_stdio(task: &Task, stdio: &StdioSet) -> Result<()> {
    for (fd, name, descriptor) in [
        (0, "stdin", &stdio.stdin),
        (1, "stdout", &stdio.stdout),
        (2, "stderr", &stdio.stderr),
    ] {
        let (file, path) = open_stream_descriptor(task, name, descriptor)?;
        task.set_fd(fd, file, path);
    }
    Ok(())
}

fn open_stream_descriptor(
    task: &Task,
    name: &str,
    descriptor: &StreamDescriptor,
) -> Result<(BoxFile, String)> {
    match descriptor {
        StreamDescriptor::Inherit => open_virtual_fd(FdKind::Pipe, name.to_string()),
        StreamDescriptor::Null => open_virtual_fd(FdKind::Pipe, format!("null:{name}")),
        StreamDescriptor::Fd(fd) => open_fd_descriptor(task, fd, None),
        StreamDescriptor::Port(port) => {
            open_virtual_fd(FdKind::Port, format!("port:{}", port.port_id))
        }
    }
}

fn open_fd_descriptor(
    task: &Task,
    descriptor: &FdDescriptor,
    default_fd: Option<u32>,
) -> Result<(BoxFile, String)> {
    if let Some(path) = descriptor.path.as_deref() {
        let file = task.namespace().open(&FsContext::new(), path)?;
        return Ok((file, path.to_string()));
    }
    let name = fallback_fd_path(descriptor, default_fd);
    open_virtual_fd(descriptor.kind, name)
}

fn fallback_fd_path(descriptor: &FdDescriptor, default_fd: Option<u32>) -> String {
    match descriptor.kind {
        FdKind::Port => format!("port-fd:{}", descriptor.fd),
        FdKind::Directory => format!("dir-fd:{}", descriptor.fd),
        FdKind::Pipe => format!("pipe-fd:{}", descriptor.fd),
        FdKind::Socket => format!("socket-fd:{}", descriptor.fd),
        FdKind::File => format!("file-fd:{}", default_fd.unwrap_or(descriptor.fd)),
    }
}

fn open_virtual_fd(kind: FdKind, name: String) -> Result<(BoxFile, String)> {
    let node = match kind {
        FdKind::Directory => Node::dir(name.clone(), FileMode::DIR | FileMode::from_perm(0o755)),
        FdKind::File | FdKind::Pipe | FdKind::Socket | FdKind::Port => {
            Node::file(name.clone(), Vec::new(), FileMode::from_perm(0o666))
        }
    };
    Ok((node.open(&FsContext::new(), ".")?, name))
}

fn render_exit_status(status: &ExitStatus) -> String {
    match status {
        ExitStatus::ExitCode(code) => code.to_string(),
        ExitStatus::Signal(signal) => format!("signal:{signal}"),
        ExitStatus::Trap(reason) => format!("trap:{reason}"),
    }
}

fn execution_kind_name(kind: ExecutionKind) -> &'static str {
    match kind {
        ExecutionKind::Wasi => "wasi",
        ExecutionKind::JsWasm => "js_wasm",
        ExecutionKind::Native => "native",
    }
}

fn unsupported_execution(spec: &ExecutionSpec) -> Error {
    Error::Message(format!(
        "operation not supported: no execution handler registered for {} module {}",
        execution_kind_name(spec.kind),
        spec.module
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_task_fd(task: &Task, fd: u32) -> Result<Vec<u8>> {
    task.with_fd_mut(fd, |file| {
        let _ = file.seek(SeekFrom::Start(0));
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
        Ok(out)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn append_task_fd(task: &Task, fd: u32, data: &[u8]) -> Result<()> {
    task.with_fd_mut(fd, |file| {
        let _ = file.seek(SeekFrom::End(0));
        let written = file.write(data)?;
        if written != data.len() {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        file.sync().or_else(|err| {
            if err.kind() == ErrorKind::NotSupported {
                Ok(())
            } else {
                Err(err)
            }
        })
    })
}
