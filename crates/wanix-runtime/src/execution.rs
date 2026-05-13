use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use wanix_core::{Error, FileMode, FsContext, Result};
use wanix_fs::{BoxFile, FileSystem, Node};
use wanix_protocol::runtime::{
    EnvironmentEntry, ExecutionKind, ExecutionSpec, ExitStatus, FdDescriptor, FdKind, StdioSet,
    StreamDescriptor,
};
use wanix_task::Task;

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
        StreamDescriptor::Inherit => open_placeholder_fd(FdKind::Pipe, name.to_string()),
        StreamDescriptor::Null => open_placeholder_fd(FdKind::Pipe, format!("null:{name}")),
        StreamDescriptor::Fd(fd) => open_fd_descriptor(task, fd, None),
        StreamDescriptor::Port(port) => {
            open_placeholder_fd(FdKind::Port, format!("port:{}", port.port_id))
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
    open_placeholder_fd(descriptor.kind, name)
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

fn open_placeholder_fd(kind: FdKind, name: String) -> Result<(BoxFile, String)> {
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
    }
}

fn unsupported_execution(spec: &ExecutionSpec) -> Error {
    Error::Message(format!(
        "operation not supported: no execution handler registered for {} module {}",
        execution_kind_name(spec.kind),
        spec.module
    ))
}
