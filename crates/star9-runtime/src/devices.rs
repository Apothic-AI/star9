use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use star9_core::{clean_path, valid_path, DirEntry, Error, ErrorKind, FsContext, Metadata, Result};
use star9_fs::{
    directory_file, fs_ref, BoxFile, ControlFile, FileHandle, FileSystem, FsRef, MapFs, MemFs,
    SignalFs,
};
use star9_task::Task;
use star9_vfs::BindMode;

#[derive(Clone)]
pub(crate) struct RuntimeDevices {
    vm: VmDevice,
}

impl RuntimeDevices {
    pub(crate) fn set_vm_guest(&self, vm_id: &str, guest: FsRef) -> Result<()> {
        self.vm.set_guest(vm_id, guest)
    }

    pub(crate) fn set_vm_provider(&self, provider: Arc<dyn VmProvider>) {
        self.vm.set_provider(provider);
    }
}

pub(crate) fn bind_devices(root: &Task) -> Result<RuntimeDevices> {
    let vm = VmDevice::new();
    let devices: [(&str, FsRef); 11] = [
        (
            "#pipe",
            fs_ref(DeviceAllocator::new("pipe", |_| {
                fs_ref(star9_fs::PipeFs::new(false))
            })),
        ),
        (
            "#signal",
            fs_ref(DeviceAllocator::new("signal", |_| {
                fs_ref(star9_fs::SignalFs::default())
            })),
        ),
        (
            "#ramfs",
            fs_ref(DeviceAllocator::new("ramfs", |_| fs_ref(MemFs::new()))),
        ),
        ("#term", fs_ref(DeviceAllocator::new("term", terminal_fs))),
        ("#vm", fs_ref(vm.clone())),
        (
            "#worker",
            fs_ref(DeviceAllocator::new("worker", |_| worker_fs())),
        ),
        ("#web", fs_ref(web_fs())),
        ("#js", fs_ref(js_value_fs())),
        (
            "#cache",
            fs_ref(DeviceAllocator::new("cache", |_| fs_ref(MemFs::new()))),
        ),
        (
            "#download",
            fs_ref(DeviceAllocator::new("download", |_| download_fs())),
        ),
        ("#net", fs_ref(NetDevice::new())),
    ];
    for (dst, fs) in devices {
        root.namespace().bind(fs, ".", dst, BindMode::Replace)?;
    }
    Ok(RuntimeDevices { vm })
}

#[derive(Clone)]
pub(crate) struct DeviceAllocator {
    kind: String,
    state: Arc<DeviceAllocatorState>,
}

struct DeviceAllocatorState {
    next_id: Mutex<u32>,
    resources: RwLock<BTreeMap<String, FsRef>>,
    factory: Box<dyn Fn(&str) -> FsRef + Send + Sync>,
}

impl DeviceAllocator {
    pub(crate) fn new(
        kind: impl Into<String>,
        factory: impl Fn(&str) -> FsRef + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind: kind.into(),
            state: Arc::new(DeviceAllocatorState {
                next_id: Mutex::new(0),
                resources: RwLock::new(BTreeMap::new()),
                factory: Box::new(factory),
            }),
        }
    }

    fn get(&self, id: &str) -> Option<FsRef> {
        self.state.resources.read().unwrap().get(id).cloned()
    }

    fn alloc(&self) -> String {
        let mut next = self.state.next_id.lock().unwrap();
        *next += 1;
        let id = next.to_string();
        let resource = (self.state.factory)(&id);
        self.state
            .resources
            .write()
            .unwrap()
            .insert(id.clone(), resource);
        id
    }
}

impl FileSystem for DeviceAllocator {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if name == "." {
            let mut entries = vec![DirEntry::new("new", Metadata::file("new", 0o555, 0))];
            entries.extend(
                self.state
                    .resources
                    .read()
                    .unwrap()
                    .keys()
                    .map(|id| DirEntry::new(id.clone(), Metadata::dir(id.clone(), 0o555))),
            );
            return Ok(directory_file(Metadata::dir(".", 0o555), entries));
        }
        if name == "new" {
            return Ok(Box::new(NewDeviceHandle {
                allocator: self.clone(),
                data: None,
                offset: 0,
            }));
        }
        let (head, rest) = name.split_once('/').unwrap_or((name.as_str(), "."));
        let resource = self
            .get(head)
            .ok_or_else(|| Error::path("open", head, ErrorKind::NotFound))?;
        resource.open(ctx, rest)
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct NewDeviceHandle {
    allocator: DeviceAllocator,
    data: Option<Vec<u8>>,
    offset: u64,
}

impl FileHandle for NewDeviceHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.data.is_none() {
            self.data = Some(format!("{}\n", self.allocator.alloc()).into_bytes());
        }
        let data = self.data.as_ref().unwrap();
        let start = self.offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file(
            format!("new-{}", self.allocator.kind),
            0o555,
            0,
        ))
    }
}

fn terminal_fs(id: &str) -> FsRef {
    let state = SharedTextFile::readonly("state", "ready");
    let size = SharedTextFile::writable("size", "80x24");
    let screen = SharedLogFile::new("screen");
    let data = TerminalDataFile::new("data", screen.clone());
    let program = QueueFile::program("program");
    let raw = QueueFile::loopback("raw");
    let winch = SignalFs::default();

    let ctl_state = state.clone();
    let ctl_size = size.clone();
    let ctl_data = data.clone();
    let ctl_program = program.clone();
    let ctl_raw = raw.clone();
    let ctl = ControlFile::new("ctl", move |cmd| match cmd {
        "clear" => {
            ctl_data.clear();
            ctl_program.clear();
            ctl_raw.clear();
            Ok(())
        }
        "reset" => {
            ctl_data.clear();
            ctl_program.clear();
            ctl_raw.clear();
            ctl_size.set("80x24");
            ctl_state.set("ready");
            Ok(())
        }
        "noop" => Ok(()),
        _ => Ok(()),
    });

    let fs = MapFs::new();
    fs.insert("id", fs_ref(SharedTextFile::readonly("id", id)));
    fs.insert("ctl", fs_ref(ctl));
    fs.insert("state", fs_ref(state));
    fs.insert("size", fs_ref(size));
    fs.insert("data", fs_ref(data));
    fs.insert("screen", fs_ref(screen));
    fs.insert("program", fs_ref(program));
    fs.insert("raw", fs_ref(raw));
    fs.insert("winch", fs_ref(winch));
    fs_ref(fs)
}

#[derive(Clone)]
struct VmDevice {
    state: Arc<VmDeviceState>,
}

struct VmDeviceState {
    next_id: Mutex<u32>,
    resources: RwLock<BTreeMap<String, VmResource>>,
    aliases: RwLock<BTreeMap<String, String>>,
    provider: RwLock<Arc<dyn VmProvider>>,
}

