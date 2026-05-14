use star9_core::{Error, ErrorKind, Result};
use star9_protocol::runtime::{self, RuntimeRequest, RuntimeResponse, TaskMessage};
use star9_runtime::RuntimeProtocolHost;

use crate::message_port::MessagePort;

const REQUEST_TAG: u8 = 1;
const RESPONSE_TAG: u8 = 2;
const TASK_MESSAGE_TAG: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerRuntimeMessage {
    Request(RuntimeRequest),
    Response(RuntimeResponse),
    Task(TaskMessage),
}

pub fn encode_worker_runtime_message(message: &WorkerRuntimeMessage) -> Result<Vec<u8>> {
    let (tag, payload) = match message {
        WorkerRuntimeMessage::Request(request) => (REQUEST_TAG, runtime::encode_request(request)?),
        WorkerRuntimeMessage::Response(response) => {
            (RESPONSE_TAG, runtime::encode_response(response)?)
        }
        WorkerRuntimeMessage::Task(message) => {
            (TASK_MESSAGE_TAG, runtime::encode_task_message(message)?)
        }
    };

    let mut encoded = Vec::with_capacity(1 + payload.len());
    encoded.push(tag);
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub fn decode_worker_runtime_message(data: &[u8]) -> Result<WorkerRuntimeMessage> {
    let Some((&tag, payload)) = data.split_first() else {
        return Err(ErrorKind::UnexpectedEof.into());
    };

    match tag {
        REQUEST_TAG => runtime::decode_request(payload).map(WorkerRuntimeMessage::Request),
        RESPONSE_TAG => runtime::decode_response(payload).map(WorkerRuntimeMessage::Response),
        TASK_MESSAGE_TAG => runtime::decode_task_message(payload).map(WorkerRuntimeMessage::Task),
        _ => Err(Error::Message(format!(
            "unknown worker runtime message tag {tag}"
        ))),
    }
}

pub trait RuntimeMessageHandler: Clone + Send + Sync + 'static {
    fn handle_request(&self, request: RuntimeRequest) -> Result<RuntimeResponse>;

    fn handle_task_message(&self, message: TaskMessage) -> Result<()>;
}

impl RuntimeMessageHandler for RuntimeProtocolHost {
    fn handle_request(&self, request: RuntimeRequest) -> Result<RuntimeResponse> {
        RuntimeProtocolHost::handle_request(self, request)
    }

    fn handle_task_message(&self, message: TaskMessage) -> Result<()> {
        RuntimeProtocolHost::handle_request(self, RuntimeRequest::PostMessage(message)).map(|_| ())
    }
}

#[derive(Clone)]
pub struct WebWorkerAdapter<P> {
    port: P,
}

