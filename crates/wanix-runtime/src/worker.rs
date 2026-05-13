use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use wanix_core::{Error, ErrorKind, FileMode, FsContext, Result};
use wanix_fs::{BoxFile, FileSystem, Node};
use wanix_protocol::runtime::{
    EnvironmentEntry, ExecutionSpec, ExitStatus, FdDescriptor, FdKind, PortDescriptor, PortHandoff,
    PortOpenRequest, RuntimeRequest, RuntimeResponse, StdioSet, StreamDescriptor, TaskMessage,
    TaskMessagePayload, WorkerHandle, WorkerSpawnRequest, WorkerStartRequest,
};
use wanix_task::{Task, TaskFs};

use crate::Runtime;

#[derive(Clone)]
pub struct RuntimeProtocolHost {
    root: Task,
    task_fs: TaskFs,
    state: Arc<Mutex<RuntimeProtocolState>>,
}

#[derive(Default)]
struct RuntimeProtocolState {
    workers: BTreeMap<String, String>,
    ports: BTreeMap<String, PortRecord>,
    messages: BTreeMap<String, Vec<TaskMessage>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PortRecord {
    descriptor: PortDescriptor,
    owner_task_id: String,
    worker_id: Option<String>,
    handoff_targets: Vec<String>,
}

#[derive(Clone)]
pub struct WorkerHost {
    runtime: RuntimeProtocolHost,
    worker: WorkerHandle,
}

impl RuntimeProtocolHost {
    pub fn new(root: Task, task_fs: TaskFs) -> Self {
        Self {
            root,
            task_fs,
            state: Arc::new(Mutex::new(RuntimeProtocolState::default())),
        }
    }

    pub fn from_runtime(runtime: &Runtime) -> Self {
        runtime.protocol_host()
    }

    pub fn worker_host(&self, worker: WorkerHandle) -> Result<WorkerHost> {
        self.lookup_task_for_worker(&worker)?;
        Ok(WorkerHost {
            runtime: self.clone(),
            worker,
        })
    }

    pub fn handle_request(&self, request: RuntimeRequest) -> Result<RuntimeResponse> {
        match request {
            RuntimeRequest::SpawnWorker(request) => {
                self.spawn_worker(request).map(RuntimeResponse::Worker)
            }
            RuntimeRequest::StartWorker(request) => {
                self.start_worker(request)?;
                Ok(RuntimeResponse::Unit)
            }
            RuntimeRequest::OpenPort(request) => self.open_port(request).map(RuntimeResponse::Port),
            RuntimeRequest::HandoffPort(request) => {
                self.handoff_port(request).map(RuntimeResponse::Port)
            }
            RuntimeRequest::PostMessage(message) => {
                self.post_message(message.clone())?;
                Ok(RuntimeResponse::TaskMessage(message))
            }
        }
    }

    fn spawn_worker(&self, request: WorkerSpawnRequest) -> Result<WorkerHandle> {
        let parent = match request.parent_task_id.as_deref() {
            Some(parent_id) => Some(self.task_fs.lookup(parent_id)?),
            None => Some(self.root.clone()),
        };
        let task = self.task_fs.alloc("auto", parent)?;
        let handle = WorkerHandle {
            worker_id: request.worker.worker_id,
            task_id: task.id(),
        };
        self.record_worker(&handle)?;
        Ok(handle)
    }

    fn start_worker(&self, request: WorkerStartRequest) -> Result<()> {
        let task = self.lookup_task_for_worker(&request.worker)?;
        self.record_worker(&request.worker)?;
        apply_execution_spec(&task, &request.worker, &request.execution)
    }

    fn open_port(&self, request: PortOpenRequest) -> Result<PortDescriptor> {
        let task = self.lookup_task_for_worker(&request.worker)?;
        self.record_worker(&request.worker)?;
        self.upsert_port(
            request.port.clone(),
            task.id(),
            Some(request.worker.worker_id),
            None,
        )?;
        Ok(request.port)
    }