impl Default for VmDeviceState {
    fn default() -> Self {
        Self {
            next_id: Mutex::new(0),
            resources: RwLock::new(BTreeMap::new()),
            aliases: RwLock::new(BTreeMap::new()),
            provider: RwLock::new(Arc::new(DeterministicVmProvider)),
        }
    }
}

impl Default for VmDevice {
    fn default() -> Self {
        Self {
            state: Arc::new(VmDeviceState::default()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VmProviderResource {
    pub id: String,
    pub kind: String,
    pub alias: String,
    pub config: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct VmProviderUpdate {
    pub state: Option<String>,
    pub console_lines: Vec<String>,
    pub clear_console: bool,
}

impl VmProviderUpdate {
    pub fn state(state: impl Into<String>) -> Self {
        Self {
            state: Some(state.into()),
            console_lines: Vec::new(),
            clear_console: false,
        }
    }
}

pub trait VmProvider: Send + Sync {
    fn start(&self, resource: &VmProviderResource) -> Result<VmProviderUpdate>;
    fn stop(&self, resource: &VmProviderResource) -> Result<VmProviderUpdate>;
    fn reset(&self, resource: &VmProviderResource) -> Result<VmProviderUpdate>;
}

#[derive(Default)]
pub struct DeterministicVmProvider;

impl VmProvider for DeterministicVmProvider {
    fn start(&self, _resource: &VmProviderResource) -> Result<VmProviderUpdate> {
        Ok(VmProviderUpdate {
            state: Some("running".to_string()),
            console_lines: vec!["start".to_string()],
            clear_console: false,
        })
    }

    fn stop(&self, _resource: &VmProviderResource) -> Result<VmProviderUpdate> {
        Ok(VmProviderUpdate {
            state: Some("stopped".to_string()),
            console_lines: vec!["stop".to_string()],
            clear_console: false,
        })
    }

    fn reset(&self, _resource: &VmProviderResource) -> Result<VmProviderUpdate> {
        Ok(VmProviderUpdate {
            state: Some("created".to_string()),
            console_lines: Vec::new(),
            clear_console: true,
        })
    }
}

impl VmDevice {
    fn new() -> Self {
        Self::default()
    }

    fn alloc(&self, kind: &str) -> Result<String> {
        let kind = kind.trim();
        if kind.is_empty() {
            return Err(ErrorKind::Invalid.into());
        }
        let mut next = self.state.next_id.lock().unwrap();
        *next += 1;
        let id = next.to_string();
        drop(next);
        let resource = VmResource::new(id.clone(), kind.to_string(), self.clone());
        self.state
            .resources
            .write()
            .unwrap()
            .insert(id.clone(), resource);
        Ok(id)
    }

    fn resolve(&self, name: &str) -> Result<VmResource> {
        if let Some(resource) = self.state.resources.read().unwrap().get(name).cloned() {
            return Ok(resource);
        }
        let id = self
            .state
            .aliases
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::path("open", name, ErrorKind::NotFound))?;
        self.state
            .resources
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::path("open", name, ErrorKind::NotFound))
    }

    fn update_alias(&self, id: &str, old: &str, new: &str) {
        let mut aliases = self.state.aliases.write().unwrap();
        if !old.is_empty() {
            aliases.remove(old);
        }
        if !new.is_empty() {
            aliases.insert(new.to_string(), id.to_string());
        }
    }

    fn set_guest(&self, id: &str, guest: FsRef) -> Result<()> {
        self.resolve(id)?.set_guest(guest);
        Ok(())
    }

    fn set_provider(&self, provider: Arc<dyn VmProvider>) {
        *self.state.provider.write().unwrap() = provider;
    }

    fn provider(&self) -> Arc<dyn VmProvider> {
        self.state.provider.read().unwrap().clone()
    }
}

impl FileSystem for VmDevice {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if name == "." {
            let mut entries = vec![DirEntry::new("new", Metadata::file("new", 0o555, 0))];
            entries.extend(
                self.state
                    .resources
                    .read()
                    .unwrap()
                    .keys()
                    .map(|id| DirEntry::new(id.clone(), Metadata::dir(id.clone(), 0o555))),
            );
            entries.extend(
                self.state
                    .aliases
                    .read()
                    .unwrap()
                    .keys()
                    .map(|alias| DirEntry::new(alias.clone(), Metadata::dir(alias.clone(), 0o555))),
            );
            return Ok(directory_file(Metadata::dir(".", 0o555), entries));
        }
        if name == "new" {
            return Ok(Box::new(NewVmHandle {
                device: self.clone(),
                kind: "vm".to_string(),
                data: None,
                offset: 0,
            }));
        }
        if let Some(kind) = name.strip_prefix("new/") {
            return Ok(Box::new(NewVmHandle {
                device: self.clone(),
                kind: kind.to_string(),
                data: None,
                offset: 0,
            }));
        }
        let (head, rest) = name.split_once('/').unwrap_or((name.as_str(), "."));
        self.resolve(head)?.open(ctx, rest)
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct NewVmHandle {
    device: VmDevice,
    kind: String,
    data: Option<Vec<u8>>,
    offset: u64,
}

impl FileHandle for NewVmHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.data.is_none() {
            let id = self.device.alloc(&self.kind)?;
            self.data = Some(format!("{id}\n").into_bytes());
        }
        let data = self.data.as_ref().unwrap();
        let start = self.offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file("new-vm", 0o555, 0))
    }
}

#[derive(Clone)]
struct VmResource {
    id: String,
    kind: String,
    device: VmDevice,
    state: SharedTextFile,
    alias: SharedTextFile,
    config: SharedTextFile,
    console: SharedLogFile,
    guest: Arc<RwLock<Option<FsRef>>>,
}

impl VmResource {
    fn new(id: String, kind: String, device: VmDevice) -> Self {
        Self {
            id,
            kind,
            device,
            state: SharedTextFile::readonly("state", "created"),
            alias: SharedTextFile::writable("alias", ""),
            config: SharedTextFile::writable("config", ""),
            console: SharedLogFile::new("console"),
            guest: Arc::new(RwLock::new(None)),
        }
    }

    fn set_alias(&self, value: impl Into<String>) {
        let next = value.into().trim().to_string();
        let old = self.alias.get();
        self.alias.set(next.clone());
        self.device.update_alias(&self.id, &old, &next);
    }

    fn set_guest(&self, guest: FsRef) {
        *self.guest.write().unwrap() = Some(guest);
    }

    fn guest(&self) -> Option<FsRef> {
        self.guest.read().unwrap().clone()
    }

