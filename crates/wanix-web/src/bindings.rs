use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use wanix_core::{Error, Result};
use wanix_protocol::p9::NinePTransport;

#[derive(Clone, Default)]
pub struct BrowserBindingRegistry {
    state: Arc<RwLock<BrowserBindingRegistryState>>,
}

#[derive(Default)]
struct BrowserBindingRegistryState {
    files: BTreeMap<String, Arc<[u8]>>,
    archives: BTreeMap<String, Arc<[u8]>>,
    imports: BTreeMap<String, Arc<dyn NinePTransport>>,
}

impl BrowserBindingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_file_bytes(
        &self,
        src: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let src = validate_src(src.into())?;
        self.state
            .write()
            .unwrap()
            .files
            .insert(src, Arc::from(bytes.into()));
        Ok(())
    }

    pub fn file_bytes(&self, src: &str) -> Option<Arc<[u8]>> {
        self.state.read().unwrap().files.get(src).cloned()
    }

    pub fn register_archive_bytes(
        &self,
        src: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let src = validate_src(src.into())?;
        self.state
            .write()
            .unwrap()
            .archives
            .insert(src, Arc::from(bytes.into()));
        Ok(())
    }

    pub fn archive_bytes(&self, src: &str) -> Option<Arc<[u8]>> {
        self.state.read().unwrap().archives.get(src).cloned()
    }

    pub fn register_import_transport(
        &self,
        src: impl Into<String>,
        transport: Arc<dyn NinePTransport>,
    ) -> Result<()> {
        let src = validate_src(src.into())?;
        self.state.write().unwrap().imports.insert(src, transport);
        Ok(())
    }

    pub fn import_transport(&self, src: &str) -> Option<Arc<dyn NinePTransport>> {
        self.state.read().unwrap().imports.get(src).cloned()
    }
}

fn validate_src(src: String) -> Result<String> {
    if src.trim().is_empty() {
        Err(Error::Message(
            "invalid binding src: must not be empty".to_string(),
        ))
    } else {
        Ok(src)
    }
}