    fn handoff_port(&self, request: PortHandoff) -> Result<PortDescriptor> {
        self.lookup_task_for_worker(&request.worker)?;
        self.task_fs.lookup(&request.target_task_id)?;
        self.record_worker(&request.worker)?;
        self.upsert_port(
            request.port.clone(),
            request.target_task_id.clone(),
            Some(request.worker.worker_id),
            Some(request.target_task_id),
        )?;
        Ok(request.port)
    }

    fn post_message(&self, message: TaskMessage) -> Result<()> {
        let task = self.task_fs.lookup(&message.task_id)?;
        if let Some(worker_id) = message.worker_id.clone() {
            self.record_worker(&WorkerHandle {
                worker_id,
                task_id: task.id(),
            })?;
        }
        {
            let mut state = self.state.lock().unwrap();
            state
                .messages
                .entry(task.id())
                .or_default()
                .push(message.clone());
        }
        match message.payload {
            TaskMessagePayload::Exit(status) => {
                task.set_exit(render_exit_status(&status));
            }
            TaskMessagePayload::PortOpened(port) => {
                self.upsert_port(port, task.id(), message.worker_id, None)?;
            }
            TaskMessagePayload::PortHandoff(handoff) => {
                self.handoff_port(handoff)?;
            }
            TaskMessagePayload::Ready
            | TaskMessagePayload::StdioData(_)
            | TaskMessagePayload::FdData(_) => {}
        }
        Ok(())
    }

    fn lookup_task_for_worker(&self, worker: &WorkerHandle) -> Result<Task> {
        let task = self.task_fs.lookup(&worker.task_id)?;
        let mapped_task_id = self
            .state
            .lock()
            .unwrap()
            .workers
            .get(&worker.worker_id)
            .cloned();
        if let Some(existing_task_id) = mapped_task_id {
            if existing_task_id != worker.task_id {
                return Err(Error::path(
                    "worker",
                    worker.worker_id.clone(),
                    ErrorKind::AlreadyExists,
                ));
            }
        }
        Ok(task)
    }

    fn record_worker(&self, worker: &WorkerHandle) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        match state.workers.get(&worker.worker_id) {
            Some(existing_task_id) if existing_task_id != &worker.task_id => Err(Error::path(
                "worker",
                worker.worker_id.clone(),
                ErrorKind::AlreadyExists,
            )),
            _ => {
                state
                    .workers
                    .insert(worker.worker_id.clone(), worker.task_id.clone());
                Ok(())
            }
        }
    }

    fn upsert_port(
        &self,
        port: PortDescriptor,
        owner_task_id: String,
        worker_id: Option<String>,
        handoff_target: Option<String>,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let entry = state
            .ports
            .entry(port.port_id.clone())
            .or_insert(PortRecord {
                descriptor: port.clone(),
                owner_task_id: owner_task_id.clone(),
                worker_id: worker_id.clone(),
                handoff_targets: Vec::new(),
            });
        entry.descriptor = port;
        entry.owner_task_id = owner_task_id;
        entry.worker_id = worker_id;
        if let Some(target) = handoff_target {
            entry.handoff_targets.push(target);
        }
        Ok(())
    }

    #[cfg(test)]
    fn port_record(&self, port_id: &str) -> Option<PortRecord> {
        self.state.lock().unwrap().ports.get(port_id).cloned()
    }

    #[cfg(test)]
    fn messages_for(&self, task_id: &str) -> Vec<TaskMessage> {
        self.state
            .lock()
            .unwrap()
            .messages
            .get(task_id)
            .cloned()
            .unwrap_or_default()
    }
}

impl WorkerHost {
    pub fn worker(&self) -> &WorkerHandle {
        &self.worker
    }

    pub fn start(&self, execution: ExecutionSpec) -> Result<()> {
        self.runtime.start_worker(WorkerStartRequest {
            worker: self.worker.clone(),
            execution,
        })
    }

    pub fn open_port(&self, port: PortDescriptor) -> Result<PortDescriptor> {
        self.runtime.open_port(PortOpenRequest {
            worker: self.worker.clone(),
            port,
        })
    }