    fn provider_resource(&self) -> VmProviderResource {
        VmProviderResource {
            id: self.id.clone(),
            kind: self.kind.clone(),
            alias: self.alias.get(),
            config: self.config.get(),
        }
    }

    fn apply_provider_update(&self, update: VmProviderUpdate) {
        if update.clear_console {
            self.console.clear();
        }
        if let Some(state) = update.state {
            self.state.set(state);
        }
        for line in update.console_lines {
            self.console.append_line(&line);
        }
    }
}

impl FileSystem for VmResource {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name == "guest" || name.starts_with("guest/") {
            let guest = self
                .guest()
                .ok_or_else(|| Error::path("open", name, ErrorKind::NotFound))?;
            let rel = name
                .strip_prefix("guest/")
                .map(clean_path)
                .unwrap_or_else(|| ".".to_string());
            return guest.open(ctx, &rel);
        }
        match name {
            "." => {
                let mut entries = vec![
                    DirEntry::new("id", Metadata::file("id", 0o444, self.id.len() as u64 + 1)),
                    DirEntry::new(
                        "kind",
                        Metadata::file("kind", 0o444, self.kind.len() as u64 + 1),
                    ),
                    DirEntry::new("ctl", Metadata::file("ctl", 0o222, 0)),
                    DirEntry::new(
                        "state",
                        Metadata::file("state", 0o444, self.state.get().len() as u64 + 1),
                    ),
                    DirEntry::new(
                        "alias",
                        Metadata::file("alias", 0o666, self.alias.get().len() as u64 + 1),
                    ),
                    DirEntry::new(
                        "config",
                        Metadata::file("config", 0o666, self.config.get().len() as u64 + 1),
                    ),
                    DirEntry::new(
                        "console",
                        Metadata::file(
                            "console",
                            0o666,
                            self.console.data.lock().unwrap().len() as u64,
                        ),
                    ),
                ];
                if self.guest().is_some() {
                    entries.push(DirEntry::new("guest", Metadata::dir("guest", 0o555)));
                }
                Ok(directory_file(Metadata::dir(".", 0o555), entries))
            }
            "id" => SharedTextFile::readonly("id", self.id.clone()).open(&FsContext::new(), "."),
            "kind" => {
                SharedTextFile::readonly("kind", self.kind.clone()).open(&FsContext::new(), ".")
            }
            "ctl" => {
                let resource = self.clone();
                ControlFile::new("ctl", move |cmd| {
                    let mut parts = cmd.split_whitespace();
                    match parts.next() {
                        Some("start") => {
                            let update = resource
                                .device
                                .provider()
                                .start(&resource.provider_resource())?;
                            resource.apply_provider_update(update);
                            Ok(())
                        }
                        Some("stop") => {
                            let update = resource
                                .device
                                .provider()
                                .stop(&resource.provider_resource())?;
                            resource.apply_provider_update(update);
                            Ok(())
                        }
                        Some("reset") => {
                            let update = resource
                                .device
                                .provider()
                                .reset(&resource.provider_resource())?;
                            resource.apply_provider_update(update);
                            Ok(())
                        }
                        Some("alias") => {
                            resource.set_alias(parts.collect::<Vec<_>>().join(" "));
                            Ok(())
                        }
                        Some("config") => {
                            resource.config.set(parts.collect::<Vec<_>>().join(" "));
                            Ok(())
                        }
                        Some("noop") | None => Ok(()),
                        Some(_) => Err(ErrorKind::Invalid.into()),
                    }
                })
                .open(&FsContext::new(), ".")
            }
            "state" => self.state.open(&FsContext::new(), "."),
            "alias" => Ok(Box::new(VmAliasHandle::new(self.clone()))),
            "config" => self.config.open(&FsContext::new(), "."),
            "console" => self.console.open(&FsContext::new(), "."),
            _ => Err(Error::path("open", name, ErrorKind::NotFound)),
        }
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct VmAliasHandle {
    resource: VmResource,
    data: Vec<u8>,
    offset: u64,
    dirty: Vec<u8>,
}

impl VmAliasHandle {
    fn new(resource: VmResource) -> Self {
        Self {
            data: format!("{}\n", resource.alias.get()).into_bytes(),
            resource,
            offset: 0,
            dirty: Vec::new(),
        }
    }
}

impl FileHandle for VmAliasHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let start = self.offset as usize;
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.dirty.extend_from_slice(data);
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file(
            "alias",
            0o666,
            self.resource.alias.get().len() as u64 + 1,
        ))
    }

    fn close(&mut self) -> Result<()> {
        if !self.dirty.is_empty() {
            self.resource
                .set_alias(String::from_utf8_lossy(&self.dirty).trim().to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct NetDevice {
    state: Arc<NetDeviceState>,
}

#[derive(Default)]
struct NetDeviceState {
    next_id: Mutex<u32>,
    next_port: Mutex<u32>,
    resources: RwLock<BTreeMap<String, NetConn>>,
    listeners: RwLock<BTreeMap<String, String>>,
}

impl NetDevice {
    fn new() -> Self {
        Self::default()
    }

    fn alloc(&self) -> String {
        let mut next = self.state.next_id.lock().unwrap();
        *next += 1;
        let id = next.to_string();
        drop(next);
        let conn = NetConn::new(id.clone(), self.clone());
        self.state
            .resources
            .write()
            .unwrap()
            .insert(id.clone(), conn);
        id
    }

    fn lookup(&self, id: &str) -> Result<NetConn> {
        self.state
            .resources
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::path("open", id, ErrorKind::NotFound))
    }

    fn alloc_connected(&self, local: String, remote: String, endpoint: NetEndpoint) -> String {
        let id = self.alloc();
        let conn = self.lookup(&id).expect("allocated connection exists");
        conn.install_accepted(local, remote, endpoint);
        id
    }

    fn next_auto_local(&self) -> String {
        let mut next = self.state.next_port.lock().unwrap();
        *next += 1;
        format!("local:{}", 10_000 + *next)
    }

    fn register_listener(&self, addr: &str, id: &str) -> Result<()> {
        let mut listeners = self.state.listeners.write().unwrap();
        if listeners.contains_key(addr) {
            return Err(Error::Message(format!(
                "announce: address already in use: {addr}"
            )));
        }
        listeners.insert(addr.to_string(), id.to_string());
        Ok(())
    }

    fn unregister_listener(&self, addr: &str, id: &str) {
        let mut listeners = self.state.listeners.write().unwrap();
        if listeners.get(addr).is_some_and(|current| current == id) {
            listeners.remove(addr);
        }
    }

    fn listener(&self, addr: &str) -> Option<NetConn> {
        let id = self.state.listeners.read().unwrap().get(addr).cloned()?;
        self.lookup(&id).ok()
    }
}

impl FileSystem for NetDevice {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let name = clean_path(name);
        if name == "." {
            let mut entries = vec![DirEntry::new("new", Metadata::file("new", 0o555, 0))];
            entries.extend(
                self.state
                    .resources
                    .read()
                    .unwrap()
                    .keys()
                    .map(|id| DirEntry::new(id.clone(), Metadata::dir(id.clone(), 0o555))),
            );
            return Ok(directory_file(Metadata::dir(".", 0o555), entries));
        }
        if name == "new" {
            return Ok(Box::new(NewNetHandle {
                device: self.clone(),
                data: None,
                offset: 0,
            }));
        }
        let (head, rest) = name.split_once('/').unwrap_or((name.as_str(), "."));
        self.lookup(head)?.open(ctx, rest)
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct NewNetHandle {
    device: NetDevice,
    data: Option<Vec<u8>>,
    offset: u64,
}

impl FileHandle for NewNetHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.data.is_none() {
            self.data = Some(format!("{}\n", self.device.alloc()).into_bytes());
        }
        let data = self.data.as_ref().unwrap();
        let start = self.offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file("new-net", 0o555, 0))
    }
}

