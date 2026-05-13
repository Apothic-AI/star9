//! Typed Wanix file API surface.
//!
//! This crate keeps the protocol boundary typed so native callers, browser glue,
//! and tests exercise the same method set without depending on non-Rust internals.

use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use wanix_core::{Error, ErrorKind, FileMode, OpenFlags, Result};
use wanix_fs::{self as fs, FileSystem};
use wanix_task::Task;

pub mod p9;

pub fn encode_request(request: &ApiRequest) -> Result<Vec<u8>> {
    encode_cbor(request)
}

pub fn decode_request(data: &[u8]) -> Result<ApiRequest> {
    decode_cbor(data)
}

pub fn encode_response(response: &ApiResponse) -> Result<Vec<u8>> {
    encode_cbor(response)
}

pub fn decode_response(data: &[u8]) -> Result<ApiResponse> {
    decode_cbor(data)
}

fn encode_cbor(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|err| Error::Message(err.to_string()))?;
    Ok(out)
}

fn decode_cbor<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T> {
    ciborium::from_reader(data).map_err(|err| Error::Message(err.to_string()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StatInfo {
    pub size: u64,
    pub mode: u32,
    pub is_dir: bool,
    pub modified_ms: u128,
}

impl From<wanix_core::Metadata> for StatInfo {
    fn from(meta: wanix_core::Metadata) -> Self {
        let modified_ms = meta
            .modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            size: meta.size,
            mode: meta.mode.unix_type_and_perm(),
            is_dir: meta.is_dir(),
            modified_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "args")]
pub enum ApiRequest {
    Open(String),
    OpenFile(String, u32, u32),
    Create(String),
    Close(u32),
    Sync(u32),
    Read(u32, usize),
    Write(u32, #[serde(with = "serde_bytes")] Vec<u8>),
    WriteAt(u32, #[serde(with = "serde_bytes")] Vec<u8>, u64),
    ReadDir(String),
    Mkdir(String),
    MkdirAll(String),
    Bind(String, String),
    Unbind(String, String),
    Stat(String),
    Truncate(String, u64),
    WaitFor(String, u64),
    Rename(String, String),
    Copy(String, String),
    Remove(String),
    RemoveAll(String),
    ReadFile(String),
    WriteFile(String, #[serde(with = "serde_bytes")] Vec<u8>),
    AppendFile(String, #[serde(with = "serde_bytes")] Vec<u8>),
    Fstat(u32),
    Lstat(String),
    Chmod(String, u32),
    Chown(String, u32, u32),
    Fchmod(u32, u32),
    Fchown(u32, u32, u32),
    Ftruncate(u32, u64),
    Readlink(String),
    Symlink(String, String),
    Chtimes(String, f64, f64),
}

impl ApiRequest {
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::Open(_) => "Open",
            Self::OpenFile(_, _, _) => "OpenFile",
            Self::Create(_) => "Create",
            Self::Close(_) => "Close",
            Self::Sync(_) => "Sync",
            Self::Read(_, _) => "Read",
            Self::Write(_, _) => "Write",
            Self::WriteAt(_, _, _) => "WriteAt",
            Self::ReadDir(_) => "ReadDir",
            Self::Mkdir(_) => "Mkdir",
            Self::MkdirAll(_) => "MkdirAll",
            Self::Bind(_, _) => "Bind",
            Self::Unbind(_, _) => "Unbind",
            Self::Stat(_) => "Stat",
            Self::Truncate(_, _) => "Truncate",
            Self::WaitFor(_, _) => "WaitFor",
            Self::Rename(_, _) => "Rename",
            Self::Copy(_, _) => "Copy",
            Self::Remove(_) => "Remove",
            Self::RemoveAll(_) => "RemoveAll",
            Self::ReadFile(_) => "ReadFile",
            Self::WriteFile(_, _) => "WriteFile",
            Self::AppendFile(_, _) => "AppendFile",
            Self::Fstat(_) => "Fstat",
            Self::Lstat(_) => "Lstat",
            Self::Chmod(_, _) => "Chmod",
            Self::Chown(_, _, _) => "Chown",
            Self::Fchmod(_, _) => "Fchmod",
            Self::Fchown(_, _, _) => "Fchown",
            Self::Ftruncate(_, _) => "Ftruncate",
            Self::Readlink(_) => "Readlink",
            Self::Symlink(_, _) => "Symlink",
            Self::Chtimes(_, _, _) => "Chtimes",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum ApiResponse {
    Unit,
    Fd(u32),
    Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
    OptionalBytes(Option<Vec<u8>>),
    Count(usize),
    Entries(Vec<String>),
    Stat(StatInfo),
    String(String),
}

#[derive(Clone)]
pub struct WanixApi {
    task: Task,
}

impl WanixApi {
    pub fn new(task: Task) -> Self {
        Self { task }
    }

    pub fn task(&self) -> &Task {
        &self.task
    }

    pub fn dispatch(&self, req: ApiRequest) -> Result<ApiResponse> {
        match req {
            ApiRequest::Open(path) => self.open(&path).map(ApiResponse::Fd),
            ApiRequest::OpenFile(path, flags, mode) => self
                .open_file(
                    &path,
                    OpenFlags::from_bits(flags),
                    FileMode::from_bits(mode),
                )
                .map(ApiResponse::Fd),
            ApiRequest::Create(path) => self.create(&path).map(ApiResponse::Fd),
            ApiRequest::Close(fd) => self.close(fd).map(|_| ApiResponse::Unit),
            ApiRequest::Sync(fd) => self.sync(fd).map(|_| ApiResponse::Unit),
            ApiRequest::Read(fd, count) => self.read(fd, count).map(ApiResponse::OptionalBytes),
            ApiRequest::Write(fd, data) => self.write(fd, &data).map(ApiResponse::Count),
            ApiRequest::WriteAt(fd, data, offset) => {
                self.write_at(fd, &data, offset).map(ApiResponse::Count)
            }
            ApiRequest::ReadDir(path) => self.read_dir(&path).map(ApiResponse::Entries),
            ApiRequest::Mkdir(path) => self.mkdir(&path).map(|_| ApiResponse::Unit),
            ApiRequest::MkdirAll(path) => self.mkdir_all(&path).map(|_| ApiResponse::Unit),
            ApiRequest::Bind(src, dst) => self.bind(&src, &dst).map(|_| ApiResponse::Unit),
            ApiRequest::Unbind(src, dst) => self.unbind(&src, &dst).map(|_| ApiResponse::Unit),
            ApiRequest::Stat(path) => self.stat(&path).map(ApiResponse::Stat),
            ApiRequest::Truncate(path, size) => {
                self.truncate(&path, size).map(|_| ApiResponse::Unit)
            }
            ApiRequest::WaitFor(path, timeout) => self
                .wait_for(&path, Duration::from_millis(timeout))
                .map(|_| ApiResponse::Unit),
            ApiRequest::Rename(old, new) => self.rename(&old, &new).map(|_| ApiResponse::Unit),
            ApiRequest::Copy(old, new) => self.copy(&old, &new).map(|_| ApiResponse::Unit),
            ApiRequest::Remove(path) => self.remove(&path).map(|_| ApiResponse::Unit),
            ApiRequest::RemoveAll(path) => self.remove_all(&path).map(|_| ApiResponse::Unit),
            ApiRequest::ReadFile(path) => self.read_file(&path).map(ApiResponse::Bytes),
            ApiRequest::WriteFile(path, data) => {
                self.write_file(&path, &data).map(|_| ApiResponse::Unit)
            }
            ApiRequest::AppendFile(path, data) => {
                self.append_file(&path, &data).map(|_| ApiResponse::Unit)
            }
            ApiRequest::Fstat(fd) => self.fstat(fd).map(ApiResponse::Stat),
            ApiRequest::Lstat(path) => self.lstat(&path).map(ApiResponse::Stat),
            ApiRequest::Chmod(path, mode) => self.chmod(&path, mode).map(|_| ApiResponse::Unit),
            ApiRequest::Chown(path, uid, gid) => {
                self.chown(&path, uid, gid).map(|_| ApiResponse::Unit)
            }
            ApiRequest::Fchmod(fd, mode) => self.fchmod(fd, mode).map(|_| ApiResponse::Unit),
            ApiRequest::Fchown(fd, uid, gid) => {
                self.fchown(fd, uid, gid).map(|_| ApiResponse::Unit)
            }
            ApiRequest::Ftruncate(fd, size) => self.ftruncate(fd, size).map(|_| ApiResponse::Unit),
            ApiRequest::Readlink(path) => self.readlink(&path).map(ApiResponse::String),
            ApiRequest::Symlink(old, new) => self.symlink(&old, &new).map(|_| ApiResponse::Unit),
            ApiRequest::Chtimes(path, _atime, mtime) => self
                .chtimes(&path, unix_float_to_time(mtime))
                .map(|_| ApiResponse::Unit),
        }
    }

    pub fn open(&self, path: &str) -> Result<u32> {
        let file = self
            .task
            .namespace()
            .open(&wanix_core::FsContext::new(), path)?;
        Ok(self.task.open_fd(file, path))
    }

    pub fn open_file(&self, path: &str, flags: OpenFlags, mode: FileMode) -> Result<u32> {
        let file = self.task.namespace().open_file(path, flags, mode)?;
        Ok(self.task.open_fd(file, path))
    }

    pub fn create(&self, path: &str) -> Result<u32> {
        let file = self.task.namespace().create(path)?;
        Ok(self.task.open_fd(file, path))
    }

    pub fn close(&self, fd: u32) -> Result<()> {
        self.task.close_fd(fd)
    }

    pub fn sync(&self, fd: u32) -> Result<()> {
        self.task.with_fd_mut(fd, |file| file.sync())
    }

    pub fn read(&self, fd: u32, count: usize) -> Result<Option<Vec<u8>>> {
        self.task.with_fd_mut(fd, |file| {
            let mut buf = vec![0; count];
            let n = file.read(&mut buf)?;
            if n == 0 {
                Ok(None)
            } else {
                buf.truncate(n);
                Ok(Some(buf))
            }
        })
    }

    pub fn write(&self, fd: u32, data: &[u8]) -> Result<usize> {
        self.task.with_fd_mut(fd, |file| file.write(data))
    }

    pub fn write_at(&self, fd: u32, data: &[u8], offset: u64) -> Result<usize> {
        self.task
            .with_fd_mut(fd, |file| file.write_at(data, offset))
    }

    pub fn read_dir(&self, path: &str) -> Result<Vec<String>> {
        Ok(fs::read_dir(self.task.namespace().as_ref(), path)?
            .into_iter()
            .map(|entry| {
                if entry.is_dir() {
                    format!("{}/", entry.name)
                } else {
                    entry.name
                }
            })
            .collect())
    }

    pub fn mkdir(&self, path: &str) -> Result<()> {
        self.task
            .namespace()
            .mkdir(path, FileMode::DIR | FileMode::from_perm(0o755))
    }

    pub fn mkdir_all(&self, path: &str) -> Result<()> {
        fs::mkdir_all(
            self.task.namespace().as_ref(),
            path,
            FileMode::DIR | FileMode::from_perm(0o755),
        )
    }

    pub fn bind(&self, src: &str, dst: &str) -> Result<()> {
        self.task.bind(src, dst)
    }

    pub fn unbind(&self, src: &str, dst: &str) -> Result<()> {
        self.task.unbind(src, dst)
    }

    pub fn stat(&self, path: &str) -> Result<StatInfo> {
        Ok(fs::stat(self.task.namespace().as_ref(), path)?.into())
    }

    pub fn truncate(&self, path: &str, size: u64) -> Result<()> {
        self.task.namespace().truncate(path, size)
    }

    pub fn wait_for(&self, path: &str, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() <= deadline {
            if fs::exists(self.task.namespace().as_ref(), path)? {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(Error::path("waitfor", path, ErrorKind::NotFound))
    }

    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        self.task.namespace().rename(old, new)
    }

    pub fn copy(&self, old: &str, new: &str) -> Result<()> {
        fs::copy_all(self.task.namespace().as_ref(), old, new)
    }

    pub fn remove(&self, path: &str) -> Result<()> {
        self.task.namespace().remove(path)
    }

    pub fn remove_all(&self, path: &str) -> Result<()> {
        fs::remove_all(self.task.namespace().as_ref(), path)
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        fs::read_file(self.task.namespace().as_ref(), path)
    }

    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        fs::write_file(
            self.task.namespace().as_ref(),
            path,
            data,
            FileMode::from_perm(0o644),
        )
    }

    pub fn append_file(&self, path: &str, data: &[u8]) -> Result<()> {
        fs::append_file(self.task.namespace().as_ref(), path, data)
    }

    pub fn fstat(&self, fd: u32) -> Result<StatInfo> {
        self.task
            .with_fd_mut(fd, |file| file.stat().map(Into::into))
    }

    pub fn lstat(&self, path: &str) -> Result<StatInfo> {
        Ok(fs::lstat(self.task.namespace().as_ref(), path)?.into())
    }

    pub fn chmod(&self, path: &str, mode: u32) -> Result<()> {
        self.task.namespace().chmod(path, FileMode::from_bits(mode))
    }

    pub fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<()> {
        self.task.namespace().chown(path, uid, gid)
    }

    pub fn fchmod(&self, fd: u32, mode: u32) -> Result<()> {
        let path = self.task.fd_path(fd)?;
        self.chmod(&path, mode)
    }

    pub fn fchown(&self, fd: u32, uid: u32, gid: u32) -> Result<()> {
        let path = self.task.fd_path(fd)?;
        self.chown(&path, uid, gid)
    }

    pub fn ftruncate(&self, fd: u32, size: u64) -> Result<()> {
        let path = self.task.fd_path(fd)?;
        self.truncate(&path, size)
    }

    pub fn readlink(&self, path: &str) -> Result<String> {
        self.task.namespace().readlink(path)
    }

    pub fn symlink(&self, old: &str, new: &str) -> Result<()> {
        self.task.namespace().symlink(old, new)
    }

    pub fn chtimes(&self, path: &str, mtime: SystemTime) -> Result<()> {
        self.task.namespace().chtimes(path, mtime)
    }
}

fn unix_float_to_time(value: f64) -> SystemTime {
    let secs = value.trunc() as u64;
    let nanos = ((value.fract()) * 1_000_000_000.0) as u32;
    SystemTime::UNIX_EPOCH + Duration::new(secs, nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{fs_ref, MemFs};
    use wanix_task::TaskFs;
    use wanix_vfs::BindMode;

    fn api_with_memfs() -> WanixApi {
        let task = TaskFs::new().alloc("auto", None).unwrap();
        let mem = fs_ref(MemFs::new());
        task.namespace()
            .bind(mem, ".", ".", BindMode::Replace)
            .unwrap();
        WanixApi::new(task)
    }

    #[test]
    fn dispatches_js_handle_file_operations() {
        let api = api_with_memfs();
        api.dispatch(ApiRequest::WriteFile("a".into(), b"one".to_vec()))
            .unwrap();
        assert_eq!(
            api.dispatch(ApiRequest::ReadFile("a".into())).unwrap(),
            ApiResponse::Bytes(b"one".to_vec())
        );
        let fd = match api
            .dispatch(ApiRequest::OpenFile(
                "a".into(),
                (OpenFlags::WRONLY | OpenFlags::APPEND).bits(),
                0,
            ))
            .unwrap()
        {
            ApiResponse::Fd(fd) => fd,
            other => panic!("unexpected response: {other:?}"),
        };
        api.dispatch(ApiRequest::Write(fd, b"two".to_vec()))
            .unwrap();
        api.dispatch(ApiRequest::Close(fd)).unwrap();
        assert_eq!(api.read_file("a").unwrap(), b"onetwo");
    }

    #[test]
    fn read_returns_none_at_eof_like_handle_null() {
        let api = api_with_memfs();
        api.write_file("a", b"x").unwrap();
        let fd = api.open("a").unwrap();
        assert_eq!(api.read(fd, 8).unwrap(), Some(b"x".to_vec()));
        assert_eq!(api.read(fd, 8).unwrap(), None);
    }

    #[test]
    fn cbor_round_trips_requests_and_responses() {
        let request = ApiRequest::WriteAt(7, b"abc".to_vec(), 42);
        let encoded = encode_request(&request).unwrap();
        assert_eq!(decode_request(&encoded).unwrap(), request);

        let response = ApiResponse::Stat(StatInfo {
            size: 3,
            mode: 0o100644,
            is_dir: false,
            modified_ms: 10,
        });
        let encoded = encode_response(&response).unwrap();
        assert_eq!(decode_response(&encoded).unwrap(), response);
    }

    #[test]
    fn operation_fixture_covers_all_request_variants() {
        let fixture: Vec<String> =
            serde_json::from_str(include_str!("../../../tests/fixtures/api-operations.json"))
                .unwrap();
        let requests = vec![
            ApiRequest::Open("p".into()),
            ApiRequest::OpenFile("p".into(), 0, 0),
            ApiRequest::Create("p".into()),
            ApiRequest::Close(1),
            ApiRequest::Sync(1),
            ApiRequest::Read(1, 8),
            ApiRequest::Write(1, b"x".to_vec()),
            ApiRequest::WriteAt(1, b"x".to_vec(), 0),
            ApiRequest::ReadDir("p".into()),
            ApiRequest::Mkdir("p".into()),
            ApiRequest::MkdirAll("p".into()),
            ApiRequest::Bind("a".into(), "b".into()),
            ApiRequest::Unbind("a".into(), "b".into()),
            ApiRequest::Stat("p".into()),
            ApiRequest::Truncate("p".into(), 1),
            ApiRequest::WaitFor("p".into(), 1),
            ApiRequest::Rename("a".into(), "b".into()),
            ApiRequest::Copy("a".into(), "b".into()),
            ApiRequest::Remove("p".into()),
            ApiRequest::RemoveAll("p".into()),
            ApiRequest::ReadFile("p".into()),
            ApiRequest::WriteFile("p".into(), b"x".to_vec()),
            ApiRequest::AppendFile("p".into(), b"x".to_vec()),
            ApiRequest::Fstat(1),
            ApiRequest::Lstat("p".into()),
            ApiRequest::Chmod("p".into(), 0o644),
            ApiRequest::Chown("p".into(), 1, 2),
            ApiRequest::Fchmod(1, 0o644),
            ApiRequest::Fchown(1, 1, 2),
            ApiRequest::Ftruncate(1, 1),
            ApiRequest::Readlink("p".into()),
            ApiRequest::Symlink("a".into(), "b".into()),
            ApiRequest::Chtimes("p".into(), 1.0, 2.0),
        ];
        let methods: Vec<_> = requests
            .iter()
            .map(|request| request.method_name().to_string())
            .collect();
        assert_eq!(methods, fixture);
    }
}