    pub fn handoff_port(
        &self,
        target_task_id: impl Into<String>,
        port: PortDescriptor,
    ) -> Result<PortDescriptor> {
        self.runtime.handoff_port(PortHandoff {
            worker: self.worker.clone(),
            target_task_id: target_task_id.into(),
            port,
        })
    }

    pub fn post_message(&self, message: TaskMessage) -> Result<()> {
        self.runtime.post_message(message)
    }
}

fn apply_execution_spec(
    task: &Task,
    worker: &WorkerHandle,
    execution: &ExecutionSpec,
) -> Result<()> {
    task.set_cmd(render_command(&execution.module, &execution.args));
    task.set_env(render_env(&execution.env));
    if let Some(cwd) = &execution.cwd {
        task.set_dir(cwd.clone());
    }
    task.set_worker(worker.worker_id.clone());
    task.set_exit("running");
    install_stdio(task, &execution.stdio)?;
    for fd in &execution.fds {
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

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{fs_ref, MemFs};
    use wanix_protocol::runtime::{
        ExecutionKind, FdData, PortDescriptor, RuntimeRequest, RuntimeResponse, StdioData,
        StdioStream,
    };
    use wanix_vfs::BindMode;

    fn worker_request(worker_id: &str, task_id: &str) -> WorkerHandle {
        WorkerHandle {
            worker_id: worker_id.into(),
            task_id: task_id.into(),
        }
    }

    #[test]
    fn spawns_workers_and_starts_tasks_from_runtime_requests() {
        let runtime = Runtime::new().unwrap();
        let parent = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let response = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-a", "ignored"),
                parent_task_id: Some(parent.id()),
            }))
            .unwrap();
        let RuntimeResponse::Worker(handle) = response else {
            panic!("expected worker response");
        };

        runtime
            .handle_runtime_request(RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: handle.clone(),
                execution: ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "repl.wasm".into(),
                    args: vec!["--interactive".into()],
                    env: vec![EnvironmentEntry {
                        name: "TERM".into(),
                        value: "xterm-256color".into(),
                    }],
                    cwd: Some("/work".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            }))
            .unwrap();

        let task = runtime.task_fs().lookup(&handle.task_id).unwrap();
        assert_eq!(task.parent().unwrap().id(), parent.id());
        assert_eq!(task.worker(), Some("worker-a".to_string()));
        assert_eq!(task.cmd(), "repl.wasm --interactive");
        assert_eq!(task.env(), vec!["TERM=xterm-256color".to_string()]);
        assert_eq!(task.dir(), "/work");
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
    fn start_worker_sets_up_stdio_and_explicit_fds() {
        let runtime = Runtime::new().unwrap();
        let mem = fs_ref(MemFs::from_entries([
            ("input.txt", b"stdin-data".to_vec()),
            ("config.toml", b"enabled=true".to_vec()),
        ]));
        runtime
            .root()
            .namespace()
            .bind(mem, ".", "work", BindMode::Replace)
            .unwrap();

        let RuntimeResponse::Worker(handle) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-b", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };

        runtime
            .handle_runtime_request(RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: handle.clone(),
                execution: ExecutionSpec {
                    kind: ExecutionKind::JsWasm,
                    module: "/bin/app.wasm".into(),
                    args: vec!["serve".into()],
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet {
                        stdin: StreamDescriptor::Fd(FdDescriptor {
                            fd: 0,
                            kind: FdKind::File,
                            path: Some("work/input.txt".into()),
                            read: true,
                            write: false,
                        }),
                        stdout: StreamDescriptor::Port(PortDescriptor {
                            port_id: "stdout-port".into(),
                            name: "stdout".into(),
                        }),
                        stderr: StreamDescriptor::Null,
                    },
                    fds: vec![FdDescriptor {
                        fd: 7,
                        kind: FdKind::File,
                        path: Some("work/config.toml".into()),
                        read: true,
                        write: false,
                    }],
                },
            }))
            .unwrap();

        let task = runtime.task_fs().lookup(&handle.task_id).unwrap();
        assert_eq!(
            task.fd_entries(),
            vec![
                (0, "work/input.txt".to_string()),
                (1, "port:stdout-port".to_string()),
                (2, "null:stderr".to_string()),
                (7, "work/config.toml".to_string())
            ]
        );
        assert_eq!(task.fd_path(7).unwrap(), "work/config.toml");

        let mut stdin = [0_u8; 16];
        let n = task.with_fd_mut(0, |file| file.read(&mut stdin)).unwrap();
        assert_eq!(&stdin[..n], b"stdin-data");

        let mut extra = [0_u8; 16];
        let n = task.with_fd_mut(7, |file| file.read(&mut extra)).unwrap();
        assert_eq!(&extra[..n], b"enabled=true");
    }

    #[test]
    fn open_and_handoff_port_requests_update_runtime_state() {
        let runtime = Runtime::new().unwrap();
        let host = runtime.protocol_host();

        let RuntimeResponse::Worker(source) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-c", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };
        let RuntimeResponse::Worker(target) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-d", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };

        let port = PortDescriptor {
            port_id: "events".into(),
            name: "event-bus".into(),
        };
        runtime
            .handle_runtime_request(RuntimeRequest::OpenPort(PortOpenRequest {
                worker: source.clone(),
                port: port.clone(),
            }))
            .unwrap();
        assert_eq!(
            host.port_record("events").unwrap().owner_task_id,
            source.task_id
        );

        runtime
            .handle_runtime_request(RuntimeRequest::HandoffPort(PortHandoff {
                worker: source,
                target_task_id: target.task_id.clone(),
                port,
            }))
            .unwrap();
        let record = host.port_record("events").unwrap();
        assert_eq!(record.owner_task_id, target.task_id);
        assert_eq!(record.handoff_targets, vec![record.owner_task_id.clone()]);
    }

    #[test]
    fn post_message_exit_updates_task_state() {
        let runtime = Runtime::new().unwrap();
        let host = runtime.protocol_host();
        let RuntimeResponse::Worker(handle) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-e", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };

        runtime
            .handle_runtime_request(RuntimeRequest::PostMessage(TaskMessage {
                task_id: handle.task_id.clone(),
                worker_id: Some(handle.worker_id.clone()),
                sequence: 4,
                payload: TaskMessagePayload::Exit(ExitStatus::ExitCode(23)),
            }))
            .unwrap();

        let task = runtime.task_fs().lookup(&handle.task_id).unwrap();
        assert_eq!(task.exit(), "23");
        assert_eq!(host.messages_for(&handle.task_id).len(), 1);

        runtime
            .handle_runtime_request(RuntimeRequest::PostMessage(TaskMessage {
                task_id: handle.task_id.clone(),
                worker_id: Some(handle.worker_id),
                sequence: 5,
                payload: TaskMessagePayload::StdioData(StdioData {
                    stream: StdioStream::Stdout,
                    data: b"ok".to_vec(),
                    eof: true,
                }),
            }))
            .unwrap();
        assert_eq!(host.messages_for(&handle.task_id).len(), 2);
    }

    #[test]
    fn worker_host_wraps_runtime_dispatch() {
        let runtime = Runtime::new().unwrap();
        let RuntimeResponse::Worker(handle) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-f", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };
        let worker = runtime.protocol_host().worker_host(handle.clone()).unwrap();
        worker
            .start(ExecutionSpec {
                kind: ExecutionKind::Wasi,
                module: "echo.wasm".into(),
                args: vec!["hello".into()],
                env: Vec::new(),
                cwd: None,
                stdio: StdioSet::default(),
                fds: Vec::new(),
            })
            .unwrap();
        worker
            .post_message(TaskMessage {
                task_id: handle.task_id.clone(),
                worker_id: Some(handle.worker_id),
                sequence: 1,
                payload: TaskMessagePayload::FdData(FdData {
                    fd: 1,
                    data: b"hello".to_vec(),
                    eof: false,
                }),
            })
            .unwrap();
        let task = runtime.task_fs().lookup(&handle.task_id).unwrap();
        assert_eq!(task.cmd(), "echo.wasm hello");
    }
}