#[derive(Clone)]
struct NetConn {
    id: String,
    device: NetDevice,
    inner: Arc<Mutex<NetConnState>>,
}

struct NetConnState {
    phase: NetPhase,
    local: String,
    remote: String,
    last_error: Option<String>,
    endpoint: Option<NetEndpoint>,
    pending_accepts: VecDeque<PendingAccept>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NetPhase {
    Idle,
    Bound,
    Listening,
    Connected,
    Closed,
}

struct PendingAccept {
    remote: String,
    endpoint: NetEndpoint,
}

#[derive(Clone)]
struct NetEndpoint {
    inbound: Arc<ByteQueue>,
    outbound: Arc<ByteQueue>,
    state: Arc<Mutex<NetLinkState>>,
}

#[derive(Default)]
struct NetLinkState {
    closed: bool,
}

impl NetEndpoint {
    fn pair() -> (Self, Self) {
        let left = Arc::new(ByteQueue::new());
        let right = Arc::new(ByteQueue::new());
        let state = Arc::new(Mutex::new(NetLinkState::default()));
        (
            Self {
                inbound: left.clone(),
                outbound: right.clone(),
                state: state.clone(),
            },
            Self {
                inbound: right,
                outbound: left,
                state,
            },
        )
    }

    fn read(&self, buf: &mut [u8]) -> usize {
        self.inbound.read(buf)
    }

    fn write(&self, data: &[u8]) -> Result<usize> {
        if self.state.lock().unwrap().closed {
            return Err(Error::Message("data: broken pipe".to_string()));
        }
        Ok(self.outbound.write(data))
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
    }
}

impl NetConn {
    fn new(id: String, device: NetDevice) -> Self {
        Self {
            id,
            device,
            inner: Arc::new(Mutex::new(NetConnState {
                phase: NetPhase::Idle,
                local: String::new(),
                remote: String::new(),
                last_error: None,
                endpoint: None,
                pending_accepts: VecDeque::new(),
            })),
        }
    }

    fn install_accepted(&self, local: String, remote: String, endpoint: NetEndpoint) {
        let mut state = self.inner.lock().unwrap();
        state.phase = NetPhase::Connected;
        state.local = local;
        state.remote = remote;
        state.last_error = None;
        state.endpoint = Some(endpoint);
    }

    fn phase_name(phase: NetPhase) -> &'static str {
        match phase {
            NetPhase::Idle => "idle",
            NetPhase::Bound => "bound",
            NetPhase::Listening => "listening",
            NetPhase::Connected => "connected",
            NetPhase::Closed => "closed",
        }
    }

    fn snapshot(&self) -> (NetPhase, String, String, Option<String>) {
        let state = self.inner.lock().unwrap();
        (
            state.phase,
            state.local.clone(),
            state.remote.clone(),
            state.last_error.clone(),
        )
    }

    fn status(&self) -> String {
        let (phase, local, remote, last_error) = self.snapshot();
        let mut status = Self::phase_name(phase).to_string();
        if !local.is_empty() {
            status.push_str(" local=");
            status.push_str(&local);
        }
        if !remote.is_empty() {
            status.push_str(" remote=");
            status.push_str(&remote);
        }
        if let Some(last_error) = last_error {
            status.push_str(" err=");
            status.push_str(&last_error);
        }
        status
    }

    fn local(&self) -> String {
        self.inner.lock().unwrap().local.clone()
    }

    fn remote(&self) -> String {
        self.inner.lock().unwrap().remote.clone()
    }

    fn invalid_transition_message(op: &str, phase: NetPhase) -> String {
        format!("{op}: invalid transition from {}", Self::phase_name(phase))
    }

    fn bind(&self, addr: &str) -> Result<()> {
        let addr = addr.trim();
        if addr.is_empty() {
            let err = Error::Message("bind: missing address".to_string());
            self.inner.lock().unwrap().last_error = Some("bind: missing address".to_string());
            return Err(err);
        }
        let mut state = self.inner.lock().unwrap();
        match state.phase {
            NetPhase::Idle | NetPhase::Closed => {
                state.phase = NetPhase::Bound;
                state.local = addr.to_string();
                state.remote.clear();
                state.last_error = None;
                state.endpoint = None;
                Ok(())
            }
            NetPhase::Bound => {
                if state.local == addr {
                    state.last_error = None;
                    Ok(())
                } else {
                    let message = format!(
                        "bind: invalid transition from {} to {addr}",
                        Self::phase_name(state.phase)
                    );
                    state.last_error = Some(message.clone());
                    Err(Error::Message(message))
                }
            }
            phase => {
                let message = Self::invalid_transition_message("bind", phase);
                state.last_error = Some(message.clone());
                Err(Error::Message(message))
            }
        }
    }

    fn announce(&self, addr: &str) -> Result<()> {
        let addr = addr.trim();
        if addr.is_empty() {
            let err = Error::Message("announce: missing address".to_string());
            self.inner.lock().unwrap().last_error = Some("announce: missing address".to_string());
            return Err(err);
        }
        let listener_addr = {
            let mut state = self.inner.lock().unwrap();
            match state.phase {
                NetPhase::Idle | NetPhase::Closed => {
                    state.local = addr.to_string();
                }
                NetPhase::Bound => {
                    if state.local != addr {
                        let message = format!(
                            "announce: bound address mismatch: expected {}, got {addr}",
                            state.local
                        );
                        state.last_error = Some(message.clone());
                        return Err(Error::Message(message));
                    }
                }
                phase => {
                    let message = Self::invalid_transition_message("announce", phase);
                    state.last_error = Some(message.clone());
                    return Err(Error::Message(message));
                }
            }
            state.local.clone()
        };
        self.device.register_listener(&listener_addr, &self.id)?;
        let mut state = self.inner.lock().unwrap();
        state.phase = NetPhase::Listening;
        state.remote.clear();
        state.last_error = None;
        state.endpoint = None;
        Ok(())
    }

