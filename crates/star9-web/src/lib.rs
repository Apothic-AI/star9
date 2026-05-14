//! Browser/WASM entry points for the Rust Star 9 runtime.

mod bindings;
mod descriptors;
pub mod message_port;
pub mod p9_transport;
mod storage;
pub mod worker;

use std::io::Cursor;
use std::sync::Arc;

use star9_core::{Error, ErrorKind, FileMode, Result};
use star9_fs::{fs_ref, open, MemFs, TarFs};
use star9_protocol::{
    p9::{NinePClientFs, NinePServer},
    runtime::{
        ExecutionSpec, ExitStatus, PortDescriptor, PortHandoff, PortOpenRequest, RuntimeRequest,
        RuntimeResponse, StdioData, StdioStream, TaskMessage, TaskMessagePayload, WorkerHandle,
        WorkerSpawnRequest, WorkerStartRequest,
    },
    Star9Api,
};
use star9_runtime::{ExecutionAdapter, Runtime};
use star9_vfs::BindMode;

pub use bindings::*;
pub use descriptors::*;
pub use storage::*;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct Star9System {
    runtime: Runtime,
    api: Star9Api,
    p9_server: Arc<NinePServer>,
    binding_registry: BrowserBindingRegistry,
    storage_registry: BrowserStorageRegistry,
}

impl Star9System {
    fn build() -> Result<Self> {
        let runtime = Runtime::new()?;
        let api = Star9Api::new(runtime.root());
        let p9_server = runtime.export_9p();
        let binding_registry = BrowserBindingRegistry::new();
        let storage_registry = BrowserStorageRegistry::new();
        Ok(Self {
            runtime,
            api,
            p9_server,
            binding_registry,
            storage_registry,
        })
    }

    pub fn runtime(&self) -> Runtime {
        self.runtime.clone()
    }

    pub fn api(&self) -> Star9Api {
        self.api.clone()
    }

    pub fn binding_registry(&self) -> BrowserBindingRegistry {
        self.binding_registry.clone()
    }

    pub fn storage_registry(&self) -> BrowserStorageRegistry {
        self.storage_registry.clone()
    }

    pub fn register_file_bytes_native(&self, src: &str, bytes: &[u8]) -> Result<()> {
        self.binding_registry.register_file_bytes(src, bytes)
    }

    pub fn register_file_text_native(&self, src: &str, text: &str) -> Result<()> {
        self.register_file_bytes_native(src, text.as_bytes())
    }

    pub fn register_archive_bytes_native(&self, src: &str, bytes: &[u8]) -> Result<()> {
        self.binding_registry.register_archive_bytes(src, bytes)
    }

