use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use wanix_core::{clean_path, valid_path, DirEntry, Error, ErrorKind, FsContext, Metadata, Result};
use wanix_fs::{
    directory_file, fs_ref, BoxFile, ControlFile, FileHandle, FileSystem, FsRef, MapFs, MemFs,
    SignalFs,
};
use wanix_task::Task;
use wanix_vfs::BindMode;

pub(crate) fn bind_devices(root: &Task) -> Result<()> {
    let devices: [(&str, FsRef); 11] = [
        (
            "#pipe",
            fs_ref(DeviceAllocator::new("pipe", |_| {
                fs_ref(wanix_fs::PipeFs::new(false))
            })),
        ),
        (
            "#signal",
            fs_ref(DeviceAllocator::new("signal", |_| {
                fs_ref(wanix_fs::SignalFs::default())
            })),
        ),
        (
            "#ramfs",
            fs_ref(DeviceAllocator::new("ramfs", |_| fs_ref(MemFs::new()))),
        ),
        ("#term", fs_ref(DeviceAllocator::new("term", terminal_fs))),
        ("#vm", fs_ref(DeviceAllocator::new("vm", vm_fs))),
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
        ("#net", fs_ref(DeviceAllocator::new("net", net_fs))),
    ];
    for (dst, fs) in devices {
        root.namespace().bind(fs, ".", dst, BindMode::Replace)?;
    }
    Ok(())
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
    let data = QueueFile::loopback("data");
    let program = QueueFile::loopback("program");
    let winch = SignalFs::default();

    let ctl_state = state.clone();
    let ctl_size = size.clone();
    let ctl_data = data.clone();
    let ctl_program = program.clone();
    let ctl = ControlFile::new("ctl", move |cmd| match cmd {
        "clear" => {
            ctl_data.clear();
            ctl_program.clear();
            Ok(())
        }
        "reset" => {
            ctl_data.clear();
            ctl_program.clear();
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
    fs.insert("program", fs_ref(program));
    fs.insert("winch", fs_ref(winch));
    fs_ref(fs)
}

fn vm_fs(id: &str) -> FsRef {
    let state = SharedTextFile::readonly("state", "created");
    let alias = SharedTextFile::writable("alias", "");
    let config = SharedTextFile::writable("config", "");
    let console = SharedLogFile::new("console");

    let ctl_state = state.clone();
    let ctl_alias = alias.clone();
    let ctl_config = config.clone();
    let ctl_console = console.clone();
    let ctl = ControlFile::new("ctl", move |cmd| {
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            Some("start") => {
                ctl_state.set("running");
                ctl_console.append_line("start");
                Ok(())
            }
            Some("stop") => {
                ctl_state.set("stopped");
                ctl_console.append_line("stop");
                Ok(())
            }
            Some("reset") => {
                ctl_state.set("created");
                ctl_console.clear();
                Ok(())
            }
            Some("alias") => {
                ctl_alias.set(parts.collect::<Vec<_>>().join(" "));
                Ok(())
            }
            Some("config") => {
                ctl_config.set(parts.collect::<Vec<_>>().join(" "));
                Ok(())
            }
            Some("noop") | None => Ok(()),
            Some(_) => Err(ErrorKind::Invalid.into()),
        }
    });

    let fs = MapFs::new();
    fs.insert("id", fs_ref(SharedTextFile::readonly("id", id)));
    fs.insert("kind", fs_ref(SharedTextFile::readonly("kind", "vm")));
    fs.insert("ctl", fs_ref(ctl));
    fs.insert("state", fs_ref(state));
    fs.insert("alias", fs_ref(alias));
    fs.insert("config", fs_ref(config));
    fs.insert("console", fs_ref(console));
    fs_ref(fs)
}

fn net_fs(id: &str) -> FsRef {
    let status = SharedTextFile::readonly("status", "idle");
    let data = QueueFile::loopback("data");

    let ctl_status = status.clone();
    let ctl_data = data.clone();
    let ctl = ControlFile::new("ctl", move |cmd| {
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            Some("connect") => {
                let peer = parts.collect::<Vec<_>>().join(" ");
                if peer.is_empty() {
                    ctl_status.set("connected");
                } else {
                    ctl_status.set(format!("connected {peer}"));
                }
                Ok(())
            }
            Some("listen") => {
                let addr = parts.collect::<Vec<_>>().join(" ");
                if addr.is_empty() {
                    ctl_status.set("listening");
                } else {
                    ctl_status.set(format!("listening {addr}"));
                }
                Ok(())
            }
            Some("close") => {
                ctl_status.set("closed");
                ctl_data.clear();
                Ok(())
            }
            Some("reset") => {
                ctl_status.set("idle");
                ctl_data.clear();
                Ok(())
            }
            Some("noop") | None => Ok(()),
            Some(_) => Err(ErrorKind::Invalid.into()),
        }
    });

    let fs = MapFs::new();
    fs.insert("id", fs_ref(SharedTextFile::readonly("id", id)));
    fs.insert("ctl", fs_ref(ctl));
    fs.insert("data", fs_ref(data));
    fs.insert("status", fs_ref(status));
    fs_ref(fs)
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
}

impl QueueFile {
    fn loopback(name: impl Into<String>) -> Self {
        let queue = Arc::new(ByteQueue::new());
        Self {
            name: name.into(),
            reader: queue.clone(),
            writer: queue,
        }
    }

    fn clear(&self) {
        self.reader.clear();
        if !Arc::ptr_eq(&self.reader, &self.writer) {
            self.writer.clear();
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
        Ok(self.file.writer.write(data))
    }

    fn stat(&self) -> Result<Metadata> {
        self.file.stat(&FsContext::new(), ".")
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
    use crate::Runtime;
    use wanix_fs::{open, read_file};

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
        let mut buf = [0_u8; 8];
        let n = program_reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"run");

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
        let vm_id = alloc_id(&runtime, "#vm/new");

        write_handle(&runtime, &format!("#vm/{vm_id}/alias"), b"guest-a");
        write_handle(&runtime, &format!("#vm/{vm_id}/config"), b"mem=128M cpu=1");
        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"start");
        write_handle(&runtime, &format!("#vm/{vm_id}/ctl"), b"stop");

        assert_eq!(
            read_file(runtime.namespace().as_ref(), &format!("#vm/{vm_id}/alias")).unwrap(),
            b"guest-a\n"
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

    #[test]
    fn net_device_allocates_deterministic_placeholder_connections() {
        let runtime = Runtime::new().unwrap();
        let net_id = alloc_id(&runtime, "#net/new");

        write_handle(&runtime, &format!("#net/{net_id}/ctl"), b"connect loopback");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{net_id}/status")
            )
            .unwrap(),
            b"connected loopback\n"
        );

        let mut reader =
            open(runtime.namespace().as_ref(), &format!("#net/{net_id}/data")).unwrap();
        let mut writer =
            open(runtime.namespace().as_ref(), &format!("#net/{net_id}/data")).unwrap();
        writer.write(b"payload").unwrap();
        let mut buf = [0_u8; 16];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"payload");

        write_handle(&runtime, &format!("#net/{net_id}/ctl"), b"close");
        assert_eq!(
            read_file(
                runtime.namespace().as_ref(),
                &format!("#net/{net_id}/status")
            )
            .unwrap(),
            b"closed\n"
        );
        let mut drained =
            open(runtime.namespace().as_ref(), &format!("#net/{net_id}/data")).unwrap();
        assert_eq!(drained.read(&mut buf).unwrap(), 0);
    }
}