    fn dial(&self, addr: &str) -> Result<()> {
        let addr = addr.trim();
        if addr.is_empty() {
            let err = Error::Message("dial: missing address".to_string());
            self.inner.lock().unwrap().last_error = Some("dial: missing address".to_string());
            return Err(err);
        }
        let listener = self
            .device
            .listener(addr)
            .ok_or_else(|| Error::Message(format!("dial: no listener for {addr}")))?;

        let (dialer, acceptor) = NetEndpoint::pair();
        let local = {
            let mut state = self.inner.lock().unwrap();
            match state.phase {
                NetPhase::Idle | NetPhase::Bound | NetPhase::Closed => {
                    if state.local.is_empty() {
                        state.local = self.device.next_auto_local();
                    }
                    state.phase = NetPhase::Connected;
                    state.remote = addr.to_string();
                    state.last_error = None;
                    state.endpoint = Some(dialer.clone());
                    state.local.clone()
                }
                phase => {
                    let message = Self::invalid_transition_message("dial", phase);
                    state.last_error = Some(message.clone());
                    return Err(Error::Message(message));
                }
            }
        };

        listener.enqueue_accept(PendingAccept {
            remote: local,
            endpoint: acceptor,
        })?;
        Ok(())
    }

    fn enqueue_accept(&self, pending: PendingAccept) -> Result<()> {
        let mut state = self.inner.lock().unwrap();
        if state.phase != NetPhase::Listening {
            return Err(Error::Message("listen: listener is not active".to_string()));
        }
        state.pending_accepts.push_back(pending);
        Ok(())
    }

    fn accept_one(&self) -> Result<Option<String>> {
        let accepted = {
            let mut state = self.inner.lock().unwrap();
            if state.phase != NetPhase::Listening {
                let message = Self::invalid_transition_message("listen", state.phase);
                state.last_error = Some(message.clone());
                return Err(Error::Message(message));
            }
            state
                .pending_accepts
                .pop_front()
                .map(|pending| (state.local.clone(), pending))
        };
        if let Some((local, pending)) = accepted {
            let id = self
                .device
                .alloc_connected(local, pending.remote, pending.endpoint);
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    fn hangup(&self) -> Result<()> {
        let (phase, local, endpoint) = {
            let mut state = self.inner.lock().unwrap();
            match state.phase {
                NetPhase::Connected | NetPhase::Listening => {
                    let phase = state.phase;
                    let local = state.local.clone();
                    let endpoint = state.endpoint.take();
                    state.phase = NetPhase::Closed;
                    state.last_error = None;
                    state.pending_accepts.clear();
                    (phase, local, endpoint)
                }
                phase => {
                    let message = Self::invalid_transition_message("hangup", phase);
                    state.last_error = Some(message.clone());
                    return Err(Error::Message(message));
                }
            }
        };
        if phase == NetPhase::Listening && !local.is_empty() {
            self.device.unregister_listener(&local, &self.id);
        }
        if let Some(endpoint) = endpoint {
            endpoint.close();
        }
        Ok(())
    }

    fn reset(&self) {
        let (local, endpoint) = {
            let mut state = self.inner.lock().unwrap();
            let local = state.local.clone();
            let endpoint = state.endpoint.take();
            state.phase = NetPhase::Idle;
            state.local.clear();
            state.remote.clear();
            state.last_error = None;
            state.pending_accepts.clear();
            (local, endpoint)
        };
        if !local.is_empty() {
            self.device.unregister_listener(&local, &self.id);
        }
        if let Some(endpoint) = endpoint {
            endpoint.close();
        }
    }
}

impl FileSystem for NetConn {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        match name {
            "." => {
                let (phase, _, _, _) = self.snapshot();
                let mut entries = vec![
                    DirEntry::new("id", Metadata::file("id", 0o444, self.id.len() as u64 + 1)),
                    DirEntry::new("ctl", Metadata::file("ctl", 0o222, 0)),
                    DirEntry::new("data", Metadata::file("data", 0o666, 0)),
                    DirEntry::new(
                        "status",
                        Metadata::file("status", 0o444, self.status().len() as u64 + 1),
                    ),
                    DirEntry::new(
                        "local",
                        Metadata::file("local", 0o444, self.local().len() as u64 + 1),
                    ),
                    DirEntry::new(
                        "remote",
                        Metadata::file("remote", 0o444, self.remote().len() as u64 + 1),
                    ),
                ];
                if phase == NetPhase::Listening {
                    entries.push(DirEntry::new("listen", Metadata::file("listen", 0o444, 0)));
                }
                Ok(directory_file(Metadata::dir(".", 0o555), entries))
            }
            "id" => SharedTextFile::readonly("id", self.id.clone()).open(&FsContext::new(), "."),
            "ctl" => {
                let conn = self.clone();
                ControlFile::new("ctl", move |cmd| {
                    let mut parts = cmd.split_whitespace();
                    match parts.next() {
                        Some("dial") => conn.dial(&parts.collect::<Vec<_>>().join(" ")),
                        Some("bind") => conn.bind(&parts.collect::<Vec<_>>().join(" ")),
                        Some("announce") => conn.announce(&parts.collect::<Vec<_>>().join(" ")),
                        Some("hangup") => conn.hangup(),
                        Some("reset") => {
                            conn.reset();
                            Ok(())
                        }
                        Some("noop") | None => Ok(()),
                        Some(other) => {
                            Err(Error::Message(format!("ctl: unsupported command {other}")))
                        }
                    }
                })
                .open(&FsContext::new(), ".")
            }
            "data" => Ok(Box::new(NetDataHandle { conn: self.clone() })),
            "status" => {
                SharedTextFile::readonly("status", self.status()).open(&FsContext::new(), ".")
            }
            "local" => SharedTextFile::readonly("local", self.local()).open(&FsContext::new(), "."),
            "remote" => {
                SharedTextFile::readonly("remote", self.remote()).open(&FsContext::new(), ".")
            }
            "listen" => {
                if self.snapshot().0 != NetPhase::Listening {
                    return Err(Error::path("open", name, ErrorKind::NotFound));
                }
                Ok(Box::new(NetListenHandle {
                    conn: self.clone(),
                    data: None,
                    offset: 0,
                }))
            }
            _ => Err(Error::path("open", name, ErrorKind::NotFound)),
        }
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        let mut file = self.open(ctx, name)?;
        let stat = file.stat();
        let _ = file.close();
        stat
    }
}

struct NetDataHandle {
    conn: NetConn,
}

impl FileHandle for NetDataHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let endpoint = {
            let state = self.conn.inner.lock().unwrap();
            if state.phase != NetPhase::Connected {
                return Err(Error::Message(format!(
                    "data: unavailable while {}",
                    NetConn::phase_name(state.phase)
                )));
            }
            state.endpoint.clone()
        };
        Ok(endpoint.expect("connected endpoint exists").read(buf))
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let endpoint = {
            let state = self.conn.inner.lock().unwrap();
            if state.phase != NetPhase::Connected {
                return Err(Error::Message(format!(
                    "data: unavailable while {}",
                    NetConn::phase_name(state.phase)
                )));
            }
            state.endpoint.clone()
        };
        endpoint.expect("connected endpoint exists").write(data)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file("data", 0o666, 0))
    }
}