    pub fn read_text_native(&self, path: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.api.read_file(path)?).into_owned())
    }

    pub fn write_text_native(&self, path: &str, value: &str) -> Result<()> {
        self.api.write_file(path, value.as_bytes())
    }

    pub fn write_existing_native(&self, path: &str, data: &[u8]) -> Result<()> {
        let namespace = self.runtime.namespace();
        let mut file = open(namespace.as_ref(), path)?;
        let written = file.write(data)?;
        if written != data.len() {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        file.close()
    }

    pub fn write_existing_text_native(&self, path: &str, value: &str) -> Result<()> {
        self.write_existing_native(path, value.as_bytes())
    }

    pub fn read_dir_native(&self, path: &str) -> Result<Vec<String>> {
        self.api.read_dir(path)
    }

    pub fn stat_native(&self, path: &str) -> Result<star9_protocol::StatInfo> {
        self.api.stat(path)
    }

    pub fn mkdir_native(&self, path: &str) -> Result<()> {
        self.api.mkdir(path)
    }

    pub fn remove_native(&self, path: &str) -> Result<()> {
        self.api.remove(path)
    }

    pub fn bind_ramfs_native(&self, dst: &str) -> Result<()> {
        self.runtime
            .namespace()
            .bind(fs_ref(MemFs::new()), ".", dst, BindMode::Replace)
    }

    pub fn mount_self_9p_native(&self, dst: &str) -> Result<()> {
        self.runtime
            .import_9p_loopback(dst, self.p9_server.clone(), BindMode::Replace)?;
        Ok(())
    }

    pub fn handle_9p_frame_native(&self, frame: &[u8]) -> Result<Vec<u8>> {
        self.p9_server.handle_frame(frame)
    }

    pub fn setup_namespace_native(&self, task_id: &str, bindings: &[WebBinding]) -> Result<()> {
        let task = self.runtime.root().lookup(task_id)?;
        for binding in bindings {
            binding.validate()?;
            if let Some(storage) = &binding.storage {
                let fs = self.storage_registry.resolve(storage)?;
                task.namespace()
                    .bind(fs, ".", &binding.dst, BindMode::After)?;
                continue;
            }
            match binding.kind {
                WebBindingKind::Ns => {
                    let src = binding.src_or_default().unwrap_or(".");
                    task.namespace().bind(
                        self.runtime.namespace(),
                        src,
                        &binding.dst,
                        BindMode::After,
                    )?;
                }
                WebBindingKind::File => {
                    let data = match binding.src.as_deref() {
                        Some(src) => self
                            .binding_registry
                            .file_bytes(src)
                            .ok_or_else(|| missing_binding_source("file", src))?,
                        None => (&b""[..]).into(),
                    };
                    star9_fs::write_file(
                        task.namespace().as_ref(),
                        &binding.dst,
                        data.as_ref(),
                        FileMode::from_perm(0o644),
                    )?;
                }
                WebBindingKind::Archive => {
                    let src = binding.src.as_deref().expect("archive src validated");
                    let archive = self
                        .binding_registry
                        .archive_bytes(src)
                        .ok_or_else(|| missing_binding_source("archive", src))?;
                    let fs = fs_ref(TarFs::from_reader(Cursor::new(archive.as_ref()))?);
                    task.namespace()
                        .bind(fs, ".", &binding.dst, BindMode::After)?;
                }
                WebBindingKind::Import => {
                    let src = binding.src.as_deref().expect("import src validated");
                    let transport = self
                        .binding_registry
                        .import_transport(src)
                        .ok_or_else(|| missing_binding_source("import", src))?;
                    let fs = fs_ref(NinePClientFs::connect(transport)?);
                    task.namespace()
                        .bind(fs, ".", &binding.dst, BindMode::After)?;
                }
            }
        }
        Ok(())
    }

    pub fn start_wasi_native(&self, command: &str) -> Result<String> {
        self.start_task_native("wasi", command)
    }

    pub fn start_gojs_native(&self, command: &str) -> Result<String> {
        self.start_task_native("gojs", command)
    }

    pub fn start_task_native(&self, kind: &str, command: &str) -> Result<String> {
        let task = self
            .runtime
            .task_fs()
            .alloc(task_alloc_kind(kind), Some(self.runtime.root()))?;
        if let Some(adapter) = execution_adapter(kind, command) {
            adapter.start(&task)?;
        } else {
            task.set_cmd(command);
            task.start()?;
        }
        Ok(task.id())
    }

    pub fn handle_runtime_request_native(&self, data: &[u8]) -> Result<Vec<u8>> {
        let request = star9_protocol::runtime::decode_request(data)?;
        let response = self.runtime.handle_runtime_request(request)?;
        star9_protocol::runtime::encode_response(&response)
    }

    pub fn handle_runtime_task_message_native(&self, data: &[u8]) -> Result<Vec<u8>> {
        let message = star9_protocol::runtime::decode_task_message(data)?;
        let response = self.runtime.handle_runtime_request(
            star9_protocol::runtime::RuntimeRequest::PostMessage(message),
        )?;
        star9_protocol::runtime::encode_response(&response)
    }

    pub fn spawn_worker_native(
        &self,
        worker_id: impl Into<String>,
        parent_task_id: Option<String>,
    ) -> Result<WorkerHandle> {
        let response = self
            .runtime
            .handle_runtime_request(RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: WorkerHandle {
                    worker_id: worker_id.into(),
                    task_id: String::new(),
                },
                parent_task_id,
            }))?;
        match response {
            RuntimeResponse::Worker(worker) => Ok(worker),
            _ => Err(Error::Message(
                "runtime returned non-worker response".into(),
            )),
        }
    }

    pub fn start_worker_native(
        &self,
        worker: WorkerHandle,
        execution: ExecutionSpec,
    ) -> Result<()> {
        let response = self
            .runtime
            .handle_runtime_request(RuntimeRequest::StartWorker(WorkerStartRequest {
                worker,
                execution,
            }))?;
        match response {
            RuntimeResponse::Unit => Ok(()),
            _ => Err(Error::Message("runtime returned non-unit response".into())),
        }
    }

    pub fn open_worker_port_native(
        &self,
        worker: WorkerHandle,
        port: PortDescriptor,
    ) -> Result<PortDescriptor> {
        let response = self
            .runtime
            .handle_runtime_request(RuntimeRequest::OpenPort(PortOpenRequest { worker, port }))?;
        match response {
            RuntimeResponse::Port(port) => Ok(port),
            _ => Err(Error::Message("runtime returned non-port response".into())),
        }
    }

    pub fn handoff_worker_port_native(
        &self,
        worker: WorkerHandle,
        target_task_id: impl Into<String>,
        port: PortDescriptor,
    ) -> Result<PortDescriptor> {
        let response = self
            .runtime
            .handle_runtime_request(RuntimeRequest::HandoffPort(PortHandoff {
                worker,
                target_task_id: target_task_id.into(),
                port,
            }))?;
        match response {
            RuntimeResponse::Port(port) => Ok(port),
            _ => Err(Error::Message("runtime returned non-port response".into())),
        }
    }

    pub fn post_worker_exit_native(
        &self,
        task_id: impl Into<String>,
        worker_id: Option<String>,
        sequence: u64,
        code: i32,
    ) -> Result<()> {
        self.post_worker_task_message_native(TaskMessage {
            task_id: task_id.into(),
            worker_id,
            sequence,
            payload: TaskMessagePayload::Exit(ExitStatus::ExitCode(code)),
        })
    }

    pub fn post_worker_stdout_native(
        &self,
        task_id: impl Into<String>,
        worker_id: Option<String>,
        sequence: u64,
        data: &[u8],
        eof: bool,
    ) -> Result<()> {
        self.post_worker_task_message_native(TaskMessage {
            task_id: task_id.into(),
            worker_id,
            sequence,
            payload: TaskMessagePayload::StdioData(StdioData {
                stream: StdioStream::Stdout,
                data: data.to_vec(),
                eof,
            }),
        })
    }

    pub fn post_worker_task_message_native(&self, message: TaskMessage) -> Result<()> {
        let response = self
            .runtime
            .handle_runtime_request(RuntimeRequest::PostMessage(message))?;
        match response {
            RuntimeResponse::TaskMessage(_) => Ok(()),
            _ => Err(Error::Message(
                "runtime returned non-task-message response".into(),
            )),
        }
    }
}

