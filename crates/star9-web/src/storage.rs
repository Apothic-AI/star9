use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use star9_core::{FileMode, Result};
use star9_fs::{fs_ref, mkdir_all, FsRef, MemFs};
use star9_vfs::{BindMode, Namespace};

use crate::{
    CacheStorageDescriptor, DomStorageDescriptor, DownloadStorageDescriptor,
    FileSystemAccessStorageDescriptor, JsValueStorageDescriptor, OpfsStorageDescriptor,
    StarFsStorageDescriptor, WebStorageDescriptor, WorkerStorageDescriptor,
};

#[derive(Clone, Default)]
pub struct BrowserStorageRegistry {
    state: Arc<RwLock<BrowserStorageRegistryState>>,
}

#[derive(Default)]
struct BrowserStorageRegistryState {
    opfs: Option<FsRef>,
    file_system_access: BTreeMap<String, FsRef>,
    caches: BTreeMap<String, FsRef>,
    js_values: BTreeMap<String, FsRef>,
    downloads: BTreeMap<DownloadBucketKey, FsRef>,
    workers: BTreeMap<String, FsRef>,
    dom: BTreeMap<DomStorageKey, FsRef>,
    starfs: BTreeMap<String, FsRef>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DownloadBucketKey {
    name: Option<String>,
    media_type: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DomStorageKey {
    node: String,
    property: Option<String>,
}

impl BrowserStorageRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_opfs_root(&self, fs: FsRef) {
        self.state.write().unwrap().opfs = Some(fs);
    }

    pub fn resolve_opfs_root(&self, descriptor: &OpfsStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let fs = {
            let mut state = self.state.write().unwrap();
            state
                .opfs
                .get_or_insert_with(|| fs_ref(MemFs::new()))
                .clone()
        };
        rooted_view(fs, descriptor.root.as_deref())
    }

    pub fn register_file_system_access_handle(
        &self,
        handle: impl Into<String>,
        fs: FsRef,
    ) -> Result<()> {
        let handle = handle.into();
        FileSystemAccessStorageDescriptor {
            handle: handle.clone(),
            path: None,
            writable: true,
        }
        .validate()?;
        self.state
            .write()
            .unwrap()
            .file_system_access
            .insert(handle, fs);
        Ok(())
    }

    pub fn resolve_file_system_access(
        &self,
        descriptor: &FileSystemAccessStorageDescriptor,
    ) -> Result<FsRef> {
        descriptor.validate()?;
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.file_system_access, descriptor.handle.clone())
        };
        rooted_view(fs, descriptor.path.as_deref())
    }

    pub fn register_cache(&self, cache: impl Into<String>, fs: FsRef) -> Result<()> {
        let cache = cache.into();
        CacheStorageDescriptor {
            cache: cache.clone(),
            path: None,
        }
        .validate()?;
        self.state.write().unwrap().caches.insert(cache, fs);
        Ok(())
    }

    pub fn resolve_cache(&self, descriptor: &CacheStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.caches, descriptor.cache.clone())
        };
        rooted_view(fs, descriptor.path.as_deref())
    }

    pub fn register_js_value(&self, value: impl Into<String>, fs: FsRef) -> Result<()> {
        let value = value.into();
        JsValueStorageDescriptor {
            value: value.clone(),
            path: None,
        }
        .validate()?;
        self.state.write().unwrap().js_values.insert(value, fs);
        Ok(())
    }

    pub fn resolve_js_value(&self, descriptor: &JsValueStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.js_values, descriptor.value.clone())
        };
        rooted_view(fs, descriptor.path.as_deref())
    }

    pub fn register_download_bucket(
        &self,
        name: Option<String>,
        media_type: Option<String>,
        fs: FsRef,
    ) -> Result<()> {
        DownloadStorageDescriptor {
            name: name.clone(),
            media_type: media_type.clone(),
        }
        .validate()?;
        self.state
            .write()
            .unwrap()
            .downloads
            .insert(DownloadBucketKey { name, media_type }, fs);
        Ok(())
    }

    pub fn resolve_download(&self, descriptor: &DownloadStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let key = DownloadBucketKey {
            name: descriptor.name.clone(),
            media_type: descriptor.media_type.clone(),
        };
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.downloads, key)
        };
        Ok(fs)
    }

    pub fn register_worker_handle(&self, worker: impl Into<String>, fs: FsRef) -> Result<()> {
        let worker = worker.into();
        WorkerStorageDescriptor {
            worker: worker.clone(),
            path: None,
        }
        .validate()?;
        self.state.write().unwrap().workers.insert(worker, fs);
        Ok(())
    }

    pub fn resolve_worker(&self, descriptor: &WorkerStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.workers, descriptor.worker.clone())
        };
        rooted_view(fs, descriptor.path.as_deref())
    }

    pub fn register_dom_handle(
        &self,
        node: impl Into<String>,
        property: Option<String>,
        fs: FsRef,
    ) -> Result<()> {
        let node = node.into();
        DomStorageDescriptor {
            node: node.clone(),
            property: property.clone(),
        }
        .validate()?;
        self.state
            .write()
            .unwrap()
            .dom
            .insert(DomStorageKey { node, property }, fs);
        Ok(())
    }

    pub fn resolve_dom(&self, descriptor: &DomStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let key = DomStorageKey {
            node: descriptor.node.clone(),
            property: descriptor.property.clone(),
        };
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.dom, key)
        };
        Ok(fs)
    }

    pub fn resolve_starfs(&self, descriptor: &StarFsStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        let key = descriptor
            .id
            .clone()
            .or_else(|| descriptor.root.clone())
            .unwrap_or_else(|| "default".to_string());
        let fs = {
            let mut state = self.state.write().unwrap();
            get_or_insert_memfs(&mut state.starfs, key)
        };
        rooted_view(fs, descriptor.root.as_deref())
    }

    pub fn resolve(&self, descriptor: &WebStorageDescriptor) -> Result<FsRef> {
        descriptor.validate()?;
        match descriptor {
            WebStorageDescriptor::Opfs(descriptor) => self.resolve_opfs_root(descriptor),
            WebStorageDescriptor::FileSystemAccess(descriptor) => {
                self.resolve_file_system_access(descriptor)
            }
            WebStorageDescriptor::Cache(descriptor) => self.resolve_cache(descriptor),
            WebStorageDescriptor::JsValue(descriptor) => self.resolve_js_value(descriptor),
            WebStorageDescriptor::Download(descriptor) => self.resolve_download(descriptor),
            WebStorageDescriptor::Worker(descriptor) => self.resolve_worker(descriptor),
            WebStorageDescriptor::Dom(descriptor) => self.resolve_dom(descriptor),
            WebStorageDescriptor::Starfs(descriptor) => self.resolve_starfs(descriptor),
        }
    }
}

fn get_or_insert_memfs<K>(map: &mut BTreeMap<K, FsRef>, key: K) -> FsRef
where
    K: Ord,
{
    map.entry(key)
        .or_insert_with(|| fs_ref(MemFs::new()))
        .clone()
}

fn rooted_view(fs: FsRef, root: Option<&str>) -> Result<FsRef> {
    let root = root.unwrap_or(".");
    if root == "." {
        return Ok(fs);
    }

    mkdir_all(fs.as_ref(), root, FileMode::from_perm(0o755))?;
    let view = Namespace::new();
    view.bind(fs, root, ".", BindMode::Replace)?;
    Ok(fs_ref(view))
}
