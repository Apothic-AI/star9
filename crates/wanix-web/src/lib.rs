//! Browser/WASM entry points for the Rust Wanix runtime.

mod descriptors;
pub mod p9_transport;
mod storage;

use wanix_core::Result;
use wanix_protocol::WanixApi;
use wanix_runtime::{ExecutionAdapter, Runtime};
use wanix_vfs::BindMode;

pub use descriptors::*;
pub use storage::*;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct WanixSystem {
    runtime: Runtime,
    api: WanixApi,
    storage_registry: BrowserStorageRegistry,
}

impl WanixSystem {
    fn build() -> Result<Self> {
        let runtime = Runtime::new()?;
        let api = WanixApi::new(runtime.root());
        let storage_registry = BrowserStorageRegistry::new();
        Ok(Self {
            runtime,
            api,
            storage_registry,
        })
    }

    pub fn runtime(&self) -> Runtime {
        self.runtime.clone()
    }

    pub fn api(&self) -> WanixApi {
        self.api.clone()
    }

    pub fn storage_registry(&self) -> BrowserStorageRegistry {
        self.storage_registry.clone()
    }

    pub fn read_text_native(&self, path: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.api.read_file(path)?).into_owned())
    }

    pub fn write_text_native(&self, path: &str, value: &str) -> Result<()> {
        self.api.write_file(path, value.as_bytes())
    }

    pub fn read_dir_native(&self, path: &str) -> Result<Vec<String>> {
        self.api.read_dir(path)
    }

    pub fn bind_ramfs_native(&self, dst: &str) -> Result<()> {
        self.runtime.namespace().bind(
            wanix_fs::fs_ref(wanix_fs::MemFs::new()),
            ".",
            dst,
            BindMode::Replace,
        )
    }

    pub fn mount_self_9p_native(&self, dst: &str) -> Result<()> {
        let server = self.runtime.export_9p();
        self.runtime
            .import_9p_loopback(dst, server, BindMode::Replace)?;
        Ok(())
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
                    self.api.write_file(&binding.dst, b"")?;
                }
                WebBindingKind::Archive | WebBindingKind::Import => {
                    return Err(wanix_core::ErrorKind::NotSupported.into());
                }
            }
        }
        Ok(())
    }

    pub fn start_wasi_native(&self, command: &str) -> Result<String> {
        let task = self
            .runtime
            .task_fs()
            .alloc("auto", Some(self.runtime.root()))?;
        ExecutionAdapter::wasi(command).start(&task)?;
        Ok(task.id())
    }

    pub fn start_gojs_native(&self, command: &str) -> Result<String> {
        let task = self
            .runtime
            .task_fs()
            .alloc("auto", Some(self.runtime.root()))?;
        ExecutionAdapter::go_js(command).start(&task)?;
        Ok(task.id())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl WanixSystem {
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

    #[wasm_bindgen]
    impl WanixSystem {
        #[wasm_bindgen(constructor)]
        pub fn new() -> std::result::Result<WanixSystem, JsValue> {
            WanixSystem::build().map_err(js_err)
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

        #[wasm_bindgen(js_name = readDir)]
        pub fn read_dir(&self, path: &str) -> std::result::Result<JsValue, JsValue> {
            serde_wasm_bindgen::to_value(&self.read_dir_native(path).map_err(js_err)?)
                .map_err(js_err)
        }

        #[wasm_bindgen(js_name = bindRamFs)]
        pub fn bind_ramfs(&self, dst: &str) -> std::result::Result<(), JsValue> {
            self.bind_ramfs_native(dst).map_err(js_err)
        }

        #[wasm_bindgen(js_name = mountSelf9p)]
        pub fn mount_self_9p(&self, dst: &str) -> std::result::Result<(), JsValue> {
            self.mount_self_9p_native(dst).map_err(js_err)
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
            web_sys::console::log_1(&JsValue::from_str("wanix-system ready"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{fs_ref, read_file, write_file, FileMode, MemFs};

    fn alloc_task(system: &WanixSystem) -> String {
        let runtime = system.runtime();
        runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap()
            .id()
    }

    #[test]
    fn native_browser_facade_runs_file_operations_and_tasks() {
        let system = WanixSystem::new().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system.write_text_native("tmp/hello", "ok").unwrap();
        system.mount_self_9p_native("remote").unwrap();
        assert_eq!(system.read_text_native("remote/tmp/hello").unwrap(), "ok");
        assert!(system
            .read_text_native("#wanix/version")
            .unwrap()
            .contains("0.1.0"));
        let wasi = system.start_wasi_native("repl.wasm").unwrap();
        let gojs = system.start_gojs_native("repl-gojs.wasm").unwrap();
        assert_ne!(wasi, gojs);
    }

    #[test]
    fn descriptor_backed_mount_is_writable_from_registered_handle() {
        let system = WanixSystem::new().unwrap();
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
        let system = WanixSystem::new().unwrap();
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
        let system = WanixSystem::new().unwrap();
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
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn browser_smoke_runs_file_api_and_execution_adapters() {
        let system = WanixSystem::build().unwrap();
        system.bind_ramfs_native("tmp").unwrap();
        system.write_text_native("tmp/hello", "browser-ok").unwrap();
        system.mount_self_9p_native("remote").unwrap();
        assert_eq!(
            system.read_text_native("remote/tmp/hello").unwrap(),
            "browser-ok"
        );
        assert!(system
            .read_text_native("#wanix/version")
            .unwrap()
            .contains("0.1.0"));
        let task_id = system.start_wasi_native("repl.wasm").unwrap();
        assert!(!task_id.is_empty());
        let task_id = system.start_gojs_native("repl-gojs.wasm").unwrap();
        assert!(!task_id.is_empty());
    }
}