struct NetListenHandle {
    conn: NetConn,
    data: Option<Vec<u8>>,
    offset: u64,
}

impl FileHandle for NetListenHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.data.is_none() {
            self.data = self
                .conn
                .accept_one()?
                .map(|id| format!("{id}\n").into_bytes());
        }
        let Some(data) = self.data.as_ref() else {
            return Ok(0);
        };
        let start = self.offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn stat(&self) -> Result<Metadata> {
        Ok(Metadata::file("listen", 0o444, 0))
    }
}

fn worker_fs() -> FsRef {
    let fs = MemFs::from_entries([
        ("ctl", b"".to_vec()),
        ("kind", b"worker\n".to_vec()),
        ("state", b"created\n".to_vec()),
    ]);
    fs_ref(fs)
}

fn web_fs() -> MemFs {
    MemFs::from_entries([
        ("dom/ctl", b"".to_vec()),
        ("caches/new", b"".to_vec()),
        ("download/ctl", b"".to_vec()),
        ("worker/new", b"".to_vec()),
        ("opfs/new", b"".to_vec()),
    ])
}

fn js_value_fs() -> MemFs {
    MemFs::from_entries([
        ("global", b"[object global]\n".to_vec()),
        ("values", b"".to_vec()),
    ])
}

fn download_fs() -> FsRef {
    let fs = MemFs::from_entries([("ctl", b"".to_vec()), ("files", b"".to_vec())]);
    fs_ref(fs)
}

struct ByteQueue {
    state: Mutex<ByteQueueState>,
    ready: Condvar,
}

struct ByteQueueState {
    data: VecDeque<u8>,
}

impl ByteQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(ByteQueueState {
                data: VecDeque::new(),
            }),
            ready: Condvar::new(),
        }
    }

    fn len(&self) -> usize {
        self.state.lock().unwrap().data.len()
    }

    fn read(&self, buf: &mut [u8]) -> usize {
        let mut state = self.state.lock().unwrap();
        if state.data.is_empty() {
            return 0;
        }
        let n = buf.len().min(state.data.len());
        for out in &mut buf[..n] {
            *out = state.data.pop_front().unwrap();
        }
        n
    }

    fn write(&self, data: &[u8]) -> usize {
        let mut state = self.state.lock().unwrap();
        state.data.extend(data);
        self.ready.notify_all();
        data.len()
    }

    fn clear(&self) {
        self.state.lock().unwrap().data.clear();
    }
}

#[derive(Clone)]
struct QueueFile {
    name: String,
    reader: Arc<ByteQueue>,
    writer: Arc<ByteQueue>,
    behavior: QueueWriteBehavior,
}

#[derive(Clone)]
enum QueueWriteBehavior {
    Raw,
    NormalizeLf { prev: Arc<Mutex<Option<u8>>> },
}

impl QueueFile {
    fn loopback(name: impl Into<String>) -> Self {
        let queue = Arc::new(ByteQueue::new());
        Self {
            name: name.into(),
            reader: queue.clone(),
            writer: queue,
            behavior: QueueWriteBehavior::Raw,
        }
    }

    fn program(name: impl Into<String>) -> Self {
        let queue = Arc::new(ByteQueue::new());
        Self {
            name: name.into(),
            reader: queue.clone(),
            writer: queue,
            behavior: QueueWriteBehavior::NormalizeLf {
                prev: Arc::new(Mutex::new(None)),
            },
        }
    }

    fn clear(&self) {
        self.reader.clear();
        if !Arc::ptr_eq(&self.reader, &self.writer) {
            self.writer.clear();
        }
        if let QueueWriteBehavior::NormalizeLf { prev } = &self.behavior {
            *prev.lock().unwrap() = None;
        }
    }

    fn write(&self, data: &[u8]) -> usize {
        match &self.behavior {
            QueueWriteBehavior::Raw => self.writer.write(data),
            QueueWriteBehavior::NormalizeLf { prev } => {
                if data.is_empty() {
                    return 0;
                }
                let mut previous = prev.lock().unwrap();
                let mut normalized = Vec::with_capacity(data.len() + data.len() / 8);
                let mut last = *previous;
                for &byte in data {
                    if byte == b'\n' && last != Some(b'\r') {
                        normalized.push(b'\r');
                    }
                    normalized.push(byte);
                    last = Some(byte);
                }
                *previous = last;
                self.writer.write(&normalized);
                data.len()
            }
        }
    }
}

impl FileSystem for QueueFile {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        Ok(Box::new(QueueHandle { file: self.clone() }))
    }

    fn stat(&self, _ctx: &FsContext, _name: &str) -> Result<Metadata> {
        Ok(Metadata::file(
            self.name.clone(),
            0o666,
            self.reader.len() as u64,
        ))
    }
}

struct QueueHandle {
    file: QueueFile,
}

impl FileHandle for QueueHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.file.reader.read(buf))
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        Ok(self.file.write(data))
    }

    fn stat(&self) -> Result<Metadata> {
        self.file.stat(&FsContext::new(), ".")
    }
}

#[derive(Clone)]
struct TerminalDataFile {
    queue: QueueFile,
    screen: SharedLogFile,
}

impl TerminalDataFile {
    fn new(name: impl Into<String>, screen: SharedLogFile) -> Self {
        Self {
            queue: QueueFile::loopback(name),
            screen,
        }
    }

    fn clear(&self) {
        self.queue.clear();
        self.screen.clear();
    }
}

impl FileSystem for TerminalDataFile {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        Ok(Box::new(TerminalDataHandle { file: self.clone() }))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.queue.stat(ctx, name)
    }
}

struct TerminalDataHandle {
    file: TerminalDataFile,
}

impl FileHandle for TerminalDataHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.file.queue.reader.read(buf))
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.file.screen.append(data);
        Ok(self.file.queue.write(data))
    }

    fn stat(&self) -> Result<Metadata> {
        self.file.queue.stat(&FsContext::new(), ".")
    }
}