fn task_alloc_kind(kind: &str) -> &str {
    if execution_adapter_kind(kind) {
        "auto"
    } else {
        kind
    }
}

fn execution_adapter_kind(kind: &str) -> bool {
    matches!(kind, "wasi" | "gojs" | "go-js")
}

fn execution_adapter(kind: &str, command: &str) -> Option<ExecutionAdapter> {
    match kind {
        "wasi" => Some(ExecutionAdapter::wasi(command)),
        "gojs" | "go-js" => Some(ExecutionAdapter::go_js(command)),
        _ => None,
    }
}

fn missing_binding_source(kind: &str, src: &str) -> Error {
    Error::Message(format!("{kind} binding source {src:?} is not registered"))
}

#[cfg(not(target_arch = "wasm32"))]
impl Star9System {
    pub fn new() -> Result<Self> {
        Self::build()
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use wasm_bindgen::prelude::*;

    fn js_err(err: impl std::fmt::Display) -> JsValue {
        JsValue::from_str(&err.to_string())
    }

    fn parse_bindings_json(json: &str) -> std::result::Result<Vec<WebBinding>, JsValue> {
        let value = js_sys::JSON::parse(json).map_err(|err| {
            err.as_string()
                .map(|message| JsValue::from_str(&message))
                .unwrap_or_else(|| JsValue::from_str("failed to parse bindings JSON"))
        })?;
        serde_wasm_bindgen::from_value(value).map_err(js_err)
    }

    #[wasm_bindgen]
    impl Star9System {
        #[wasm_bindgen(constructor)]
        pub fn new() -> std::result::Result<Star9System, JsValue> {
            Star9System::build().map_err(js_err)
        }

        #[wasm_bindgen(js_name = registerFileBytes)]
        pub fn register_file_bytes(
            &self,
            src: &str,
            bytes: &[u8],
        ) -> std::result::Result<(), JsValue> {
            self.register_file_bytes_native(src, bytes).map_err(js_err)
        }

        #[wasm_bindgen(js_name = registerFileText)]
        pub fn register_file_text(
            &self,
            src: &str,
            text: &str,
        ) -> std::result::Result<(), JsValue> {
            self.register_file_text_native(src, text).map_err(js_err)
        }

        #[wasm_bindgen(js_name = registerArchiveBytes)]
        pub fn register_archive_bytes(
            &self,
            src: &str,
            bytes: &[u8],
        ) -> std::result::Result<(), JsValue> {
            self.register_archive_bytes_native(src, bytes)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = readFile)]
        pub fn read_file(&self, path: &str) -> std::result::Result<Vec<u8>, JsValue> {
            self.api.read_file(path).map_err(js_err)
        }

        #[wasm_bindgen(js_name = readText)]
        pub fn read_text(&self, path: &str) -> std::result::Result<String, JsValue> {
            self.read_text_native(path).map_err(js_err)
        }

        #[wasm_bindgen(js_name = writeFile)]
        pub fn write_file(&self, path: &str, data: &[u8]) -> std::result::Result<(), JsValue> {
            self.api.write_file(path, data).map_err(js_err)
        }

        #[wasm_bindgen(js_name = writeText)]
        pub fn write_text(&self, path: &str, value: &str) -> std::result::Result<(), JsValue> {
            self.write_text_native(path, value).map_err(js_err)
        }

        #[wasm_bindgen(js_name = writeExistingFile)]
        pub fn write_existing_file(
            &self,
            path: &str,
            data: &[u8],
        ) -> std::result::Result<(), JsValue> {
            self.write_existing_native(path, data).map_err(js_err)
        }

        #[wasm_bindgen(js_name = writeExistingText)]
        pub fn write_existing_text(
            &self,
            path: &str,
            value: &str,
        ) -> std::result::Result<(), JsValue> {
            self.write_existing_text_native(path, value).map_err(js_err)
        }

        #[wasm_bindgen(js_name = readDir)]
        pub fn read_dir(&self, path: &str) -> std::result::Result<JsValue, JsValue> {
            serde_wasm_bindgen::to_value(&self.read_dir_native(path).map_err(js_err)?)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = stat)]
        pub fn stat(&self, path: &str) -> std::result::Result<JsValue, JsValue> {
            serde_wasm_bindgen::to_value(&self.stat_native(path).map_err(js_err)?).map_err(js_err)
        }

