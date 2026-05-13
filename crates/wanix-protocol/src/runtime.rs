//! Typed Wanix worker and execution protocol messages.

use serde::{Deserialize, Serialize};
use wanix_core::Result;

use crate::{decode_cbor, encode_cbor};

pub fn encode_request(request: &RuntimeRequest) -> Result<Vec<u8>> {
    encode_cbor(request)
}

pub fn decode_request(data: &[u8]) -> Result<RuntimeRequest> {
    decode_cbor(data)
}

pub fn encode_response(response: &RuntimeResponse) -> Result<Vec<u8>> {
    encode_cbor(response)
}

pub fn decode_response(data: &[u8]) -> Result<RuntimeResponse> {
    decode_cbor(data)
}

pub fn encode_task_message(message: &TaskMessage) -> Result<Vec<u8>> {
    encode_cbor(message)
}

pub fn decode_task_message(data: &[u8]) -> Result<TaskMessage> {
    decode_cbor(data)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerHandle {
    pub worker_id: String,
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSpawnRequest {
    pub worker: WorkerHandle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStartRequest {
    pub worker: WorkerHandle,
    pub execution: ExecutionSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionSpec {
    pub kind: ExecutionKind,
    pub module: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvironmentEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub stdio: StdioSet,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fds: Vec<FdDescriptor>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvironmentEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKind {
    Wasi,
    JsWasm,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StdioSet {
    pub stdin: StreamDescriptor,
    pub stdout: StreamDescriptor,
    pub stderr: StreamDescriptor,
}

impl Default for StdioSet {
    fn default() -> Self {
        Self {
            stdin: StreamDescriptor::Inherit,
            stdout: StreamDescriptor::Inherit,
            stderr: StreamDescriptor::Inherit,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value")]
pub enum StreamDescriptor {
    Inherit,
    Null,
    Fd(FdDescriptor),
    Port(PortDescriptor),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FdDescriptor {
    pub fd: u32,
    pub kind: FdKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub read: bool,
    pub write: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FdKind {
    File,
    Directory,
    Pipe,
    Socket,
    Port,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortDescriptor {
    pub port_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortOpenRequest {
    pub worker: WorkerHandle,
    pub port: PortDescriptor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortHandoff {
    pub worker: WorkerHandle,
    pub target_task_id: String,
    pub port: PortDescriptor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskMessage {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub sequence: u64,
    pub payload: TaskMessagePayload,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value")]
pub enum TaskMessagePayload {
    Ready,
    StdioData(StdioData),
    FdData(FdData),
    PortOpened(PortDescriptor),
    PortHandoff(PortHandoff),
    Exit(ExitStatus),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StdioData {
    pub stream: StdioStream,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    #[serde(default)]
    pub eof: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StdioStream {
    Stdin,
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FdData {
    pub fd: u32,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    #[serde(default)]
    pub eof: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value")]
pub enum ExitStatus {
    ExitCode(i32),
    Signal(String),
    Trap(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "method", content = "args")]
pub enum RuntimeRequest {
    SpawnWorker(WorkerSpawnRequest),
    StartWorker(WorkerStartRequest),
    OpenPort(PortOpenRequest),
    HandoffPort(PortHandoff),
    PostMessage(TaskMessage),
}

impl RuntimeRequest {
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::SpawnWorker(_) => "SpawnWorker",
            Self::StartWorker(_) => "StartWorker",
            Self::OpenPort(_) => "OpenPort",
            Self::HandoffPort(_) => "HandoffPort",
            Self::PostMessage(_) => "PostMessage",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value")]
pub enum RuntimeResponse {
    Unit,
    Worker(WorkerHandle),
    Port(PortDescriptor),
    TaskMessage(TaskMessage),
    ExitStatus(ExitStatus),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker() -> WorkerHandle {
        WorkerHandle {
            worker_id: "worker-1".into(),
            task_id: "42".into(),
        }
    }

    #[test]
    fn cbor_round_trips_runtime_requests() {
        let request = RuntimeRequest::StartWorker(WorkerStartRequest {
            worker: worker(),
            execution: ExecutionSpec {
                kind: ExecutionKind::Wasi,
                module: "/bin/repl.wasm".into(),
                args: vec!["--interactive".into()],
                env: vec![EnvironmentEntry {
                    name: "TERM".into(),
                    value: "xterm-256color".into(),
                }],
                cwd: Some("/work".into()),
                stdio: StdioSet {
                    stdin: StreamDescriptor::Fd(FdDescriptor {
                        fd: 0,
                        kind: FdKind::Pipe,
                        path: Some("#pipe/1/data".into()),
                        read: true,
                        write: false,
                    }),
                    stdout: StreamDescriptor::Port(PortDescriptor {
                        port_id: "stdout".into(),
                        name: "worker-stdout".into(),
                    }),
                    stderr: StreamDescriptor::Null,
                },
                fds: vec![FdDescriptor {
                    fd: 3,
                    kind: FdKind::File,
                    path: Some("/work/input.txt".into()),
                    read: true,
                    write: true,
                }],
            },
        });
        let encoded = encode_request(&request).unwrap();
        assert_eq!(decode_request(&encoded).unwrap(), request);
    }

    #[test]
    fn cbor_round_trips_runtime_responses() {
        let response = RuntimeResponse::TaskMessage(TaskMessage {
            task_id: "42".into(),
            worker_id: Some("worker-1".into()),
            sequence: 9,
            payload: TaskMessagePayload::FdData(FdData {
                fd: 4,
                data: b"hello".to_vec(),
                eof: true,
            }),
        });
        let encoded = encode_response(&response).unwrap();
        assert_eq!(decode_response(&encoded).unwrap(), response);
    }

    #[test]
    fn cbor_round_trips_task_messages() {
        let message = TaskMessage {
            task_id: "42".into(),
            worker_id: Some("worker-1".into()),
            sequence: 2,
            payload: TaskMessagePayload::PortHandoff(PortHandoff {
                worker: worker(),
                target_task_id: "99".into(),
                port: PortDescriptor {
                    port_id: "rpc".into(),
                    name: "service".into(),
                },
            }),
        };
        let encoded = encode_task_message(&message).unwrap();
        assert_eq!(decode_task_message(&encoded).unwrap(), message);
    }

    #[test]
    fn request_method_names_cover_all_runtime_variants() {
        let methods = [
            RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: worker(),
                parent_task_id: Some("1".into()),
            }),
            RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: worker(),
                execution: ExecutionSpec {
                    kind: ExecutionKind::JsWasm,
                    module: "/bin/app.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: None,
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            }),
            RuntimeRequest::OpenPort(PortOpenRequest {
                worker: worker(),
                port: PortDescriptor {
                    port_id: "events".into(),
                    name: "event-bus".into(),
                },
            }),
            RuntimeRequest::HandoffPort(PortHandoff {
                worker: worker(),
                target_task_id: "9".into(),
                port: PortDescriptor {
                    port_id: "events".into(),
                    name: "event-bus".into(),
                },
            }),
            RuntimeRequest::PostMessage(TaskMessage {
                task_id: "42".into(),
                worker_id: Some("worker-1".into()),
                sequence: 1,
                payload: TaskMessagePayload::Exit(ExitStatus::ExitCode(0)),
            }),
        ];
        let method_names = methods.map(|request| request.method_name());
        assert_eq!(
            method_names,
            [
                "SpawnWorker",
                "StartWorker",
                "OpenPort",
                "HandoffPort",
                "PostMessage",
            ]
        );
    }
}
