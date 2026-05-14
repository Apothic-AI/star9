use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use star9_core::{Error, ErrorKind, FileMode, FsContext, Metadata, OpenFlags, Result};
use star9_fs::{read_file, write_file, BoxFile, FileSystem, Node};
use star9_protocol::runtime::{
    EnvironmentEntry, ExecutionSpec, ExitStatus, FdDescriptor, FdKind, PortDescriptor, PortHandoff,
    PortOpenRequest, RuntimeDirEntry, RuntimeFdOpenRequest, RuntimeFdReadRequest, RuntimeFdRequest,
    RuntimeFdSeekRequest, RuntimeFdWriteRequest, RuntimeMetadata, RuntimePathRenameRequest,
    RuntimePathRequest, RuntimePathTruncateRequest, RuntimePathWriteRequest, RuntimeRequest,
    RuntimeResponse, RuntimeSeekWhence, StdioSet, StreamDescriptor, TaskMessage,
    TaskMessagePayload, WorkerHandle, WorkerSpawnRequest, WorkerStartRequest,
};
use star9_task::{Task, TaskFs};

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
pub struct RuntimeProtocolSnapshot {
    pub workers: Vec<WorkerSnapshot>,
    pub ports: Vec<PortSnapshot>,
    pub task_messages: Vec<TaskMessageSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerSnapshot {
    pub handle: WorkerHandle,
    pub parent_task_id: Option<String>,
    pub command: String,
    pub env: Vec<String>,
    pub cwd: String,
    pub lifecycle: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortSnapshot {
    pub descriptor: PortDescriptor,
    pub owner_task_id: String,
    pub worker_id: Option<String>,
    pub handoff_targets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskMessageSnapshot {
    pub task_id: String,
    pub messages: Vec<TaskMessage>,
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

    pub fn snapshot(&self) -> Result<RuntimeProtocolSnapshot> {
        let state = self.cloned_state();
        let mut workers = Vec::with_capacity(state.workers.len());
        for (worker_id, task_id) in state.workers {
            workers.push(self.snapshot_worker_handle(WorkerHandle { worker_id, task_id })?);
        }

        Ok(RuntimeProtocolSnapshot {
            workers,
            ports: state.ports.into_values().map(PortSnapshot::from).collect(),
            task_messages: state
                .messages
                .into_iter()
                .map(|(task_id, messages)| TaskMessageSnapshot { task_id, messages })
                .collect(),
        })
    }

    pub fn worker_snapshot(&self, worker_id: &str) -> Result<Option<WorkerSnapshot>> {
        let Some(task_id) = self.cloned_state().workers.get(worker_id).cloned() else {
            return Ok(None);
        };
        self.snapshot_worker_handle(WorkerHandle {
            worker_id: worker_id.to_string(),
            task_id,
        })
        .map(Some)
    }

    pub fn port_snapshot(&self, port_id: &str) -> Option<PortSnapshot> {
        self.cloned_state()
            .ports
            .remove(port_id)
            .map(PortSnapshot::from)
    }

    pub fn task_messages_snapshot(&self, task_id: &str) -> Option<TaskMessageSnapshot> {
        self.cloned_state()
            .messages
            .remove(task_id)
            .map(|messages| TaskMessageSnapshot {
                task_id: task_id.to_string(),
                messages,
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
            RuntimeRequest::PathRead(request) => {
                self.path_read(request).map(RuntimeResponse::Bytes)
            }
            RuntimeRequest::PathWrite(request) => {
                self.path_write(request)?;
                Ok(RuntimeResponse::Unit)
            }
            RuntimeRequest::PathStat(request) => {
                self.path_stat(request).map(RuntimeResponse::Metadata)
            }
            RuntimeRequest::PathList(request) => {
                self.path_list(request).map(RuntimeResponse::DirEntries)
            }
            RuntimeRequest::PathMkdir(request) => {
                self.path_mkdir(request)?;
                Ok(RuntimeResponse::Unit)
            }
            RuntimeRequest::PathRemove(request) => {
                self.path_remove(request)?;
                Ok(RuntimeResponse::Unit)
            }
            RuntimeRequest::PathRename(request) => {
                self.path_rename(request)?;
                Ok(RuntimeResponse::Unit)
            }
            RuntimeRequest::PathTruncate(request) => {
                self.path_truncate(request)?;
                Ok(RuntimeResponse::Unit)
            }
            RuntimeRequest::FdOpen(request) => self.fd_open(request).map(RuntimeResponse::Fd),
            RuntimeRequest::FdRead(request) => self.fd_read(request).map(RuntimeResponse::Bytes),
            RuntimeRequest::FdWrite(request) => self.fd_write(request).map(RuntimeResponse::Count),
            RuntimeRequest::FdSeek(request) => self.fd_seek(request).map(RuntimeResponse::Offset),
            RuntimeRequest::FdClose(request) => {
                self.fd_close(request)?;
                Ok(RuntimeResponse::Unit)
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
            TaskMessagePayload::StdioData(data) => {
                write_task_fd(&task, stdio_fd(data.stream), &data.data)?;
            }
            TaskMessagePayload::FdData(data) => {
                write_task_fd(&task, data.fd, &data.data)?;
            }
            TaskMessagePayload::Ready => {}
        }
        Ok(())
    }

    fn lookup_task(&self, task_id: &str) -> Result<Task> {
        if task_id.is_empty() || task_id == self.root.id() {
            Ok(self.root.clone())
        } else {
            self.task_fs.lookup(task_id)
        }
    }

    fn path_read(&self, request: RuntimePathRequest) -> Result<Vec<u8>> {
        let task = self.lookup_task(&request.task_id)?;
        read_file(task.namespace().as_ref(), &request.path)
    }

    fn path_write(&self, request: RuntimePathWriteRequest) -> Result<()> {
        let task = self.lookup_task(&request.task_id)?;
        write_file(
            task.namespace().as_ref(),
            &request.path,
            &request.data,
            FileMode::from_perm(0o644),
        )
    }

    fn path_stat(&self, request: RuntimePathRequest) -> Result<RuntimeMetadata> {
        let task = self.lookup_task(&request.task_id)?;
        task.namespace()
            .stat(&FsContext::new(), &request.path)
            .map(metadata_response)
    }

    fn path_list(&self, request: RuntimePathRequest) -> Result<Vec<RuntimeDirEntry>> {
        let task = self.lookup_task(&request.task_id)?;
        let mut entries: Vec<_> = task
            .namespace()
            .read_dir(&FsContext::new(), &request.path)?
            .into_iter()
            .map(|entry| RuntimeDirEntry {
                name: entry.name,
                metadata: metadata_response(entry.metadata),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn path_mkdir(&self, request: RuntimePathRequest) -> Result<()> {
        let task = self.lookup_task(&request.task_id)?;
        task.namespace()
            .mkdir(&request.path, FileMode::DIR | FileMode::from_perm(0o755))
    }

    fn path_remove(&self, request: RuntimePathRequest) -> Result<()> {
        let task = self.lookup_task(&request.task_id)?;
        task.namespace().remove(&request.path)
    }

    fn path_rename(&self, request: RuntimePathRenameRequest) -> Result<()> {
        let task = self.lookup_task(&request.task_id)?;
        task.namespace()
            .rename(&request.old_path, &request.new_path)
    }

    fn path_truncate(&self, request: RuntimePathTruncateRequest) -> Result<()> {
        let task = self.lookup_task(&request.task_id)?;
        task.namespace().truncate(&request.path, request.size)
    }

    fn fd_open(&self, request: RuntimeFdOpenRequest) -> Result<u32> {
        let task = self.lookup_task(&request.task_id)?;
        let flags = runtime_open_flags(&request);
        let file = task
            .namespace()
            .open_file(&request.path, flags, FileMode::from_perm(0o666))?;
        Ok(task.open_fd(file, request.path))
    }

    fn fd_read(&self, request: RuntimeFdReadRequest) -> Result<Vec<u8>> {
        let task = self.lookup_task(&request.task_id)?;
        let mut data = vec![0_u8; request.len as usize];
        let n = task.with_fd_mut(request.fd, |file| file.read(&mut data))?;
        data.truncate(n);
        Ok(data)
    }

    fn fd_write(&self, request: RuntimeFdWriteRequest) -> Result<u32> {
        let task = self.lookup_task(&request.task_id)?;
        let count = task.with_fd_mut(request.fd, |file| {
            let count = file.write(&request.data)?;
            match file.sync() {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotSupported => {}
                Err(err) => return Err(err),
            }
            Ok(count)
        })?;
        Ok(count.try_into().unwrap_or(u32::MAX))
    }

    fn fd_seek(&self, request: RuntimeFdSeekRequest) -> Result<u64> {
        let task = self.lookup_task(&request.task_id)?;
        let pos = match request.whence {
            RuntimeSeekWhence::Start => {
                SeekFrom::Start(request.offset.try_into().map_err(|_| ErrorKind::Invalid)?)
            }
            RuntimeSeekWhence::Current => SeekFrom::Current(request.offset),
            RuntimeSeekWhence::End => SeekFrom::End(request.offset),
        };
        task.with_fd_mut(request.fd, |file| file.seek(pos))
    }

    fn fd_close(&self, request: RuntimeFdRequest) -> Result<()> {
        let task = self.lookup_task(&request.task_id)?;
        task.close_fd(request.fd)
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

    fn cloned_state(&self) -> RuntimeProtocolState {
        self.state.lock().unwrap().clone()
    }

    fn snapshot_worker_handle(&self, handle: WorkerHandle) -> Result<WorkerSnapshot> {
        let task = self.task_fs.lookup(&handle.task_id)?;
        Ok(WorkerSnapshot {
            handle,
            parent_task_id: task.parent().map(|parent| parent.id()),
            command: task.cmd(),
            env: task.env(),
            cwd: task.dir(),
            lifecycle: task.exit(),
        })
    }
}

impl WorkerHost {
    pub fn worker(&self) -> &WorkerHandle {
        &self.worker
    }

    pub fn snapshot(&self) -> Result<WorkerSnapshot> {
        self.runtime.snapshot_worker_handle(self.worker.clone())
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

impl Clone for RuntimeProtocolState {
    fn clone(&self) -> Self {
        Self {
            workers: self.workers.clone(),
            ports: self.ports.clone(),
            messages: self.messages.clone(),
        }
    }
}

impl From<PortRecord> for PortSnapshot {
    fn from(record: PortRecord) -> Self {
        Self {
            descriptor: record.descriptor,
            owner_task_id: record.owner_task_id,
            worker_id: record.worker_id,
            handoff_targets: record.handoff_targets,
        }
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

fn runtime_open_flags(request: &RuntimeFdOpenRequest) -> OpenFlags {
    let wants_write = request.write || request.create || request.truncate || request.append;
    let mut flags = if request.read && wants_write {
        OpenFlags::RDWR
    } else if wants_write {
        OpenFlags::WRONLY
    } else {
        OpenFlags::RDONLY
    };
    if request.create {
        flags |= OpenFlags::CREATE;
    }
    if request.truncate {
        flags |= OpenFlags::TRUNC;
    }
    if request.append {
        flags |= OpenFlags::APPEND;
    }
    flags
}

fn metadata_response(meta: Metadata) -> RuntimeMetadata {
    let modified_unix_nanos = meta
        .modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    RuntimeMetadata {
        name: meta.name,
        mode: meta.mode.bits(),
        size: meta.size,
        modified_unix_nanos,
        uid: meta.uid,
        gid: meta.gid,
    }
}

fn render_exit_status(status: &ExitStatus) -> String {
    match status {
        ExitStatus::ExitCode(code) => code.to_string(),
        ExitStatus::Signal(signal) => format!("signal:{signal}"),
        ExitStatus::Trap(reason) => format!("trap:{reason}"),
    }
}

fn stdio_fd(stream: star9_protocol::runtime::StdioStream) -> u32 {
    match stream {
        star9_protocol::runtime::StdioStream::Stdin => 0,
        star9_protocol::runtime::StdioStream::Stdout => 1,
        star9_protocol::runtime::StdioStream::Stderr => 2,
    }
}

fn write_task_fd(task: &Task, fd: u32, data: &[u8]) -> Result<()> {
    task.with_fd_mut(fd, |file| {
        file.seek(SeekFrom::End(0))?;
        file.write(data)?;
        file.sync()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use star9_fs::{fs_ref, MemFs};
    use star9_protocol::runtime::{
        ExecutionKind, FdData, PortDescriptor, RuntimeRequest, RuntimeResponse, StdioData,
        StdioStream,
    };
    use star9_vfs::BindMode;

    fn worker_request(worker_id: &str, task_id: &str) -> WorkerHandle {
        WorkerHandle {
            worker_id: worker_id.into(),
            task_id: task_id.into(),
        }
    }

    fn snapshot_worker<'a>(
        snapshot: &'a RuntimeProtocolSnapshot,
        worker_id: &str,
    ) -> &'a WorkerSnapshot {
        snapshot
            .workers
            .iter()
            .find(|worker| worker.handle.worker_id == worker_id)
            .unwrap()
    }

    fn snapshot_port<'a>(snapshot: &'a RuntimeProtocolSnapshot, port_id: &str) -> &'a PortSnapshot {
        snapshot
            .ports
            .iter()
            .find(|port| port.descriptor.port_id == port_id)
            .unwrap()
    }

    fn snapshot_messages<'a>(
        snapshot: &'a RuntimeProtocolSnapshot,
        task_id: &str,
    ) -> &'a TaskMessageSnapshot {
        snapshot
            .task_messages
            .iter()
            .find(|entry| entry.task_id == task_id)
            .unwrap()
    }

    fn read_task_fd(task: &Task, fd: u32) -> Vec<u8> {
        task.with_fd_mut(fd, |file| {
            file.seek(SeekFrom::Start(0))?;
            let mut out = Vec::new();
            let mut buf = [0_u8; 64];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
            Ok(out)
        })
        .unwrap()
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

        let snapshot = runtime
            .protocol_host()
            .worker_snapshot(&handle.worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.handle, handle);
        assert_eq!(snapshot.parent_task_id, Some(parent.id()));
        assert_eq!(snapshot.command, "repl.wasm --interactive");
        assert_eq!(snapshot.env, vec!["TERM=xterm-256color".to_string()]);
        assert_eq!(snapshot.cwd, "/work");
        assert_eq!(snapshot.lifecycle, "started");
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
    fn runtime_requests_route_worker_path_and_fd_io_through_task_namespace() {
        let runtime = Runtime::new().unwrap();
        let mem = fs_ref(MemFs::from_entries([("input.txt", b"seed".to_vec())]));
        runtime
            .root()
            .namespace()
            .bind(mem, ".", "work", BindMode::Replace)
            .unwrap();

        let RuntimeResponse::Worker(handle) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-io", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };

        assert_eq!(
            runtime
                .handle_runtime_request(RuntimeRequest::PathRead(RuntimePathRequest {
                    task_id: handle.task_id.clone(),
                    path: "work/input.txt".into(),
                }))
                .unwrap(),
            RuntimeResponse::Bytes(b"seed".to_vec())
        );
        assert_eq!(
            runtime
                .handle_runtime_request(RuntimeRequest::PathWrite(RuntimePathWriteRequest {
                    task_id: handle.task_id.clone(),
                    path: "work/output.txt".into(),
                    data: b"abcdef".to_vec(),
                }))
                .unwrap(),
            RuntimeResponse::Unit
        );
        let RuntimeResponse::Metadata(meta) = runtime
            .handle_runtime_request(RuntimeRequest::PathStat(RuntimePathRequest {
                task_id: handle.task_id.clone(),
                path: "work/output.txt".into(),
            }))
            .unwrap()
        else {
            panic!("expected metadata response");
        };
        assert_eq!(meta.size, 6);

        runtime
            .handle_runtime_request(RuntimeRequest::PathMkdir(RuntimePathRequest {
                task_id: handle.task_id.clone(),
                path: "work/subdir".into(),
            }))
            .unwrap();
        let RuntimeResponse::DirEntries(entries) = runtime
            .handle_runtime_request(RuntimeRequest::PathList(RuntimePathRequest {
                task_id: handle.task_id.clone(),
                path: "work".into(),
            }))
            .unwrap()
        else {
            panic!("expected dir entries response");
        };
        assert_eq!(
            entries
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["input.txt", "output.txt", "subdir"]
        );

        runtime
            .handle_runtime_request(RuntimeRequest::PathRename(RuntimePathRenameRequest {
                task_id: handle.task_id.clone(),
                old_path: "work/output.txt".into(),
                new_path: "work/data.txt".into(),
            }))
            .unwrap();
        runtime
            .handle_runtime_request(RuntimeRequest::PathTruncate(RuntimePathTruncateRequest {
                task_id: handle.task_id.clone(),
                path: "work/data.txt".into(),
                size: 3,
            }))
            .unwrap();

        let RuntimeResponse::Fd(fd) = runtime
            .handle_runtime_request(RuntimeRequest::FdOpen(RuntimeFdOpenRequest {
                task_id: handle.task_id.clone(),
                path: "work/data.txt".into(),
                read: true,
                write: true,
                create: false,
                truncate: false,
                append: false,
            }))
            .unwrap()
        else {
            panic!("expected fd response");
        };
        assert_eq!(
            runtime
                .handle_runtime_request(RuntimeRequest::FdSeek(RuntimeFdSeekRequest {
                    task_id: handle.task_id.clone(),
                    fd,
                    offset: 0,
                    whence: RuntimeSeekWhence::End,
                }))
                .unwrap(),
            RuntimeResponse::Offset(3)
        );
        assert_eq!(
            runtime
                .handle_runtime_request(RuntimeRequest::FdWrite(RuntimeFdWriteRequest {
                    task_id: handle.task_id.clone(),
                    fd,
                    data: b"XYZ".to_vec(),
                }))
                .unwrap(),
            RuntimeResponse::Count(3)
        );
        runtime
            .handle_runtime_request(RuntimeRequest::FdSeek(RuntimeFdSeekRequest {
                task_id: handle.task_id.clone(),
                fd,
                offset: 0,
                whence: RuntimeSeekWhence::Start,
            }))
            .unwrap();
        assert_eq!(
            runtime
                .handle_runtime_request(RuntimeRequest::FdRead(RuntimeFdReadRequest {
                    task_id: handle.task_id.clone(),
                    fd,
                    len: 16,
                }))
                .unwrap(),
            RuntimeResponse::Bytes(b"abcXYZ".to_vec())
        );
        runtime
            .handle_runtime_request(RuntimeRequest::FdClose(RuntimeFdRequest {
                task_id: handle.task_id.clone(),
                fd,
            }))
            .unwrap();
        runtime
            .handle_runtime_request(RuntimeRequest::PathRemove(RuntimePathRequest {
                task_id: handle.task_id,
                path: "work/data.txt".into(),
            }))
            .unwrap();
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
            host.port_snapshot("events").unwrap().owner_task_id,
            source.task_id
        );

        runtime
            .handle_runtime_request(RuntimeRequest::HandoffPort(PortHandoff {
                worker: source,
                target_task_id: target.task_id.clone(),
                port,
            }))
            .unwrap();
        let record = host.port_snapshot("events").unwrap();
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
            .handle_runtime_request(RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: handle.clone(),
                execution: ExecutionSpec {
                    kind: ExecutionKind::JsWasm,
                    module: "worker.mjs".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            }))
            .unwrap();

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
        assert_eq!(
            host.task_messages_snapshot(&handle.task_id)
                .unwrap()
                .messages
                .len(),
            1
        );

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
        assert_eq!(read_task_fd(&task, 1), b"ok");
        assert_eq!(
            host.task_messages_snapshot(&handle.task_id)
                .unwrap()
                .messages
                .len(),
            2
        );
    }

    #[test]
    fn runtime_snapshots_are_cloned_and_cover_workers_ports_and_messages() {
        let runtime = Runtime::new().unwrap();
        let host = runtime.protocol_host();

        let RuntimeResponse::Worker(source) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-g", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };
        let RuntimeResponse::Worker(target) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-h", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };

        runtime
            .handle_runtime_request(RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: source.clone(),
                execution: ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "worker.wasm".into(),
                    args: vec!["serve".into()],
                    env: vec![EnvironmentEntry {
                        name: "MODE".into(),
                        value: "test".into(),
                    }],
                    cwd: Some("/srv".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            }))
            .unwrap();
        runtime
            .handle_runtime_request(RuntimeRequest::OpenPort(PortOpenRequest {
                worker: source.clone(),
                port: PortDescriptor {
                    port_id: "events".into(),
                    name: "event-bus".into(),
                },
            }))
            .unwrap();
        runtime
            .handle_runtime_request(RuntimeRequest::PostMessage(TaskMessage {
                task_id: source.task_id.clone(),
                worker_id: Some(source.worker_id.clone()),
                sequence: 1,
                payload: TaskMessagePayload::Ready,
            }))
            .unwrap();

        let mut snapshot = host.snapshot().unwrap();
        assert_eq!(snapshot.workers.len(), 2);
        assert_eq!(
            snapshot_worker(&snapshot, &source.worker_id).command,
            "worker.wasm serve"
        );
        assert_eq!(
            snapshot_worker(&snapshot, &source.worker_id).env,
            vec!["MODE=test".to_string()]
        );
        assert_eq!(snapshot_worker(&snapshot, &source.worker_id).cwd, "/srv");
        assert_eq!(
            snapshot_worker(&snapshot, &source.worker_id).lifecycle,
            "started"
        );
        assert_eq!(
            snapshot_port(&snapshot, "events").owner_task_id,
            source.task_id
        );
        assert!(snapshot_port(&snapshot, "events")
            .handoff_targets
            .is_empty());
        assert_eq!(
            snapshot_messages(&snapshot, &source.task_id).messages.len(),
            1
        );

        snapshot.workers[0].lifecycle = "mutated".into();
        snapshot.ports[0].handoff_targets.push("mutated".into());
        snapshot.task_messages[0].messages.clear();

        let fresh = host.snapshot().unwrap();
        assert_eq!(
            snapshot_worker(&fresh, &source.worker_id).lifecycle,
            "started"
        );
        assert!(snapshot_port(&fresh, "events").handoff_targets.is_empty());
        assert_eq!(snapshot_messages(&fresh, &source.task_id).messages.len(), 1);

        runtime
            .handle_runtime_request(RuntimeRequest::HandoffPort(PortHandoff {
                worker: source.clone(),
                target_task_id: target.task_id.clone(),
                port: PortDescriptor {
                    port_id: "events".into(),
                    name: "event-bus".into(),
                },
            }))
            .unwrap();
        runtime
            .handle_runtime_request(RuntimeRequest::PostMessage(TaskMessage {
                task_id: source.task_id.clone(),
                worker_id: Some(source.worker_id.clone()),
                sequence: 2,
                payload: TaskMessagePayload::Exit(ExitStatus::ExitCode(7)),
            }))
            .unwrap();

        assert_eq!(
            snapshot_worker(&snapshot, &source.worker_id).lifecycle,
            "mutated"
        );
        assert_eq!(
            snapshot_port(&snapshot, "events").handoff_targets,
            vec!["mutated".to_string()]
        );
        assert!(snapshot_messages(&snapshot, &source.task_id)
            .messages
            .is_empty());

        let updated = host.snapshot().unwrap();
        assert_eq!(snapshot_worker(&updated, &source.worker_id).lifecycle, "7");
        assert_eq!(
            snapshot_port(&updated, "events").owner_task_id,
            target.task_id
        );
        assert_eq!(
            snapshot_port(&updated, "events").handoff_targets,
            vec![target.task_id]
        );
        assert_eq!(
            snapshot_messages(&updated, &source.task_id).messages.len(),
            2
        );
    }

    #[test]
    fn worker_task_conflicts_still_error_without_mutating_snapshots() {
        let runtime = Runtime::new().unwrap();
        let host = runtime.protocol_host();
        let RuntimeResponse::Worker(handle) = runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker_request("worker-i", "ignored"),
                parent_task_id: None,
            }))
            .unwrap()
        else {
            panic!("expected worker response");
        };
        let other_task = runtime.task_fs().alloc("auto", None).unwrap();

        let err = runtime
            .handle_runtime_request(RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: WorkerHandle {
                    worker_id: handle.worker_id.clone(),
                    task_id: other_task.id(),
                },
                execution: ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "conflict.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            }))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);

        let snapshot = host.snapshot().unwrap();
        assert_eq!(snapshot.workers.len(), 1);
        assert_eq!(snapshot_worker(&snapshot, &handle.worker_id).handle, handle);
        assert!(host.worker_snapshot("missing").unwrap().is_none());
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
        assert_eq!(worker.snapshot().unwrap().lifecycle, "started");
        assert_eq!(read_task_fd(&task, 1), b"hello");
    }
}