        #[wasm_bindgen(js_name = mkdir)]
        pub fn mkdir(&self, path: &str) -> std::result::Result<(), JsValue> {
            self.mkdir_native(path).map_err(js_err)
        }

        #[wasm_bindgen(js_name = remove)]
        pub fn remove(&self, path: &str) -> std::result::Result<(), JsValue> {
            self.remove_native(path).map_err(js_err)
        }

        #[wasm_bindgen(js_name = bindRamFs)]
        pub fn bind_ramfs(&self, dst: &str) -> std::result::Result<(), JsValue> {
            self.bind_ramfs_native(dst).map_err(js_err)
        }

        #[wasm_bindgen(js_name = mountSelf9p)]
        pub fn mount_self_9p(&self, dst: &str) -> std::result::Result<(), JsValue> {
            self.mount_self_9p_native(dst).map_err(js_err)
        }

        #[wasm_bindgen(js_name = handle9pFrame)]
        pub fn handle_9p_frame(&self, frame: &[u8]) -> std::result::Result<Vec<u8>, JsValue> {
            self.handle_9p_frame_native(frame).map_err(js_err)
        }

        #[wasm_bindgen(js_name = handleRuntimeRequest)]
        pub fn handle_runtime_request(
            &self,
            request: &[u8],
        ) -> std::result::Result<Vec<u8>, JsValue> {
            self.handle_runtime_request_native(request).map_err(js_err)
        }

