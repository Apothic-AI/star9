use serde::{Deserialize, Serialize};
use wanix_core::{valid_path, Error, Result};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebBindingKind {
    #[default]
    Ns,
    File,
    Archive,
    Import,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebBinding {
    pub dst: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    #[serde(default)]
    pub kind: WebBindingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<WebStorageDescriptor>,
}

impl WebBinding {
    pub fn validate(&self) -> Result<()> {
        validate_rel_path("binding dst", &self.dst)?;
        if let (WebBindingKind::Ns, Some(src)) = (self.kind, self.src.as_deref()) {
            validate_rel_path("binding src", src)?;
        }
        match self.kind {
            WebBindingKind::Archive | WebBindingKind::Import if self.src.is_none() => Err(invalid(
                "binding src",
                "is required for archive/import bindings",
            )),
            _ => Ok(()),
        }?;
        if let Some(storage) = &self.storage {
            storage.validate()?;
        }
        Ok(())
    }

    pub fn src_or_default(&self) -> Option<&str> {
        self.src.as_deref().or(match self.kind {
            WebBindingKind::Ns => Some("."),
            WebBindingKind::File | WebBindingKind::Archive | WebBindingKind::Import => None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "kebab-case")]
pub enum WebStorageDescriptor {
    Opfs(OpfsStorageDescriptor),
    FileSystemAccess(FileSystemAccessStorageDescriptor),
    Cache(CacheStorageDescriptor),
    JsValue(JsValueStorageDescriptor),
    Download(DownloadStorageDescriptor),
    Worker(WorkerStorageDescriptor),
    Dom(DomStorageDescriptor),
    Starfs(StarFsStorageDescriptor),
}

impl WebStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Opfs(descriptor) => descriptor.validate(),
            Self::FileSystemAccess(descriptor) => descriptor.validate(),
            Self::Cache(descriptor) => descriptor.validate(),
            Self::JsValue(descriptor) => descriptor.validate(),
            Self::Download(descriptor) => descriptor.validate(),
            Self::Worker(descriptor) => descriptor.validate(),
            Self::Dom(descriptor) => descriptor.validate(),
            Self::Starfs(descriptor) => descriptor.validate(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpfsStorageDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

impl OpfsStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        if let Some(root) = self.root.as_deref() {
            validate_rel_path("opfs root", root)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileSystemAccessStorageDescriptor {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub writable: bool,
}

impl FileSystemAccessStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_non_empty("file-system-access handle", &self.handle)?;
        if let Some(path) = self.path.as_deref() {
            validate_rel_path("file-system-access path", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CacheStorageDescriptor {
    pub cache: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl CacheStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_non_empty("cache name", &self.cache)?;
        if let Some(path) = self.path.as_deref() {
            validate_rel_path("cache path", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JsValueStorageDescriptor {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl JsValueStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_non_empty("js-value handle", &self.value)?;
        if let Some(path) = self.path.as_deref() {
            validate_rel_path("js-value path", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DownloadStorageDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

impl DownloadStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        if let Some(name) = self.name.as_deref() {
            validate_download_name(name)?;
        }
        if let Some(media_type) = self.media_type.as_deref() {
            validate_media_type(media_type)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerStorageDescriptor {
    pub worker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl WorkerStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_non_empty("worker handle", &self.worker)?;
        if let Some(path) = self.path.as_deref() {
            validate_rel_path("worker path", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomStorageDescriptor {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
}

impl DomStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_non_empty("dom node", &self.node)?;
        if let Some(property) = self.property.as_deref() {
            validate_non_empty("dom property", property)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StarFsStorageDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<Box<WebStorageDescriptor>>,
}

impl StarFsStorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        if let Some(id) = self.id.as_deref() {
            validate_non_empty("starfs id", id)?;
        }
        if let Some(root) = self.root.as_deref() {
            validate_rel_path("starfs root", root)?;
        }
        if let Some(storage) = self.storage.as_deref() {
            if matches!(storage, WebStorageDescriptor::Starfs(_)) {
                return Err(invalid("starfs storage", "must not recursively use starfs"));
            }
            storage.validate()?;
        }
        Ok(())
    }
}

fn validate_rel_path(field: &'static str, path: &str) -> Result<()> {
    if path == "." || valid_path(path) {
        Ok(())
    } else {
        Err(invalid(
            field,
            format!("expected a clean relative path, got {path:?}"),
        ))
    }
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(invalid(field, "must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_download_name(name: &str) -> Result<()> {
    validate_non_empty("download name", name)?;
    if name.contains('/') || name.contains('\\') {
        Err(invalid(
            "download name",
            "must be a single file name without path separators",
        ))
    } else {
        Ok(())
    }
}

fn validate_media_type(media_type: &str) -> Result<()> {
    validate_non_empty("download media type", media_type)?;
    if media_type.chars().any(|ch| ch.is_control()) {
        Err(invalid(
            "download media type",
            "must not contain control characters",
        ))
    } else {
        Ok(())
    }
}

fn invalid(field: &'static str, detail: impl Into<String>) -> Error {
    Error::Message(format!("invalid {field}: {}", detail.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ns_binding_defaults_to_current_namespace_source() {
        let binding = WebBinding {
            dst: "mnt".to_string(),
            src: None,
            kind: WebBindingKind::Ns,
            storage: None,
        };

        binding.validate().unwrap();
        assert_eq!(binding.src_or_default(), Some("."));
    }

    #[test]
    fn archive_binding_requires_a_source() {
        let binding = WebBinding {
            dst: "assets".to_string(),
            src: None,
            kind: WebBindingKind::Archive,
            storage: None,
        };

        let err = binding.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message(
                "invalid binding src: is required for archive/import bindings".to_string()
            )
        );
    }

    #[test]
    fn file_and_import_bindings_accept_url_sources() {
        let file = WebBinding {
            dst: "public/app.wasm".to_string(),
            src: Some("https://example.invalid/app.wasm".to_string()),
            kind: WebBindingKind::File,
            storage: None,
        };
        let import = WebBinding {
            dst: "remote".to_string(),
            src: Some("wss://example.invalid/9p".to_string()),
            kind: WebBindingKind::Import,
            storage: None,
        };

        file.validate().unwrap();
        import.validate().unwrap();
    }

    #[test]
    fn binding_rejects_unclean_destination_paths() {
        let binding = WebBinding {
            dst: "/absolute".to_string(),
            src: Some(".".to_string()),
            kind: WebBindingKind::Ns,
            storage: None,
        };

        let err = binding.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message(
                "invalid binding dst: expected a clean relative path, got \"/absolute\""
                    .to_string()
            )
        );
    }

    #[test]
    fn opfs_descriptor_accepts_rooted_relative_plans() {
        let descriptor = WebStorageDescriptor::Opfs(OpfsStorageDescriptor {
            root: Some("workspace/data".to_string()),
        });

        descriptor.validate().unwrap();
    }

    #[test]
    fn file_system_access_descriptor_requires_handle() {
        let descriptor =
            WebStorageDescriptor::FileSystemAccess(FileSystemAccessStorageDescriptor {
                handle: "   ".to_string(),
                path: Some("docs".to_string()),
                writable: true,
            });

        let err = descriptor.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message("invalid file-system-access handle: must not be empty".to_string())
        );
    }

    #[test]
    fn cache_descriptor_rejects_parent_navigation() {
        let descriptor = WebStorageDescriptor::Cache(CacheStorageDescriptor {
            cache: "shell".to_string(),
            path: Some("../escape".to_string()),
        });

        let err = descriptor.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message(
                "invalid cache path: expected a clean relative path, got \"../escape\"".to_string()
            )
        );
    }

    #[test]
    fn download_descriptor_rejects_path_like_names() {
        let descriptor = WebStorageDescriptor::Download(DownloadStorageDescriptor {
            name: Some("dir/file.txt".to_string()),
            media_type: Some("text/plain".to_string()),
        });

        let err = descriptor.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message(
                "invalid download name: must be a single file name without path separators"
                    .to_string()
            )
        );
    }

    #[test]
    fn import_binding_validates_worker_storage_plan() {
        let binding = WebBinding {
            dst: "remote".to_string(),
            src: Some("channel".to_string()),
            kind: WebBindingKind::Import,
            storage: Some(WebStorageDescriptor::Worker(WorkerStorageDescriptor {
                worker: "task-worker-1".to_string(),
                path: Some("imports/root".to_string()),
            })),
        };

        binding.validate().unwrap();
    }

    #[test]
    fn starfs_descriptor_accepts_opfs_backing() {
        let descriptor = WebStorageDescriptor::Starfs(StarFsStorageDescriptor {
            id: Some("agent-a".to_string()),
            root: Some("agents/a".to_string()),
            storage: Some(Box::new(WebStorageDescriptor::Opfs(
                OpfsStorageDescriptor {
                    root: Some("starfs/agent-a".to_string()),
                },
            ))),
        });

        descriptor.validate().unwrap();
    }

    #[test]
    fn starfs_descriptor_rejects_recursive_backing() {
        let descriptor = WebStorageDescriptor::Starfs(StarFsStorageDescriptor {
            id: Some("agent-a".to_string()),
            root: None,
            storage: Some(Box::new(WebStorageDescriptor::Starfs(
                StarFsStorageDescriptor {
                    id: Some("inner".to_string()),
                    root: None,
                    storage: None,
                },
            ))),
        });

        let err = descriptor.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message("invalid starfs storage: must not recursively use starfs".to_string())
        );
    }

    #[test]
    fn dom_descriptor_requires_node_handle() {
        let descriptor = WebStorageDescriptor::Dom(DomStorageDescriptor {
            node: "".to_string(),
            property: Some("files".to_string()),
        });

        let err = descriptor.validate().unwrap_err();
        assert_eq!(
            err,
            Error::Message("invalid dom node: must not be empty".to_string())
        );
    }

    #[test]
    fn js_value_descriptor_accepts_nested_member_paths() {
        let descriptor = WebStorageDescriptor::JsValue(JsValueStorageDescriptor {
            value: "window.fsRoot".to_string(),
            path: Some("children/home".to_string()),
        });

        descriptor.validate().unwrap();
    }

    #[test]
    fn browser_binding_fixture_validates_representative_plans() {
        let bindings: Vec<WebBinding> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/browser-bindings.json"
        ))
        .unwrap();

        assert_eq!(bindings.len(), 11);
        for binding in bindings {
            binding.validate().unwrap();
        }
    }
}