impl<P> WebWorkerAdapter<P>
where
    P: MessagePort,
{
    pub fn new(port: P) -> Self {
        Self { port }
    }

    pub fn post_request(&self, request: &RuntimeRequest) -> Result<()> {
        self.post_message(&WorkerRuntimeMessage::Request(request.clone()))
    }

    pub fn post_response(&self, response: &RuntimeResponse) -> Result<()> {
        self.post_message(&WorkerRuntimeMessage::Response(response.clone()))
    }

    pub fn post_task_message(&self, message: &TaskMessage) -> Result<()> {
        self.post_message(&WorkerRuntimeMessage::Task(message.clone()))
    }

    pub fn try_next_message(&self) -> Result<Option<WorkerRuntimeMessage>> {
        self.port
            .try_recv_message()?
            .map(|message| decode_worker_runtime_message(&message))
            .transpose()
    }

    pub fn try_recv_response(&self) -> Result<Option<RuntimeResponse>> {
        match self.try_next_message()? {
            Some(WorkerRuntimeMessage::Response(response)) => Ok(Some(response)),
            Some(other) => Err(unexpected_message("response", &other)),
            None => Ok(None),
        }
    }

    pub fn try_recv_task_message(&self) -> Result<Option<TaskMessage>> {
        match self.try_next_message()? {
            Some(WorkerRuntimeMessage::Task(message)) => Ok(Some(message)),
            Some(other) => Err(unexpected_message("task message", &other)),
            None => Ok(None),
        }
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    fn post_message(&self, message: &WorkerRuntimeMessage) -> Result<()> {
        self.port
            .post_message(&encode_worker_runtime_message(message)?)
    }
}

#[derive(Clone)]
pub struct BrowserWorkerRuntime<P, H> {
    adapter: WebWorkerAdapter<P>,
    handler: H,
}

impl<P, H> BrowserWorkerRuntime<P, H>
where
    P: MessagePort,
    H: RuntimeMessageHandler,
{
    pub fn new(port: P, handler: H) -> Self {
        Self {
            adapter: WebWorkerAdapter::new(port),
            handler,
        }
    }

    pub fn adapter(&self) -> &WebWorkerAdapter<P> {
        &self.adapter
    }

    pub fn process_next_message(&self) -> Result<Option<WorkerRuntimeMessage>> {
        let Some(message) = self.adapter.try_next_message()? else {
            return Ok(None);
        };

        match message {
            WorkerRuntimeMessage::Request(request) => {
                let response = self.handler.handle_request(request)?;
                self.adapter.post_response(&response)?;
                Ok(Some(WorkerRuntimeMessage::Response(response)))
            }
            WorkerRuntimeMessage::Task(message) => {
                self.handler.handle_task_message(message.clone())?;
                Ok(Some(WorkerRuntimeMessage::Task(message)))
            }
            WorkerRuntimeMessage::Response(response) => Err(unexpected_message(
                "request or task message",
                &WorkerRuntimeMessage::Response(response),
            )),
        }
    }
}

fn unexpected_message(expected: &str, actual: &WorkerRuntimeMessage) -> Error {
    Error::Message(format!(
        "expected {expected}, received {}",
        actual.kind_name()
    ))
}

impl WorkerRuntimeMessage {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Request(_) => "request",
            Self::Response(_) => "response",
            Self::Task(_) => "task message",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::message_port::InMemoryMessagePort;
    use star9_protocol::runtime::{
        EnvironmentEntry, ExecutionKind, ExecutionSpec, ExitStatus, StdioSet, TaskMessagePayload,
        WorkerHandle, WorkerSpawnRequest, WorkerStartRequest,
    };
    use star9_runtime::Runtime;

    #[derive(Clone, Default)]
    struct RecordingHandler {
        requests: Arc<Mutex<Vec<RuntimeRequest>>>,
        task_messages: Arc<Mutex<Vec<TaskMessage>>>,
    }

    impl RuntimeMessageHandler for RecordingHandler {
        fn handle_request(&self, request: RuntimeRequest) -> Result<RuntimeResponse> {
            self.requests.lock().unwrap().push(request);
            Ok(RuntimeResponse::Unit)
        }

        fn handle_task_message(&self, message: TaskMessage) -> Result<()> {
            self.task_messages.lock().unwrap().push(message);
            Ok(())
        }
    }

    fn execution_spec() -> ExecutionSpec {
        ExecutionSpec {
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
        }
    }

    #[test]
    fn browser_worker_runtime_round_trips_spawn_and_start_requests() {
        let runtime = Runtime::new().unwrap();
        let parent = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let (client_port, worker_port) = InMemoryMessagePort::channel();
        let client = WebWorkerAdapter::new(client_port);
        let worker = BrowserWorkerRuntime::new(worker_port, runtime.protocol_host());

        client
            .post_request(&RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: WorkerHandle {
                    worker_id: "worker-a".into(),
                    task_id: "ignored".into(),
                },
                parent_task_id: Some(parent.id()),
            }))
            .unwrap();
        let processed = worker.process_next_message().unwrap();
        let spawn_response = client.try_recv_response().unwrap().unwrap();
        assert_eq!(
            processed,
            Some(WorkerRuntimeMessage::Response(spawn_response.clone()))
        );
        let handle = spawn_response.into_worker();

        client
            .post_request(&RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: handle.clone(),
                execution: execution_spec(),
            }))
            .unwrap();
        assert_eq!(
            worker.process_next_message().unwrap(),
            Some(WorkerRuntimeMessage::Response(RuntimeResponse::Unit))
        );
        assert_eq!(
            client.try_recv_response().unwrap(),
            Some(RuntimeResponse::Unit)
        );

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
    fn browser_worker_runtime_delivers_task_messages_to_runtime_protocol_host() {
        let runtime = Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let (client_port, worker_port) = InMemoryMessagePort::channel();
        let client = WebWorkerAdapter::new(client_port);
        let worker = BrowserWorkerRuntime::new(worker_port, runtime.protocol_host());
        let message = TaskMessage {
            task_id: task.id(),
            worker_id: Some("worker-exit".into()),
            sequence: 3,
            payload: TaskMessagePayload::Exit(ExitStatus::ExitCode(0)),
        };

        client.post_task_message(&message).unwrap();

        assert_eq!(
            worker.process_next_message().unwrap(),
            Some(WorkerRuntimeMessage::Task(message.clone()))
        );
        assert_eq!(runtime.task_fs().lookup(&task.id()).unwrap().exit(), "0");
        assert_eq!(client.try_next_message().unwrap(), None);
    }

    #[test]
    fn browser_worker_runtime_can_dispatch_to_custom_handler() {
        let handler = RecordingHandler::default();
        let (client_port, worker_port) = InMemoryMessagePort::channel();
        let client = WebWorkerAdapter::new(client_port);
        let worker = BrowserWorkerRuntime::new(worker_port, handler.clone());
        let task_message = TaskMessage {
            task_id: "task-1".into(),
            worker_id: Some("worker-1".into()),
            sequence: 8,
            payload: TaskMessagePayload::Ready,
        };

        client
            .post_request(&RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: WorkerHandle {
                    worker_id: "worker-1".into(),
                    task_id: "task-1".into(),
                },
                execution: execution_spec(),
            }))
            .unwrap();
        assert_eq!(
            worker.process_next_message().unwrap(),
            Some(WorkerRuntimeMessage::Response(RuntimeResponse::Unit))
        );
        assert_eq!(
            client.try_recv_response().unwrap(),
            Some(RuntimeResponse::Unit)
        );

        client.post_task_message(&task_message).unwrap();
        assert_eq!(
            worker.process_next_message().unwrap(),
            Some(WorkerRuntimeMessage::Task(task_message.clone()))
        );
        assert_eq!(
            handler.requests.lock().unwrap().as_slice(),
            &[RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: WorkerHandle {
                    worker_id: "worker-1".into(),
                    task_id: "task-1".into(),
                },
                execution: execution_spec(),
            })]
        );
        assert_eq!(
            handler.task_messages.lock().unwrap().as_slice(),
            &[task_message]
        );
    }

    trait IntoWorker {
        fn into_worker(self) -> WorkerHandle;
    }

    impl IntoWorker for RuntimeResponse {
        fn into_worker(self) -> WorkerHandle {
            match self {
                RuntimeResponse::Worker(worker) => worker,
                other => panic!("expected worker response, got {other:?}"),
            }
        }
    }
}