#[derive(Clone)]
struct SharedTextFile {
    name: String,
    value: Arc<Mutex<String>>,
    writable: bool,
}

impl SharedTextFile {
    fn readonly(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Arc::new(Mutex::new(value.into())),
            writable: false,
        }
    }

    fn writable(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Arc::new(Mutex::new(value.into())),
            writable: true,
        }
    }

    fn get(&self) -> String {
        self.value.lock().unwrap().clone()
    }

    fn set(&self, value: impl Into<String>) {
        *self.value.lock().unwrap() = value.into();
    }
}

impl FileSystem for SharedTextFile {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        Ok(Box::new(SharedTextHandle {
            file: self.clone(),
            data: format!("{}\n", self.get()).into_bytes(),
            offset: 0,
            dirty: Vec::new(),
        }))
    }

    fn stat(&self, _ctx: &FsContext, _name: &str) -> Result<Metadata> {
        Ok(Metadata::file(
            self.name.clone(),
            if self.writable { 0o666 } else { 0o444 },
            self.get().len() as u64 + 1,
        ))
    }
}

struct SharedTextHandle {
    file: SharedTextFile,
    data: Vec<u8>,
    offset: u64,
    dirty: Vec<u8>,
}

impl FileHandle for SharedTextHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let start = self.offset as usize;
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if !self.file.writable {
            return Err(ErrorKind::PermissionDenied.into());
        }
        self.dirty.extend_from_slice(data);
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        self.file.stat(&FsContext::new(), ".")
    }

    fn close(&mut self) -> Result<()> {
        if self.file.writable && !self.dirty.is_empty() {
            self.file
                .set(String::from_utf8_lossy(&self.dirty).trim().to_string());
        }
        Ok(())
    }
}

#[derive(Clone)]
struct SharedLogFile {
    name: String,
    data: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogFile {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn append_line(&self, line: &str) {
        let mut data = self.data.lock().unwrap();
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
    }

    fn append(&self, bytes: &[u8]) {
        self.data.lock().unwrap().extend_from_slice(bytes);
    }

    fn clear(&self) {
        self.data.lock().unwrap().clear();
    }
}

impl FileSystem for SharedLogFile {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        if name != "." {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        Ok(Box::new(SharedLogHandle {
            file: self.clone(),
            offset: 0,
        }))
    }

    fn stat(&self, _ctx: &FsContext, _name: &str) -> Result<Metadata> {
        Ok(Metadata::file(
            self.name.clone(),
            0o666,
            self.data.lock().unwrap().len() as u64,
        ))
    }
}

struct SharedLogHandle {
    file: SharedLogFile,
    offset: u64,
}

impl FileHandle for SharedLogHandle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let data = self.file.data.lock().unwrap();
        let start = self.offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        self.offset += n as u64;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.file.data.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn stat(&self) -> Result<Metadata> {
        self.file.stat(&FsContext::new(), ".")
    }
}

#[cfg(test)]
mod tests {
    use super::{VmProvider, VmProviderResource, VmProviderUpdate};
    use crate::Runtime;
    use star9_core::Result;
    use star9_fs::{open, read_dir, read_file};
    use std::sync::{Arc, Mutex};

    fn alloc_id(runtime: &Runtime, path: &str) -> String {
        String::from_utf8(read_file(runtime.namespace().as_ref(), path).unwrap())
            .unwrap()
            .trim()
            .to_string()
    }

    fn write_handle(runtime: &Runtime, path: &str, data: &[u8]) {
        let mut file = open(runtime.namespace().as_ref(), path).unwrap();
        file.write(data).unwrap();
        file.close().unwrap();
    }

    fn write_handle_error(runtime: &Runtime, path: &str, data: &[u8]) -> String {
        let mut file = open(runtime.namespace().as_ref(), path).unwrap();
        if let Err(err) = file.write(data) {
            return err.to_string();
        }
        file.close().unwrap_err().to_string()
    }