        #[wasm_bindgen(js_name = handleRuntimeTaskMessage)]
        pub fn handle_runtime_task_message(
            &self,
            message: &[u8],
        ) -> std::result::Result<Vec<u8>, JsValue> {
            self.handle_runtime_task_message_native(message)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = spawnWorker)]
        pub fn spawn_worker(
            &self,
            worker_id: &str,
            parent_task_id: &str,
        ) -> std::result::Result<JsValue, JsValue> {
            let parent = (!parent_task_id.trim().is_empty()).then(|| parent_task_id.to_string());
            serde_wasm_bindgen::to_value(
                &self
                    .spawn_worker_native(worker_id.to_string(), parent)
                    .map_err(js_err)?,
            )
            .map_err(js_err)
        }

        #[wasm_bindgen(js_name = startWorker)]
        pub fn start_worker(
            &self,
            worker: JsValue,
            execution: JsValue,
        ) -> std::result::Result<(), JsValue> {
            let worker: WorkerHandle = serde_wasm_bindgen::from_value(worker).map_err(js_err)?;
            let execution: ExecutionSpec =
                serde_wasm_bindgen::from_value(execution).map_err(js_err)?;
            self.start_worker_native(worker, execution).map_err(js_err)
        }

        #[wasm_bindgen(js_name = openWorkerPort)]
        pub fn open_worker_port(
            &self,
            worker: JsValue,
            port: JsValue,
        ) -> std::result::Result<JsValue, JsValue> {
            let worker: WorkerHandle = serde_wasm_bindgen::from_value(worker).map_err(js_err)?;
            let port: PortDescriptor = serde_wasm_bindgen::from_value(port).map_err(js_err)?;
            serde_wasm_bindgen::to_value(
                &self.open_worker_port_native(worker, port).map_err(js_err)?,
            )
            .map_err(js_err)
        }

        #[wasm_bindgen(js_name = handoffWorkerPort)]
        pub fn handoff_worker_port(
            &self,
            worker: JsValue,
            target_task_id: &str,
            port: JsValue,
        ) -> std::result::Result<JsValue, JsValue> {
            let worker: WorkerHandle = serde_wasm_bindgen::from_value(worker).map_err(js_err)?;
            let port: PortDescriptor = serde_wasm_bindgen::from_value(port).map_err(js_err)?;
            serde_wasm_bindgen::to_value(
                &self
                    .handoff_worker_port_native(worker, target_task_id, port)
                    .map_err(js_err)?,
            )
            .map_err(js_err)
        }

        #[wasm_bindgen(js_name = recordWorkerExit)]
        pub fn record_worker_exit(
            &self,
            task_id: &str,
            worker_id: &str,
            sequence: u64,
            code: i32,
        ) -> std::result::Result<(), JsValue> {
            let worker = (!worker_id.trim().is_empty()).then(|| worker_id.to_string());
            self.post_worker_exit_native(task_id, worker, sequence, code)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = recordWorkerStdout)]
        pub fn record_worker_stdout(
            &self,
            task_id: &str,
            worker_id: &str,
            sequence: u64,
            data: &[u8],
            eof: bool,
        ) -> std::result::Result<(), JsValue> {
            let worker = (!worker_id.trim().is_empty()).then(|| worker_id.to_string());
            self.post_worker_stdout_native(task_id, worker, sequence, data, eof)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = setupNamespace)]
        pub fn setup_namespace(
            &self,
            task_id: &str,
            bindings: JsValue,
        ) -> std::result::Result<(), JsValue> {
            let bindings: Vec<WebBinding> =
                serde_wasm_bindgen::from_value(bindings).map_err(js_err)?;
            self.setup_namespace_native(task_id, &bindings)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = setupNamespaceJson)]
        pub fn setup_namespace_json(
            &self,
            task_id: &str,
            json: &str,
        ) -> std::result::Result<(), JsValue> {
            let bindings = parse_bindings_json(json)?;
            self.setup_namespace_native(task_id, &bindings)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = startTask)]
        pub fn start_task(
            &self,
            kind: &str,
            command: &str,
        ) -> std::result::Result<String, JsValue> {
            self.start_task_native(kind, command).map_err(js_err)
        }

        #[wasm_bindgen(js_name = startWasi)]
        pub fn start_wasi(&self, command: &str) -> std::result::Result<String, JsValue> {
            self.start_wasi_native(command).map_err(js_err)
        }

        #[wasm_bindgen(js_name = startGoJs)]
        pub fn start_gojs(&self, command: &str) -> std::result::Result<String, JsValue> {
            self.start_gojs_native(command).map_err(js_err)
        }

        #[wasm_bindgen(js_name = wasmReady)]
        pub fn wasm_ready(&self) {
            web_sys::console::log_1(&JsValue::from_str("star9-system ready"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use star9_fs::{fs_ref, read_file, write_file, FileMode, MemFs, TarFs};
    use star9_protocol::p9::{
        decode_response, encode_request, LoopbackTransport, NinePRequest, NinePResponse,
        DEFAULT_MSIZE, NOFID, VERSION,
    };

    fn alloc_task(system: &Star9System) -> String {
        let runtime = system.runtime();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        task.id()
    }

    #[test]
    fn native_browser_facade_runs_file_operations_and_tasks() {
        let system = Star9System::new().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system.write_text_native("tmp/hello", "ok").unwrap();
        system.mount_self_9p_native("remote").unwrap();
        assert_eq!(system.read_text_native("remote/tmp/hello").unwrap(), "ok");
        assert!(system
            .read_text_native("#star9/version")
            .unwrap()
            .contains("0.1.0"));
        let wasi = system.start_wasi_native("repl.wasm").unwrap();
        let gojs = system.start_gojs_native("repl-gojs.wasm").unwrap();
        assert_ne!(wasi, gojs);
    }

    #[test]
    fn native_browser_facade_registers_text_and_archive_bytes() {
        let system = Star9System::new().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system
            .register_file_text_native("pkg:text", "hello text")
            .unwrap();

        let source = fs_ref(MemFs::new());
        star9_fs::mkdir_all(source.as_ref(), "nested", FileMode::from_perm(0o755)).unwrap();
        write_file(
            source.as_ref(),
            "nested/data.txt",
            b"archived",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        let mut archive = Vec::new();
        TarFs::archive_to_writer(source.as_ref(), &mut archive).unwrap();
        system
            .register_archive_bytes_native("pkg:archive", &archive)
            .unwrap();

        let task_id = alloc_task(&system);
        let bindings = [
            WebBinding {
                dst: "tmp/payload.txt".to_string(),
                src: Some("pkg:text".to_string()),
                kind: WebBindingKind::File,
                storage: None,
            },
            WebBinding {
                dst: "bundle".to_string(),
                src: Some("pkg:archive".to_string()),
                kind: WebBindingKind::Archive,
                storage: None,
            },
        ];

        system.setup_namespace_native(&task_id, &bindings).unwrap();
        let task = system.runtime().root().lookup(&task_id).unwrap();
        assert_eq!(
            read_file(task.namespace().as_ref(), "tmp/payload.txt").unwrap(),
            b"hello text"
        );
        assert_eq!(
            read_file(task.namespace().as_ref(), "bundle/nested/data.txt").unwrap(),
            b"archived"
        );
    }

    #[test]
    fn native_browser_facade_handles_sequential_9p_frames_with_shared_server_state() {
        let system = Star9System::new().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system.write_text_native("tmp/hello.txt", "ok").unwrap();

        let version = system
            .handle_9p_frame_native(
                &encode_request(
                    1,
                    &NinePRequest::Version {
                        msize: DEFAULT_MSIZE,
                        version: VERSION.to_string(),
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            decode_response(&version).unwrap().1,
            NinePResponse::Version { .. }
        ));

        let attach = system
            .handle_9p_frame_native(
                &encode_request(
                    2,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            decode_response(&attach).unwrap().1,
            NinePResponse::Attach { .. }
        ));

        let walk = system
            .handle_9p_frame_native(
                &encode_request(
                    3,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["tmp".into(), "hello.txt".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();
        match decode_response(&walk).unwrap().1 {
            NinePResponse::Walk { qids } => assert_eq!(qids.len(), 2),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn generic_native_task_start_handles_builtin_execution_kinds() {
        let system = Star9System::new().unwrap();

        let wasi_id = system.start_task_native("wasi", "repl.wasm").unwrap();
        let gojs_id = system.start_task_native("go-js", "repl-gojs.wasm").unwrap();

        let wasi = system.runtime().root().lookup(&wasi_id).unwrap();
        let gojs = system.runtime().root().lookup(&gojs_id).unwrap();

        assert_eq!(wasi.cmd(), "repl.wasm");
        assert_eq!(gojs.cmd(), "repl-gojs.wasm");
        assert_eq!(wasi.exit(), "started");
        assert_eq!(gojs.exit(), "started");
    }

    #[test]
    fn facade_write_existing_targets_device_files_without_create() {
        let system = Star9System::new().unwrap();
        let term_id = system
            .read_text_native("#term/new")
            .unwrap()
            .trim()
            .to_string();
        system
            .write_existing_text_native(&format!("#term/{term_id}/data"), "screen")
            .unwrap();

        assert_eq!(
            system
                .read_text_native(&format!("#term/{term_id}/screen"))
                .unwrap(),
            "screen"
        );
    }

    #[test]
    fn worker_runtime_facade_records_lifecycle_messages_and_ports() {
        use star9_protocol::runtime::{
            EnvironmentEntry, ExecutionKind, ExecutionSpec, PortDescriptor, StdioSet,
        };

        let system = Star9System::new().unwrap();
        let parent = alloc_task(&system);
        let target = alloc_task(&system);

        let worker = system
            .spawn_worker_native("browser-worker", Some(parent.clone()))
            .unwrap();
        system
            .start_worker_native(
                worker.clone(),
                ExecutionSpec {
                    kind: ExecutionKind::JsWasm,
                    module: "runner.mjs".into(),
                    args: vec!["--smoke".into()],
                    env: vec![EnvironmentEntry {
                        name: "FIXTURE_EXIT_CODE".into(),
                        value: "0".into(),
                    }],
                    cwd: Some("tmp".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        let task = system.runtime().root().lookup(&worker.task_id).unwrap();
        assert_eq!(task.parent().unwrap().id(), parent);
        assert_eq!(task.cmd(), "runner.mjs --smoke");
        assert_eq!(task.env(), ["FIXTURE_EXIT_CODE=0"]);
        assert_eq!(task.dir(), "tmp");
        assert_eq!(task.worker().as_deref(), Some("browser-worker"));
        assert_eq!(task.exit(), "started");

        let port = system
            .open_worker_port_native(
                worker.clone(),
                PortDescriptor {
                    port_id: "events".into(),
                    name: "event-bus".into(),
                },
            )
            .unwrap();
        let handed = system
            .handoff_worker_port_native(worker.clone(), target.clone(), port.clone())
            .unwrap();
        assert_eq!(handed, port);

        system
            .post_worker_stdout_native(
                &worker.task_id,
                Some(worker.worker_id.clone()),
                1,
                b"hello\n",
                false,
            )
            .unwrap();
        system
            .post_worker_exit_native(&worker.task_id, Some(worker.worker_id.clone()), 2, 0)
            .unwrap();

        assert_eq!(task.exit(), "0");
        let host = system.runtime().protocol_host();
        let worker_snapshot = host
            .worker_snapshot("browser-worker")
            .unwrap()
            .expect("worker snapshot");
        assert_eq!(
            worker_snapshot.parent_task_id.as_deref(),
            Some(parent.as_str())
        );
        assert_eq!(worker_snapshot.lifecycle, "0");
        let port_snapshot = host.port_snapshot("events").expect("port snapshot");
        assert_eq!(port_snapshot.owner_task_id, target);
        assert_eq!(port_snapshot.handoff_targets, [target]);
        let messages = host
            .task_messages_snapshot(&worker.task_id)
            .expect("task message snapshot");
        assert_eq!(messages.messages.len(), 2);
    }

    #[test]
    fn descriptor_backed_mount_is_writable_from_registered_handle() {
        let system = Star9System::new().unwrap();
        let backing = fs_ref(MemFs::new());
        system
            .storage_registry()
            .register_cache("shell", backing.clone())
            .unwrap();
        let task_id = alloc_task(&system);
        let binding = WebBinding {
            dst: "mnt".to_string(),
            src: None,
            kind: WebBindingKind::Ns,
            storage: Some(WebStorageDescriptor::Cache(CacheStorageDescriptor {
                cache: "shell".to_string(),
                path: None,
            })),
        };

        system.setup_namespace_native(&task_id, &[binding]).unwrap();
        let task = system.runtime().root().lookup(&task_id).unwrap();
        write_file(
            task.namespace().as_ref(),
            "mnt/hello.txt",
            b"hello",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        assert_eq!(read_file(backing.as_ref(), "hello.txt").unwrap(), b"hello");
    }

    #[test]
    fn descriptor_backed_mount_persists_for_same_descriptor_identity() {
        let system = Star9System::new().unwrap();
        let descriptor = WebStorageDescriptor::JsValue(JsValueStorageDescriptor {
            value: "window.fsRoot".to_string(),
            path: None,
        });
        let first_task_id = alloc_task(&system);
        let second_task_id = alloc_task(&system);
        let first_binding = WebBinding {
            dst: "shared".to_string(),
            src: None,
            kind: WebBindingKind::Ns,
            storage: Some(descriptor.clone()),
        };
        let second_binding = WebBinding {
            dst: "shared".to_string(),
            src: None,
            kind: WebBindingKind::Ns,
            storage: Some(descriptor),
        };

        system
            .setup_namespace_native(&first_task_id, &[first_binding])
            .unwrap();
        let first_task = system.runtime().root().lookup(&first_task_id).unwrap();
        write_file(
            first_task.namespace().as_ref(),
            "shared/state.txt",
            b"persisted",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        system
            .setup_namespace_native(&second_task_id, &[second_binding])
            .unwrap();
        let second_task = system.runtime().root().lookup(&second_task_id).unwrap();
        assert_eq!(
            read_file(second_task.namespace().as_ref(), "shared/state.txt").unwrap(),
            b"persisted"
        );
    }

    #[test]
    fn descriptor_backed_mount_respects_descriptor_subpath_rooting() {
        let system = Star9System::new().unwrap();
        let backing = fs_ref(MemFs::new());
        system
            .storage_registry()
            .register_worker_handle("worker-1", backing.clone())
            .unwrap();
        let task_id = alloc_task(&system);
        let binding = WebBinding {
            dst: "worker".to_string(),
            src: None,
            kind: WebBindingKind::Ns,
            storage: Some(WebStorageDescriptor::Worker(WorkerStorageDescriptor {
                worker: "worker-1".to_string(),
                path: Some("nested/root".to_string()),
            })),
        };

        system.setup_namespace_native(&task_id, &[binding]).unwrap();
        let task = system.runtime().root().lookup(&task_id).unwrap();
        write_file(
            task.namespace().as_ref(),
            "worker/file.txt",
            b"subpath",
            FileMode::from_perm(0o644),
        )
        .unwrap();

        assert_eq!(
            read_file(backing.as_ref(), "nested/root/file.txt").unwrap(),
            b"subpath"
        );
    }

    #[test]
    fn file_source_binding_writes_registered_bytes() {
        let system = Star9System::new().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system
            .binding_registry()
            .register_file_bytes("pkg:file", b"hello from registry".to_vec())
            .unwrap();
        let task_id = alloc_task(&system);
        let binding = WebBinding {
            dst: "tmp/payload.txt".to_string(),
            src: Some("pkg:file".to_string()),
            kind: WebBindingKind::File,
            storage: None,
        };

        system.setup_namespace_native(&task_id, &[binding]).unwrap();
        let task = system.runtime().root().lookup(&task_id).unwrap();
        assert_eq!(
            read_file(task.namespace().as_ref(), "tmp/payload.txt").unwrap(),
            b"hello from registry"
        );
    }

    #[test]
    fn archive_source_binding_mounts_registered_tarfs() {
        let system = Star9System::new().unwrap();
        let source = fs_ref(MemFs::new());
        star9_fs::mkdir_all(source.as_ref(), "nested", FileMode::from_perm(0o755)).unwrap();
        write_file(
            source.as_ref(),
            "nested/data.txt",
            b"archived",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        let mut archive = Vec::new();
        TarFs::archive_to_writer(source.as_ref(), &mut archive).unwrap();
        system
            .binding_registry()
            .register_archive_bytes("pkg:archive", archive)
            .unwrap();
        let task_id = alloc_task(&system);
        let binding = WebBinding {
            dst: "bundle".to_string(),
            src: Some("pkg:archive".to_string()),
            kind: WebBindingKind::Archive,
            storage: None,
        };

        system.setup_namespace_native(&task_id, &[binding]).unwrap();
        let task = system.runtime().root().lookup(&task_id).unwrap();
        assert_eq!(
            read_file(task.namespace().as_ref(), "bundle/nested/data.txt").unwrap(),
            b"archived"
        );
    }

    #[test]
    fn import_source_binding_mounts_registered_transport() {
        let system = Star9System::new().unwrap();
        let backing = fs_ref(MemFs::new());
        write_file(
            backing.as_ref(),
            "hello.txt",
            b"remote",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        system
            .binding_registry()
            .register_import_transport(
                "pkg:import",
                Arc::new(LoopbackTransport::with_filesystem(backing.clone())),
            )
            .unwrap();
        let task_id = alloc_task(&system);
        let binding = WebBinding {
            dst: "remote".to_string(),
            src: Some("pkg:import".to_string()),
            kind: WebBindingKind::Import,
            storage: None,
        };

        system.setup_namespace_native(&task_id, &[binding]).unwrap();
        let task = system.runtime().root().lookup(&task_id).unwrap();
        assert_eq!(
            read_file(task.namespace().as_ref(), "remote/hello.txt").unwrap(),
            b"remote"
        );
        write_file(
            task.namespace().as_ref(),
            "remote/writeback.txt",
            b"roundtrip",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        assert_eq!(
            read_file(backing.as_ref(), "writeback.txt").unwrap(),
            b"roundtrip"
        );
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn browser_smoke_runs_file_api_and_execution_adapters() {
        let system = Star9System::build().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system.write_text_native("tmp/hello", "browser-ok").unwrap();
        system.mount_self_9p_native("remote").unwrap();
        assert_eq!(
            system.read_text_native("remote/tmp/hello").unwrap(),
            "browser-ok"
        );
        assert!(system
            .read_text_native("#star9/version")
            .unwrap()
            .contains("0.1.0"));
        let task_id = system.start_wasi_native("repl.wasm").unwrap();
        assert!(!task_id.is_empty());
        let task_id = system.start_gojs_native("repl-gojs.wasm").unwrap();
        assert!(!task_id.is_empty());
    }
}