    #[test]
    fn terminal_device_supports_controlled_pipes_and_winch() {
        let runtime = Runtime::new().unwrap();
        let term_id = alloc_id(&runtime, "#term/new");

        let mut program_reader = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/program"),
        )
        .unwrap();
        let mut program_writer = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/program"),
        )
        .unwrap();
        program_writer.write(b"run").unwrap();
        let mut buf = [0_u8; 16];
        let n = program_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"run");
        program_writer.write(b"\nnext\n").unwrap();
        let n = program_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"\r\nnext\r\n");

        let mut data_reader = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/data"),
        )
        .unwrap();
        let mut data_writer = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/data"),
        )
        .unwrap();
        data_writer.write(b"screen").unwrap();
        let n = data_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"screen");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#term/{term_id}/screen")
            )
            .unwrap(),
            b"screen"
        );

        let mut winch_reader = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/winch/data"),
        )
        .unwrap();
        let mut winch_writer = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/winch/data"),
        )
        .unwrap();
        winch_writer.write(b"120x40").unwrap();
        let n = winch_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"120x40");

        write_handle(&runtime, &format!("#term/{term_id}/program"), b"stale");
        write_handle(&runtime, &format!("#term/{term_id}/ctl"), b"clear");
        let mut emptied = open(
            runtime.namespace().as_ref(),
            &format!("#term/{term_id}/program"),
        )
        .unwrap();
        assert_eq!(emptied.read(&mut buf).unwrap(), 0);
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#term/{term_id}/screen")
            )
            .unwrap(),
            b""
        );

        write_handle(&runtime, &format!("#term/{term_id}/size"), b"132x43");
        write_handle(&runtime, &format!("#term/{term_id}/ctl"), b"reset");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#term/{term_id}/state")
            )
            .unwrap(),
            b"ready\n"
        );
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#term/{term_id}/size")
            )
            .unwrap(),
            b"80x24\n"
        );

        write_handle(&runtime, &format!("#term/{term_id}/ctl"), b"noop");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#term/{term_id}/size")
            )
            .unwrap(),
            b"80x24\n"
        );
    }

    #[test]
    fn vm_device_tracks_state_alias_config_and_console() {
        let runtime = Runtime::new().unwrap();
        let vm_id = alloc_id(&runtime, "#vm/new/firecracker");

        write_handle(&runtime, &format!("#vm/{vm_id}/alias"), b"guest-a");
        write_handle(&runtime, &format!("#vm/{vm_id}/config"), b"mem=128M cpu=1");
        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"start");
        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"stop");

        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/alias")).unwrap(),
            b"guest-a\n"
        );
        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/kind")).unwrap(),
            b"firecracker\n"
        );
        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/config")).unwrap(),
            b"mem=128M cpu=1\n"
        );
        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/state")).unwrap(),
            b"stopped\n"
        );
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#vm/{vm_id}/console")
            )
            .unwrap(),
            b"start\nstop\n"
        );
        assert_eq!(
            read_file(runtime.namespace().as_ref(), "#vm/guest-a/alias").unwrap(),
            b"guest-a\n"
        );

        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"alias guest-b");
        assert!(read_file(runtime.namespace().as_ref(), "#vm/guest-a/state").is_err());
        assert_eq!(
            read_file(runtime.namespace().as_ref(), "#vm/guest-b/alias").unwrap(),
            b"guest-b\n"
        );

        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"reset");
        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/state")).unwrap(),
            b"created\n"
        );
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#vm/{vm_id}/console")
            )
            .unwrap(),
            b""
        );
    }

    #[derive(Default)]
    struct RecordingVmProvider {
        starts: Mutex<Vec<VmProviderResource>>,
    }

    impl VmProvider for RecordingVmProvider {
        fn start(&self, resource: &VmProviderResource) -> Result<VmProviderUpdate> {
            self.starts.lock().unwrap().push(resource.clone());
            Ok(VmProviderUpdate {
                state: Some("provider-running".to_string()),
                console_lines: vec![format!("provider-start {}", resource.kind)],
                clear_console: false,
            })
        }

        fn stop(&self, _resource: &VmProviderResource) -> Result<VmProviderUpdate> {
            Ok(VmProviderUpdate::state("provider-stopped"))
        }

        fn reset(&self, _resource: &VmProviderResource) -> Result<VmProviderUpdate> {
            Ok(VmProviderUpdate {
                state: Some("provider-created".to_string()),
                console_lines: Vec::new(),
                clear_console: true,
            })
        }
    }

    #[test]
    fn vm_device_routes_lifecycle_through_provider_contract() {
        let runtime = Runtime::new().unwrap();
        let provider = Arc::new(RecordingVmProvider::default());
        runtime.set_vm_provider(provider.clone());
        let vm_id = alloc_id(&runtime, "#vm/new/v86");

        write_handle(&runtime, &format!("#vm/{vm_id}/alias"), b"provider-guest");
        write_handle(&runtime, &format!("#vm/{vm_id}/config"), b"mem=64M");
        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"start");

        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/state")).unwrap(),
            b"provider-running\n"
        );
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#vm/{vm_id}/console")
            )
            .unwrap(),
            b"provider-start v86\n"
        );
        assert_eq!(
            provider.starts.lock().unwrap().as_slice(),
            &[VmProviderResource {
                id: vm_id.clone(),
                kind: "v86".to_string(),
                alias: "provider-guest".to_string(),
                config: "mem=64M".to_string(),
            }]
        );

        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"stop");
        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/state")).unwrap(),
            b"provider-stopped\n"
        );
    }

    #[test]
    fn net_device_exposes_deterministic_state_accept_and_data() {
        let runtime = Runtime::new().unwrap();
        let listener_id = alloc_id(&runtime, "#net/new");
        let client_id = alloc_id(&runtime, "#net/new");

        write_handle(
            &runtime,
            &format!("#net/{listener_id}/ctl"),
            b"announce service:7",
        );
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{listener_id}/status")
            )
            .unwrap(),
            b"listening local=service:7\n"
        );
        let listener_entries: Vec<_> =
            read_dir(runtime.namespace().as_ref(), &format!("#net/{listener_id}"))
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect();
        assert!(listener_entries.contains(&"listen".to_string()));

        write_handle(
            &runtime,
            &format!("#net/{client_id}/ctl"),
            b"dial service:7",
        );
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{client_id}/status")
            )
            .unwrap(),
            b"connected local=local:10001 remote=service:7\n"
        );

        let mut listen = open(
            runtime.namespace().as_ref(),
            &format!("#net/{listener_id}/listen"),
        )
        .unwrap();
        let mut buf = [0_u8; 32];
        let n = listen.read(&mut buf).unwrap();
        let accepted_id = String::from_utf8(buf[..n].to_vec()).unwrap();
        let accepted_id = accepted_id.trim().to_string();
        assert_eq!(accepted_id, "3");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{accepted_id}/status")
            )
            .unwrap(),
            b"connected local=service:7 remote=local:10001\n"
        );

        let mut accepted_reader = open(
            runtime.namespace().as_ref(),
            &format!("#net/{accepted_id}/data"),
        )
        .unwrap();
        let mut client_writer = open(
            runtime.namespace().as_ref(),
            &format!("#net/{client_id}/data"),
        )
        .unwrap();
        client_writer.write(b"payload").unwrap();
        let n = accepted_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"payload");

        let mut client_reader = open(
            runtime.namespace().as_ref(),
            &format!("#net/{client_id}/data"),
        )
        .unwrap();
        let mut accepted_writer = open(
            runtime.namespace().as_ref(),
            &format!("#net/{accepted_id}/data"),
        )
        .unwrap();
        accepted_writer.write(b"reply").unwrap();
        let n = client_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"reply");

        write_handle(&runtime, &format!("#net/{client_id}/ctl"), b"hangup");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{client_id}/status")
            )
            .unwrap(),
            b"closed local=local:10001 remote=service:7\n"
        );
        let mut drained = open(
            runtime.namespace().as_ref(),
            &format!("#net/{accepted_id}/data"),
        )
        .unwrap();
        assert_eq!(drained.read(&mut buf).unwrap(), 0);
        let err = accepted_writer.write(b"after-close").unwrap_err();
        assert_eq!(err.to_string(), "data: broken pipe");

        write_handle(&runtime, &format!("#net/{listener_id}/ctl"), b"hangup");
        assert!(open(
            runtime.namespace().as_ref(),
            &format!("#net/{listener_id}/listen")
        )
        .is_err());
        write_handle(&runtime, &format!("#net/{listener_id}/ctl"), b"reset");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{listener_id}/status")
            )
            .unwrap(),
            b"idle\n"
        );
    }

    #[test]
    fn net_device_reports_invalid_transitions() {
        let runtime = Runtime::new().unwrap();
        let net_id = alloc_id(&runtime, "#net/new");

        let err = write_handle_error(&runtime, &format!("#net/{net_id}/ctl"), b"hangup");
        assert_eq!(err, "hangup: invalid transition from idle");

        write_handle(
            &runtime,
            &format!("#net/{net_id}/ctl"),
            b"announce service:9",
        );
        let err = write_handle_error(&runtime, &format!("#net/{net_id}/ctl"), b"bind service:10");
        assert_eq!(err, "bind: invalid transition from listening");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{net_id}/status")
            )
            .unwrap(),
            b"listening local=service:9 err=bind: invalid transition from listening\n"
        );
    }
}
