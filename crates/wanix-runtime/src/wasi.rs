use std::io::SeekFrom;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use wanix_core::{clean_path, Error, ErrorKind, FileMode, FsContext, OpenFlags, Result};
use wanix_fs::{read_file, FileHandle, FileSystem};
use wanix_protocol::runtime::{EnvironmentEntry, ExecutionSpec, ExitStatus};
use wanix_task::Task;
use wasmi::{Caller, Engine, Extern, Linker, Memory, Module, Store};

use crate::NativeExecutionHandler;

const WASI: &str = "wasi_snapshot_preview1";

const ERRNO_SUCCESS: i32 = 0;
const ERRNO_BADF: i32 = 8;
const ERRNO_EXIST: i32 = 20;
const ERRNO_INVAL: i32 = 28;
const ERRNO_IO: i32 = 29;
const ERRNO_ISDIR: i32 = 31;
const ERRNO_NOENT: i32 = 44;
const ERRNO_NOTDIR: i32 = 54;
const ERRNO_NOTEMPTY: i32 = 55;
const ERRNO_NOTSUP: i32 = 58;
const ERRNO_PERM: i32 = 63;

const FILETYPE_UNKNOWN: u8 = 0;
const FILETYPE_CHARACTER_DEVICE: u8 = 2;
const FILETYPE_DIRECTORY: u8 = 3;
const FILETYPE_REGULAR_FILE: u8 = 4;
const FILETYPE_SYMBOLIC_LINK: u8 = 7;

const OFLAGS_CREATE: i32 = 1;
const OFLAGS_DIRECTORY: i32 = 2;
const OFLAGS_TRUNCATE: i32 = 8;

const FDFLAGS_APPEND: i32 = 1;

const RIGHTS_FD_READ: i64 = 1 << 1;
const RIGHTS_FD_WRITE: i64 = 1 << 6;

const WHENCE_SET: i32 = 0;
const WHENCE_CUR: i32 = 1;
const WHENCE_END: i32 = 2;

const WASI_DIRENT_HEAD_SIZE: usize = 24;

#[derive(Clone, Default)]
pub struct WasmiWasiHandler;

#[derive(Clone)]
struct WasiState {
    task: Task,
    args: Vec<String>,
    env: Vec<EnvironmentEntry>,
}

impl WasmiWasiHandler {
    pub fn new() -> Self {
        Self
    }
}

impl NativeExecutionHandler for WasmiWasiHandler {
    fn execute(&self, task: &Task, spec: &ExecutionSpec) -> Result<ExitStatus> {
        ensure_default_preopen(task)?;
        let module_path = resolve_module_path(task, &spec.module);
        let module_bytes = read_file(task.namespace().as_ref(), &module_path).or_else(|err| {
            if module_path == spec.module {
                Err(err)
            } else {
                read_file(task.namespace().as_ref(), &spec.module)
            }
        })?;

        let engine = Engine::default();
        let module = Module::new(&engine, module_bytes.as_slice()).map_err(wasmi_error)?;
        let mut store = Store::new(
            &engine,
            WasiState {
                task: task.clone(),
                args: std::iter::once(spec.module.clone())
                    .chain(spec.args.iter().cloned())
                    .collect(),
                env: spec.env.clone(),
            },
        );
        let mut linker = Linker::new(&engine);
        add_wasi_imports(&mut linker)?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(wasmi_error)?
            .start(&mut store)
            .map_err(wasmi_error)?;
        let start = instance
            .get_typed_func::<(), ()>(&store, "_start")
            .map_err(wasmi_error)?;

        match start.call(&mut store, ()) {
            Ok(()) => Ok(ExitStatus::ExitCode(0)),
            Err(err) => match err.i32_exit_status() {
                Some(code) => Ok(ExitStatus::ExitCode(code)),
                None => Err(wasmi_error(err)),
            },
        }
    }
}

fn add_wasi_imports(linker: &mut Linker<WasiState>) -> Result<()> {
    linker
        .func_wrap(WASI, "args_sizes_get", args_sizes_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "args_get", args_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "environ_sizes_get", environ_sizes_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "environ_get", environ_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "proc_exit", proc_exit)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_write", fd_write)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_read", fd_read)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_pwrite", fd_pwrite)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_pread", fd_pread)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_close", fd_close)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_seek", fd_seek)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_tell", fd_tell)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_allocate", fd_allocate)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_fdstat_get", fd_fdstat_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_filestat_get", fd_filestat_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_filestat_set_size", fd_filestat_set_size)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_prestat_get", fd_prestat_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_prestat_dir_name", fd_prestat_dir_name)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_sync", fd_sync)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_datasync", fd_datasync)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "fd_readdir", fd_readdir)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_open", path_open)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_create_directory", path_create_directory)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_filestat_get", path_filestat_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_unlink_file", path_unlink_file)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_remove_directory", path_remove_directory)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_rename", path_rename)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_symlink", path_symlink)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "path_readlink", path_readlink)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "random_get", random_get)
        .map_err(wasmi_error)?;
    linker
        .func_wrap(WASI, "clock_time_get", clock_time_get)
        .map_err(wasmi_error)?;
    Ok(())
}

fn args_sizes_get(mut caller: Caller<'_, WasiState>, count_ptr: i32, size_ptr: i32) -> i32 {
    let args = caller.data().args.clone();
    let size = strings_size(args.iter().map(String::as_str));
    write_u32(&mut caller, count_ptr, args.len() as u32)
        .and_then(|()| write_u32(&mut caller, size_ptr, size as u32))
        .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn args_get(mut caller: Caller<'_, WasiState>, argv_ptr: i32, argv_buf_ptr: i32) -> i32 {
    let args = caller.data().args.clone();
    write_string_array(
        &mut caller,
        argv_ptr,
        argv_buf_ptr,
        args.iter().map(String::as_str),
    )
    .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn environ_sizes_get(mut caller: Caller<'_, WasiState>, count_ptr: i32, size_ptr: i32) -> i32 {
    let env = caller.data().env.clone();
    let rendered = render_env(&env);
    let size = strings_size(rendered.iter().map(String::as_str));
    write_u32(&mut caller, count_ptr, rendered.len() as u32)
        .and_then(|()| write_u32(&mut caller, size_ptr, size as u32))
        .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn environ_get(mut caller: Caller<'_, WasiState>, env_ptr: i32, env_buf_ptr: i32) -> i32 {
    let rendered = render_env(&caller.data().env);
    write_string_array(
        &mut caller,
        env_ptr,
        env_buf_ptr,
        rendered.iter().map(String::as_str),
    )
    .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn proc_exit(_caller: Caller<'_, WasiState>, code: i32) -> std::result::Result<(), wasmi::Error> {
    Err(wasmi::Error::i32_exit(code))
}

fn fd_write(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nwritten_ptr: i32,
) -> i32 {
    let buffers = match read_iovs(&mut caller, iovs_ptr, iovs_len) {
        Ok(buffers) => buffers,
        Err(errno) => return errno,
    };
    let task = caller.data().task.clone();
    let mut written = 0_u32;
    for data in buffers {
        let result = task.with_fd_mut(fd as u32, |file| {
            let n = file.write(&data)?;
            match file.sync() {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotSupported => {}
                Err(err) => return Err(err),
            }
            Ok(n)
        });
        match result {
            Ok(n) => written = written.saturating_add(n as u32),
            Err(err) => return errno_from_error(&err),
        }
    }
    write_u32(&mut caller, nwritten_ptr, written).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_read(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nread_ptr: i32,
) -> i32 {
    let iovs = match read_iov_descriptors(&mut caller, iovs_ptr, iovs_len) {
        Ok(iovs) => iovs,
        Err(errno) => return errno,
    };
    let task = caller.data().task.clone();
    let mut total = 0_u32;
    for (ptr, len) in iovs {
        let mut buf = vec![0_u8; len as usize];
        let result = task.with_fd_mut(fd as u32, |file| file.read(&mut buf));
        let n = match result {
            Ok(n) => n,
            Err(err) => return errno_from_error(&err),
        };
        if write_bytes(&mut caller, ptr, &buf[..n]).is_err() {
            return ERRNO_INVAL;
        }
        total = total.saturating_add(n as u32);
        if n < len as usize {
            break;
        }
    }
    write_u32(&mut caller, nread_ptr, total).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_pwrite(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    offset: i64,
    nwritten_ptr: i32,
) -> i32 {
    if offset < 0 {
        return ERRNO_INVAL;
    }
    let buffers = match read_iovs(&mut caller, iovs_ptr, iovs_len) {
        Ok(buffers) => buffers,
        Err(errno) => return errno,
    };
    let task = caller.data().task.clone();
    let mut written = 0_u32;
    let mut current_offset = offset as u64;
    for data in buffers {
        let result = task.with_fd_mut(fd as u32, |file| {
            with_preserved_offset(file, |file| {
                let n = file.write_at(&data, current_offset)?;
                sync_ignoring_unsupported(file)?;
                Ok(n)
            })
        });
        match result {
            Ok(n) => {
                written = written.saturating_add(n as u32);
                current_offset = current_offset.saturating_add(n as u64);
                if n < data.len() {
                    break;
                }
            }
            Err(err) => return errno_from_error(&err),
        }
    }
    write_u32(&mut caller, nwritten_ptr, written).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_pread(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    offset: i64,
    nread_ptr: i32,
) -> i32 {
    if offset < 0 {
        return ERRNO_INVAL;
    }
    let iovs = match read_iov_descriptors(&mut caller, iovs_ptr, iovs_len) {
        Ok(iovs) => iovs,
        Err(errno) => return errno,
    };
    let task = caller.data().task.clone();
    let mut total = 0_u32;
    let mut current_offset = offset as u64;
    for (ptr, len) in iovs {
        let mut buf = vec![0_u8; len as usize];
        let result = task.with_fd_mut(fd as u32, |file| {
            with_preserved_offset(file, |file| file.read_at(&mut buf, current_offset))
        });
        let n = match result {
            Ok(n) => n,
            Err(err) => return errno_from_error(&err),
        };
        if write_bytes(&mut caller, ptr, &buf[..n]).is_err() {
            return ERRNO_INVAL;
        }
        total = total.saturating_add(n as u32);
        current_offset = current_offset.saturating_add(n as u64);
        if n < len as usize {
            break;
        }
    }
    write_u32(&mut caller, nread_ptr, total).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_close(caller: Caller<'_, WasiState>, fd: i32) -> i32 {
    caller
        .data()
        .task
        .close_fd(fd as u32)
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn fd_seek(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    offset: i64,
    whence: i32,
    new_offset_ptr: i32,
) -> i32 {
    let pos = match whence {
        WHENCE_SET if offset >= 0 => SeekFrom::Start(offset as u64),
        WHENCE_CUR => SeekFrom::Current(offset),
        WHENCE_END => SeekFrom::End(offset),
        _ => return ERRNO_INVAL,
    };
    let task = caller.data().task.clone();
    let new_offset = match task.with_fd_mut(fd as u32, |file| file.seek(pos)) {
        Ok(offset) => offset,
        Err(err) => return errno_from_error(&err),
    };
    write_u64(&mut caller, new_offset_ptr, new_offset).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_tell(mut caller: Caller<'_, WasiState>, fd: i32, offset_ptr: i32) -> i32 {
    let task = caller.data().task.clone();
    let offset = match task.with_fd_mut(fd as u32, |file| file.seek(SeekFrom::Current(0))) {
        Ok(offset) => offset,
        Err(err) => return errno_from_error(&err),
    };
    write_u64(&mut caller, offset_ptr, offset).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_allocate(caller: Caller<'_, WasiState>, fd: i32, offset: i64, len: i64) -> i32 {
    if offset < 0 || len < 0 {
        return ERRNO_INVAL;
    }
    let end = match (offset as u64).checked_add(len as u64) {
        Some(end) => end,
        None => return ERRNO_INVAL,
    };
    let task = caller.data().task.clone();
    task.with_fd_mut(fd as u32, |file| {
        let size = file.stat()?.size;
        if end > size {
            with_preserved_offset(file, |file| {
                file.write_at(&[0], end - 1)?;
                sync_ignoring_unsupported(file)
            })?;
        }
        Ok(())
    })
    .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn fd_fdstat_get(mut caller: Caller<'_, WasiState>, fd: i32, stat_ptr: i32) -> i32 {
    let task = caller.data().task.clone();
    let meta = match task.with_fd_mut(fd as u32, |file| file.stat()) {
        Ok(meta) => meta,
        Err(err) => return errno_from_error(&err),
    };
    write_fdstat(&mut caller, stat_ptr, meta.mode).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_filestat_get(mut caller: Caller<'_, WasiState>, fd: i32, stat_ptr: i32) -> i32 {
    let task = caller.data().task.clone();
    let meta = match task.with_fd_mut(fd as u32, |file| file.stat()) {
        Ok(meta) => meta,
        Err(err) => return errno_from_error(&err),
    };
    write_filestat(&mut caller, stat_ptr, meta.mode, meta.size)
        .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_filestat_set_size(caller: Caller<'_, WasiState>, fd: i32, size: i64) -> i32 {
    if size < 0 {
        return ERRNO_INVAL;
    }
    let task = caller.data().task.clone();
    let path = match task.fd_path(fd as u32) {
        Ok(path) => path,
        Err(err) => return errno_from_error(&err),
    };
    task.namespace()
        .truncate(&path, size as u64)
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn fd_sync(caller: Caller<'_, WasiState>, fd: i32) -> i32 {
    sync_task_fd(&caller.data().task, fd)
}

fn fd_datasync(caller: Caller<'_, WasiState>, fd: i32) -> i32 {
    sync_task_fd(&caller.data().task, fd)
}

fn fd_readdir(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    buf_ptr: i32,
    buf_len: i32,
    cookie: i64,
    bufused_ptr: i32,
) -> i32 {
    if buf_len < 0 || cookie < 0 {
        return ERRNO_INVAL;
    }
    let cookie = match usize::try_from(cookie as u64) {
        Ok(cookie) => cookie,
        Err(_) => return ERRNO_INVAL,
    };
    let entries = match wasi_readdir_entries(&caller.data().task, fd as u32) {
        Ok(entries) => entries,
        Err(err) => return errno_from_error(&err),
    };

    let mut cursor = buf_ptr;
    let mut bufused = 0_usize;
    let buf_len = buf_len as usize;

    for entry in entries.into_iter().skip(cookie) {
        if buf_len.saturating_sub(bufused) < WASI_DIRENT_HEAD_SIZE {
            bufused = buf_len;
            break;
        }
        if write_bytes(&mut caller, cursor, &entry.head_bytes()).is_err() {
            return ERRNO_INVAL;
        }
        cursor += WASI_DIRENT_HEAD_SIZE as i32;
        bufused += WASI_DIRENT_HEAD_SIZE;

        if buf_len.saturating_sub(bufused) < entry.name.len() {
            bufused = buf_len;
            break;
        }
        if write_bytes(&mut caller, cursor, &entry.name).is_err() {
            return ERRNO_INVAL;
        }
        cursor += entry.name.len() as i32;
        bufused += entry.name.len();
    }

    write_u32(&mut caller, bufused_ptr, bufused as u32).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_prestat_get(mut caller: Caller<'_, WasiState>, fd: i32, prestat_ptr: i32) -> i32 {
    let path = match caller.data().task.fd_path(fd as u32) {
        Ok(path) => path,
        Err(err) => return errno_from_error(&err),
    };
    let guest = preopen_guest_path(&path);
    write_bytes(&mut caller, prestat_ptr, &[0, 0, 0, 0])
        .and_then(|()| write_u32(&mut caller, prestat_ptr + 4, guest.len() as u32))
        .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn fd_prestat_dir_name(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> i32 {
    let path = match caller.data().task.fd_path(fd as u32) {
        Ok(path) => path,
        Err(err) => return errno_from_error(&err),
    };
    let guest = preopen_guest_path(&path);
    let bytes = guest.as_bytes();
    if bytes.len() > path_len as usize {
        return ERRNO_INVAL;
    }
    write_bytes(&mut caller, path_ptr, bytes).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

#[allow(clippy::too_many_arguments)]
fn path_open(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    _dirflags: i32,
    path_ptr: i32,
    path_len: i32,
    oflags: i32,
    rights_base: i64,
    _rights_inheriting: i64,
    fdflags: i32,
    opened_fd_ptr: i32,
) -> i32 {
    let task = caller.data().task.clone();
    let path = match read_wasi_path(&mut caller, &task, fd, path_ptr, path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let file = if (oflags & OFLAGS_DIRECTORY) != 0 {
        task.namespace().open(&FsContext::new(), &path)
    } else {
        let flags = open_flags(oflags, fdflags, rights_base);
        task.namespace()
            .open_file(&path, flags, FileMode::from_perm(0o666))
    };
    let file = match file {
        Ok(file) => file,
        Err(err) => return errno_from_error(&err),
    };
    let opened_fd = task.open_fd(file, path);
    write_u32(&mut caller, opened_fd_ptr, opened_fd).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn path_create_directory(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> i32 {
    let task = caller.data().task.clone();
    let path = match read_wasi_path(&mut caller, &task, fd, path_ptr, path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    task.namespace()
        .mkdir(&path, FileMode::DIR | FileMode::from_perm(0o755))
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn path_filestat_get(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    _flags: i32,
    path_ptr: i32,
    path_len: i32,
    stat_ptr: i32,
) -> i32 {
    let task = caller.data().task.clone();
    let path = match read_wasi_path(&mut caller, &task, fd, path_ptr, path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let meta = match task.namespace().stat(&FsContext::new(), &path) {
        Ok(meta) => meta,
        Err(err) => return errno_from_error(&err),
    };
    write_filestat(&mut caller, stat_ptr, meta.mode, meta.size)
        .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn path_unlink_file(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> i32 {
    let task = caller.data().task.clone();
    let path = match read_wasi_path(&mut caller, &task, fd, path_ptr, path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let meta = match task.namespace().lstat(&FsContext::new(), &path) {
        Ok(meta) => meta,
        Err(err) => return errno_from_error(&err),
    };
    if meta.is_dir() {
        return ERRNO_ISDIR;
    }
    task.namespace()
        .remove(&path)
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn path_remove_directory(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> i32 {
    let task = caller.data().task.clone();
    let path = match read_wasi_path(&mut caller, &task, fd, path_ptr, path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let meta = match task.namespace().lstat(&FsContext::new(), &path) {
        Ok(meta) => meta,
        Err(err) => return errno_from_error(&err),
    };
    if !meta.is_dir() {
        return ERRNO_NOTDIR;
    }
    match task.namespace().read_dir(&FsContext::new(), &path) {
        Ok(entries) if !entries.is_empty() => return errno_from_error(&ErrorKind::NotEmpty.into()),
        Ok(_) => {}
        Err(err) => return errno_from_error(&err),
    }
    task.namespace()
        .remove(&path)
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn path_rename(
    mut caller: Caller<'_, WasiState>,
    old_fd: i32,
    old_path_ptr: i32,
    old_path_len: i32,
    new_fd: i32,
    new_path_ptr: i32,
    new_path_len: i32,
) -> i32 {
    let task = caller.data().task.clone();
    let old_path = match read_wasi_path(&mut caller, &task, old_fd, old_path_ptr, old_path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let new_path = match read_wasi_path(&mut caller, &task, new_fd, new_path_ptr, new_path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    task.namespace()
        .rename(&old_path, &new_path)
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn path_symlink(
    mut caller: Caller<'_, WasiState>,
    old_path_ptr: i32,
    old_path_len: i32,
    fd: i32,
    new_path_ptr: i32,
    new_path_len: i32,
) -> i32 {
    let old_path = match read_string(&mut caller, old_path_ptr, old_path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let task = caller.data().task.clone();
    let new_path = match read_wasi_path(&mut caller, &task, fd, new_path_ptr, new_path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    task.namespace()
        .symlink(&old_path, &new_path)
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn path_readlink(
    mut caller: Caller<'_, WasiState>,
    fd: i32,
    path_ptr: i32,
    path_len: i32,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> i32 {
    if buf_len < 0 {
        return ERRNO_INVAL;
    }
    let task = caller.data().task.clone();
    let path = match read_wasi_path(&mut caller, &task, fd, path_ptr, path_len) {
        Ok(path) => path,
        Err(errno) => return errno,
    };
    let target = match task.namespace().readlink(&path) {
        Ok(target) => target,
        Err(err) => return errno_from_error(&err),
    };
    let bytes = target.as_bytes();
    let copied = bytes.len().min(buf_len as usize);
    write_bytes(&mut caller, buf_ptr, &bytes[..copied])
        .and_then(|()| write_u32(&mut caller, used_ptr, copied as u32))
        .map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn random_get(mut caller: Caller<'_, WasiState>, buf_ptr: i32, buf_len: i32) -> i32 {
    if buf_len < 0 {
        return ERRNO_INVAL;
    }
    let mut bytes = vec![0_u8; buf_len as usize];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(31).wrapping_add(17);
    }
    write_bytes(&mut caller, buf_ptr, &bytes).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn clock_time_get(
    mut caller: Caller<'_, WasiState>,
    _clock_id: i32,
    _precision: i64,
    time_ptr: i32,
) -> i32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    write_u64(&mut caller, time_ptr, nanos).map_or(ERRNO_INVAL, |_| ERRNO_SUCCESS)
}

fn ensure_default_preopen(task: &Task) -> Result<()> {
    if task.fd_path(3).is_ok() {
        return Ok(());
    }
    let dir = normalize_task_path(&task.dir());
    let file = task.namespace().open(&FsContext::new(), &dir)?;
    task.set_fd(3, file, dir);
    Ok(())
}

fn resolve_module_path(task: &Task, module: &str) -> String {
    if module.starts_with('/') {
        clean_path(module.trim_start_matches('/'))
    } else {
        join_wasi_path(&task.dir(), module)
    }
}

fn normalize_task_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        ".".to_string()
    } else {
        clean_path(trimmed.trim_start_matches('/'))
    }
}

fn join_wasi_path(base: &str, rel: &str) -> String {
    let rel = rel.trim_start_matches('/');
    let rel = clean_path(rel);
    let base = normalize_task_path(base);
    if base == "." {
        rel
    } else if rel == "." {
        base
    } else {
        clean_path(&format!("{base}/{rel}"))
    }
}

fn read_wasi_path(
    caller: &mut Caller<'_, WasiState>,
    task: &Task,
    fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> std::result::Result<String, i32> {
    let rel = read_string(caller, path_ptr, path_len)?;
    let base = task
        .fd_path(fd as u32)
        .map_err(|err| errno_from_error(&err))?;
    Ok(join_wasi_path(&base, &rel))
}

fn preopen_guest_path(path: &str) -> String {
    if normalize_task_path(path) == "." {
        "/".to_string()
    } else {
        format!("/{}", normalize_task_path(path))
    }
}

fn parent_wasi_path(path: &str) -> String {
    let path = normalize_task_path(path);
    if path == "." {
        return path;
    }
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

fn sync_task_fd(task: &Task, fd: i32) -> i32 {
    task.with_fd_mut(fd as u32, |file| sync_ignoring_unsupported(file))
        .map_or_else(|err| errno_from_error(&err), |_| ERRNO_SUCCESS)
}

fn sync_ignoring_unsupported(file: &mut dyn FileHandle) -> Result<()> {
    match file.sync() {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotSupported => Ok(()),
        Err(err) => Err(err),
    }
}

fn with_preserved_offset<T>(
    file: &mut dyn FileHandle,
    op: impl FnOnce(&mut dyn FileHandle) -> Result<T>,
) -> Result<T> {
    let original = file.seek(SeekFrom::Current(0)).ok();
    let result = op(file);
    if let Some(original) = original {
        match file.seek(SeekFrom::Start(original)) {
            Ok(_) => {}
            Err(err) if result.is_ok() => return Err(err),
            Err(_) => {}
        }
    }
    result
}

fn wasi_readdir_entries(task: &Task, fd: u32) -> Result<Vec<WasiDirentRecord>> {
    let path = task.fd_path(fd)?;
    let meta = task.namespace().stat(&FsContext::new(), &path)?;
    if !meta.is_dir() {
        return Err(Error::path("readdir", &path, ErrorKind::NotDir));
    }

    let parent = parent_wasi_path(&path);
    let mut out = vec![
        WasiDirentRecord::new(1, wasi_inode(&path), FILETYPE_DIRECTORY, "."),
        WasiDirentRecord::new(2, wasi_inode(&parent), FILETYPE_DIRECTORY, ".."),
    ];

    for (index, entry) in task
        .namespace()
        .read_dir(&FsContext::new(), &path)?
        .into_iter()
        .enumerate()
    {
        let child = join_wasi_path(&path, &entry.name);
        out.push(WasiDirentRecord::new(
            index as u64 + 3,
            wasi_inode(&child),
            filetype(entry.metadata.mode),
            entry.name,
        ));
    }
    Ok(out)
}

fn wasi_inode(path: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    normalize_task_path(path).hash(&mut hasher);
    let hash = hasher.finish();
    if hash == 0 {
        1
    } else {
        hash
    }
}

fn open_flags(oflags: i32, fdflags: i32, rights_base: i64) -> OpenFlags {
    let mut flags = if (rights_base & RIGHTS_FD_WRITE) != 0 {
        OpenFlags::RDWR
    } else {
        OpenFlags::RDONLY
    };
    if (rights_base & RIGHTS_FD_READ) == 0 && (rights_base & RIGHTS_FD_WRITE) != 0 {
        flags = OpenFlags::WRONLY;
    }
    if (oflags & OFLAGS_CREATE) != 0 {
        flags |= OpenFlags::CREATE;
    }
    if (oflags & OFLAGS_TRUNCATE) != 0 {
        flags |= OpenFlags::TRUNC;
    }
    if (fdflags & FDFLAGS_APPEND) != 0 {
        flags |= OpenFlags::APPEND;
    }
    flags
}

struct WasiDirentRecord {
    d_next: u64,
    d_ino: u64,
    d_type: u8,
    name: Vec<u8>,
}

impl WasiDirentRecord {
    fn new(d_next: u64, d_ino: u64, d_type: u8, name: impl AsRef<str>) -> Self {
        Self {
            d_next,
            d_ino,
            d_type,
            name: name.as_ref().as_bytes().to_vec(),
        }
    }

    fn head_bytes(&self) -> [u8; WASI_DIRENT_HEAD_SIZE] {
        let mut bytes = [0_u8; WASI_DIRENT_HEAD_SIZE];
        bytes[..8].copy_from_slice(&self.d_next.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.d_ino.to_le_bytes());
        bytes[16..20].copy_from_slice(&(self.name.len() as u32).to_le_bytes());
        bytes[20] = self.d_type;
        bytes
    }
}

fn strings_size<'a>(strings: impl Iterator<Item = &'a str>) -> usize {
    strings.map(|value| value.len() + 1).sum()
}

fn render_env(env: &[EnvironmentEntry]) -> Vec<String> {
    env.iter()
        .map(|entry| format!("{}={}", entry.name, entry.value))
        .collect()
}

fn write_string_array<'a>(
    caller: &mut Caller<'_, WasiState>,
    ptrs_ptr: i32,
    buf_ptr: i32,
    strings: impl Iterator<Item = &'a str>,
) -> std::result::Result<(), ()> {
    let mut cursor = buf_ptr;
    for (index, value) in strings.enumerate() {
        write_u32(caller, ptrs_ptr + (index as i32 * 4), cursor as u32)?;
        write_bytes(caller, cursor, value.as_bytes())?;
        cursor += value.len() as i32;
        write_bytes(caller, cursor, &[0])?;
        cursor += 1;
    }
    Ok(())
}

fn read_iovs(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    len: i32,
) -> std::result::Result<Vec<Vec<u8>>, i32> {
    read_iov_descriptors(caller, ptr, len)?
        .into_iter()
        .map(|(data_ptr, data_len)| {
            read_bytes(caller, data_ptr, data_len).map_err(|()| ERRNO_INVAL)
        })
        .collect()
}

fn read_iov_descriptors(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    len: i32,
) -> std::result::Result<Vec<(i32, i32)>, i32> {
    if len < 0 {
        return Err(ERRNO_INVAL);
    }
    let mut out = Vec::new();
    for index in 0..len {
        let offset = ptr + (index * 8);
        let data_ptr = read_u32(caller, offset).map_err(|()| ERRNO_INVAL)? as i32;
        let data_len = read_u32(caller, offset + 4).map_err(|()| ERRNO_INVAL)? as i32;
        out.push((data_ptr, data_len));
    }
    Ok(out)
}

fn read_string(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    len: i32,
) -> std::result::Result<String, i32> {
    if len < 0 {
        return Err(ERRNO_INVAL);
    }
    let bytes = read_bytes(caller, ptr, len).map_err(|()| ERRNO_INVAL)?;
    String::from_utf8(bytes).map_err(|_| ERRNO_INVAL)
}

fn read_bytes(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    len: i32,
) -> std::result::Result<Vec<u8>, ()> {
    if ptr < 0 || len < 0 {
        return Err(());
    }
    let memory = memory(caller).ok_or(())?;
    let data = memory.data(&*caller);
    let start = ptr as usize;
    let end = start.checked_add(len as usize).ok_or(())?;
    if end > data.len() {
        return Err(());
    }
    Ok(data[start..end].to_vec())
}

fn write_bytes(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    bytes: &[u8],
) -> std::result::Result<(), ()> {
    if ptr < 0 {
        return Err(());
    }
    let memory = memory(caller).ok_or(())?;
    let data = memory.data_mut(caller);
    let start = ptr as usize;
    let end = start.checked_add(bytes.len()).ok_or(())?;
    if end > data.len() {
        return Err(());
    }
    data[start..end].copy_from_slice(bytes);
    Ok(())
}

fn read_u32(caller: &mut Caller<'_, WasiState>, ptr: i32) -> std::result::Result<u32, ()> {
    let bytes = read_bytes(caller, ptr, 4)?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn write_u32(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    value: u32,
) -> std::result::Result<(), ()> {
    write_bytes(caller, ptr, &value.to_le_bytes())
}

fn write_u64(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    value: u64,
) -> std::result::Result<(), ()> {
    write_bytes(caller, ptr, &value.to_le_bytes())
}

fn write_fdstat(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    mode: FileMode,
) -> std::result::Result<(), ()> {
    write_bytes(caller, ptr, &[filetype(mode), 0])?;
    write_bytes(caller, ptr + 2, &0_u16.to_le_bytes())?;
    write_u64(caller, ptr + 8, u64::MAX)?;
    write_u64(caller, ptr + 16, u64::MAX)
}

fn write_filestat(
    caller: &mut Caller<'_, WasiState>,
    ptr: i32,
    mode: FileMode,
    size: u64,
) -> std::result::Result<(), ()> {
    write_u64(caller, ptr, 0)?;
    write_u64(caller, ptr + 8, 0)?;
    write_bytes(caller, ptr + 16, &[filetype(mode)])?;
    write_u64(caller, ptr + 24, 1)?;
    write_u64(caller, ptr + 32, size)?;
    write_u64(caller, ptr + 40, 0)?;
    write_u64(caller, ptr + 48, 0)?;
    write_u64(caller, ptr + 56, 0)
}

fn filetype(mode: FileMode) -> u8 {
    if mode.is_dir() {
        FILETYPE_DIRECTORY
    } else if mode.is_symlink() {
        FILETYPE_SYMBOLIC_LINK
    } else if mode.contains(FileMode::DEVICE) {
        FILETYPE_CHARACTER_DEVICE
    } else if mode.type_bits() == FileMode::empty() {
        FILETYPE_REGULAR_FILE
    } else {
        FILETYPE_UNKNOWN
    }
}

fn memory(caller: &Caller<'_, WasiState>) -> Option<Memory> {
    caller.get_export("memory").and_then(Extern::into_memory)
}

fn errno_from_error(err: &Error) -> i32 {
    match err.kind() {
        ErrorKind::NotFound => ERRNO_NOENT,
        ErrorKind::AlreadyExists => ERRNO_EXIST,
        ErrorKind::Invalid | ErrorKind::UnexpectedEof => ERRNO_INVAL,
        ErrorKind::PermissionDenied => ERRNO_PERM,
        ErrorKind::NotSupported => ERRNO_NOTSUP,
        ErrorKind::NotDir => ERRNO_NOTDIR,
        ErrorKind::IsDir => ERRNO_ISDIR,
        ErrorKind::Closed => ERRNO_BADF,
        ErrorKind::NotEmpty => ERRNO_NOTEMPTY,
        ErrorKind::Other => ERRNO_IO,
    }
}

fn wasmi_error(err: impl std::fmt::Display) -> Error {
    Error::Message(format!("wasi execution failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{fs_ref, read_file, MemFs};
    use wanix_protocol::runtime::{
        EnvironmentEntry, ExecutionKind, ExecutionSpec, FdDescriptor, FdKind, StdioSet,
        StreamDescriptor,
    };
    use wanix_vfs::BindMode;

    #[test]
    fn wasi_errno_maps_not_empty_to_preview1_errno() {
        assert_eq!(
            errno_from_error(&ErrorKind::NotEmpty.into()),
            ERRNO_NOTEMPTY
        );
    }

    #[test]
    fn wasmi_wasi_handler_runs_preview1_against_task_namespace() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "args_sizes_get"
                (func $args_sizes_get (param i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "environ_sizes_get"
                (func $environ_sizes_get (param i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_read"
                (func $fd_read (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "input.txt")
              (data (i32.const 120) "output.txt")

              (func (export "_start")
                (drop (call $args_sizes_get (i32.const 44) (i32.const 48)))
                (drop (call $environ_sizes_get (i32.const 52) (i32.const 56)))

                (drop
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 9)
                    (i32.const 0)
                    (i64.const 2)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 40)))

                (i32.store (i32.const 16) (i32.const 200))
                (i32.store (i32.const 20) (i32.const 64))
                (drop
                  (call $fd_read
                    (i32.load (i32.const 40))
                    (i32.const 16)
                    (i32.const 1)
                    (i32.const 32)))
                (drop (call $fd_close (i32.load (i32.const 40))))

                (drop
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 120)
                    (i32.const 10)
                    (i32.const 9)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 40)))

                (i32.store8
                  (i32.const 198)
                  (i32.add (i32.load (i32.const 44)) (i32.const 48)))
                (i32.store8
                  (i32.const 199)
                  (i32.add (i32.load (i32.const 52)) (i32.const 48)))
                (i32.store (i32.const 16) (i32.const 198))
                (i32.store (i32.const 20) (i32.const 2))
                (i32.store (i32.const 24) (i32.const 200))
                (i32.store (i32.const 28) (i32.load (i32.const 32)))
                (drop
                  (call $fd_write
                    (i32.load (i32.const 40))
                    (i32.const 16)
                    (i32.const 2)
                    (i32.const 36)))
                (drop (call $fd_close (i32.load (i32.const 40))))
                (call $proc_exit (i32.const 5)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("input.txt", b"input-bytes".to_vec()),
                    ("output.txt", Vec::new()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();

        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: vec!["alpha".into()],
                    env: vec![
                        EnvironmentEntry {
                            name: "MODE".into(),
                            value: "test".into(),
                        },
                        EnvironmentEntry {
                            name: "USER".into(),
                            value: "wanix".into(),
                        },
                    ],
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(5));
        assert_eq!(task.exit(), "5");
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/output.txt").unwrap(),
            b"22input-bytes"
        );
    }

    #[test]
    fn wasmi_wasi_handler_writes_to_task_stdio_fds() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (memory (export "memory") 1)
              (data (i32.const 64) "stdio-ok")
              (func (export "_start")
                (i32.store (i32.const 0) (i32.const 64))
                (i32.store (i32.const 4) (i32.const 8))
                (drop
                  (call $fd_write
                    (i32.const 1)
                    (i32.const 0)
                    (i32.const 1)
                    (i32.const 16))))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("stdout.txt", Vec::new()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet {
                        stdin: StreamDescriptor::Null,
                        stdout: StreamDescriptor::Fd(FdDescriptor {
                            fd: 1,
                            kind: FdKind::File,
                            path: Some("workspace/stdout.txt".into()),
                            read: false,
                            write: true,
                        }),
                        stderr: StreamDescriptor::Null,
                    },
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/stdout.txt").unwrap(),
            b"stdio-ok"
        );
    }

    #[test]
    fn wasmi_wasi_handler_mutates_paths_with_preview1_syscalls() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_create_directory"
                (func $path_create_directory (param i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_rename"
                (func $path_rename (param i32 i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_symlink"
                (func $path_symlink (param i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_readlink"
                (func $path_readlink (param i32 i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_unlink_file"
                (func $path_unlink_file (param i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_remove_directory"
                (func $path_remove_directory (param i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "dir")
              (data (i32.const 120) "original.txt")
              (data (i32.const 140) "dir/renamed.txt")
              (data (i32.const 170) "renamed.txt")
              (data (i32.const 190) "link.txt")
              (data (i32.const 210) "readlink.txt")
              (data (i32.const 240) "payload")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func $assert_errno (param $errno i32) (param $want i32) (param $code i32)
                local.get $errno
                local.get $want
                i32.eq
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_create_directory
                    (i32.const 3)
                    (i32.const 100)
                    (i32.const 3))
                  (i32.const 10))

                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 3)
                    (i32.const 2)
                    (i64.const 0)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 11))

                (call $assert_ok
                  (call $path_open
                    (i32.load (i32.const 0))
                    (i32.const 0)
                    (i32.const 120)
                    (i32.const 12)
                    (i32.const 1)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 4))
                  (i32.const 12))

                (i32.store (i32.const 32) (i32.const 240))
                (i32.store (i32.const 36) (i32.const 7))
                (call $assert_ok
                  (call $fd_write
                    (i32.load (i32.const 4))
                    (i32.const 32)
                    (i32.const 1)
                    (i32.const 40))
                  (i32.const 13))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 4)))
                  (i32.const 14))

                (call $assert_ok
                  (call $path_rename
                    (i32.load (i32.const 0))
                    (i32.const 120)
                    (i32.const 12)
                    (i32.const 3)
                    (i32.const 140)
                    (i32.const 15))
                  (i32.const 15))

                (call $assert_errno
                  (call $path_open
                    (i32.load (i32.const 0))
                    (i32.const 0)
                    (i32.const 120)
                    (i32.const 12)
                    (i32.const 0)
                    (i64.const 2)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 8))
                  (i32.const 44)
                  (i32.const 16))

                (call $assert_ok
                  (call $path_symlink
                    (i32.const 170)
                    (i32.const 11)
                    (i32.load (i32.const 0))
                    (i32.const 190)
                    (i32.const 8))
                  (i32.const 17))

                (call $assert_ok
                  (call $path_readlink
                    (i32.load (i32.const 0))
                    (i32.const 190)
                    (i32.const 8)
                    (i32.const 256)
                    (i32.const 64)
                    (i32.const 60))
                  (i32.const 18))

                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 210)
                    (i32.const 12)
                    (i32.const 1)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 12))
                  (i32.const 19))

                (i32.store (i32.const 32) (i32.const 256))
                (i32.store (i32.const 36) (i32.load (i32.const 60)))
                (call $assert_ok
                  (call $fd_write
                    (i32.load (i32.const 12))
                    (i32.const 32)
                    (i32.const 1)
                    (i32.const 40))
                  (i32.const 20))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 12)))
                  (i32.const 21))

                (call $assert_ok
                  (call $path_unlink_file
                    (i32.load (i32.const 0))
                    (i32.const 190)
                    (i32.const 8))
                  (i32.const 22))
                (call $assert_ok
                  (call $path_unlink_file
                    (i32.load (i32.const 0))
                    (i32.const 170)
                    (i32.const 11))
                  (i32.const 23))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 24))
                (call $assert_ok
                  (call $path_remove_directory
                    (i32.const 3)
                    (i32.const 100)
                    (i32.const 3))
                  (i32.const 25)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([("program.wasm", program)])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/readlink.txt").unwrap(),
            b"renamed.txt"
        );
        assert_eq!(
            task.namespace()
                .stat(&FsContext::new(), "workspace/dir")
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn wasmi_wasi_handler_path_readlink_rejects_regular_files() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "path_readlink"
                (func $path_readlink (param i32 i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "plain.txt")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func $assert_errno (param $errno i32) (param $want i32) (param $code i32)
                local.get $errno
                local.get $want
                i32.eq
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 9)
                    (i32.const 1)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 10))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 11))
                (call $assert_errno
                  (call $path_readlink
                    (i32.const 3)
                    (i32.const 100)
                    (i32.const 9)
                    (i32.const 128)
                    (i32.const 32)
                    (i32.const 16))
                  (i32.const 28)
                  (i32.const 12)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([("program.wasm", program)])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
    }

    #[derive(Debug)]
    struct ParsedDirent {
        d_next: u64,
        d_type: u8,
        name: String,
    }

    fn parse_dirents(bytes: &[u8]) -> Vec<ParsedDirent> {
        let mut offset = 0;
        let mut out = Vec::new();
        while offset < bytes.len() {
            assert!(offset + WASI_DIRENT_HEAD_SIZE <= bytes.len());
            let d_next = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
            let d_namlen =
                u32::from_le_bytes(bytes[offset + 16..offset + 20].try_into().unwrap()) as usize;
            let d_type = bytes[offset + 20];
            let name_start = offset + WASI_DIRENT_HEAD_SIZE;
            let name_end = name_start + d_namlen;
            assert!(name_end <= bytes.len());
            out.push(ParsedDirent {
                d_next,
                d_type,
                name: String::from_utf8(bytes[name_start..name_end].to_vec()).unwrap(),
            });
            offset = name_end;
        }
        out
    }

    #[test]
    fn wasmi_wasi_handler_readdir_lists_preopened_cwd_entries() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_readdir"
                (func $fd_readdir (param i32 i32 i32 i64 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "out.bin")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 7)
                    (i32.const 1)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 10))

                (call $assert_ok
                  (call $fd_readdir
                    (i32.const 3)
                    (i32.const 256)
                    (i32.const 25)
                    (i64.const 0)
                    (i32.const 32))
                  (i32.const 11))
                (i32.store (i32.const 48) (i32.const 256))
                (i32.store (i32.const 52) (i32.load (i32.const 32)))
                (call $assert_ok
                  (call $fd_write
                    (i32.load (i32.const 0))
                    (i32.const 48)
                    (i32.const 1)
                    (i32.const 40))
                  (i32.const 12))

                (call $assert_ok
                  (call $fd_readdir
                    (i32.const 3)
                    (i32.const 320)
                    (i32.const 512)
                    (i64.load (i32.const 256))
                    (i32.const 36))
                  (i32.const 13))
                (i32.store (i32.const 48) (i32.const 320))
                (i32.store (i32.const 52) (i32.load (i32.const 36)))
                (call $assert_ok
                  (call $fd_write
                    (i32.load (i32.const 0))
                    (i32.const 48)
                    (i32.const 1)
                    (i32.const 44))
                  (i32.const 14))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 15)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("alpha.txt", b"alpha".to_vec()),
                    ("beta.txt", b"beta".to_vec()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        let raw = read_file(task.namespace().as_ref(), "workspace/out.bin").unwrap();
        assert!(raw.windows(1).any(|window| window == b"."));
        assert!(raw.windows(2).any(|window| window == b".."));
        assert!(raw
            .windows("alpha.txt".len())
            .any(|window| window == b"alpha.txt"));
        assert!(raw
            .windows("beta.txt".len())
            .any(|window| window == b"beta.txt"));

        let dirents = parse_dirents(&raw);
        let names: Vec<_> = dirents.iter().map(|dirent| dirent.name.as_str()).collect();
        assert_eq!(names.first().copied(), Some("."));
        assert_eq!(names.get(1).copied(), Some(".."));
        assert!(names.contains(&"alpha.txt"));
        assert!(names.contains(&"beta.txt"));
        assert!(names.contains(&"out.bin"));
        assert!(names.contains(&"program.wasm"));
        assert_eq!(dirents[0].d_next, 1);
        assert_eq!(dirents[0].d_type, FILETYPE_DIRECTORY);
        assert_eq!(dirents[1].d_next, 2);
        assert_eq!(dirents[1].d_type, FILETYPE_DIRECTORY);
    }

    #[test]
    fn wasmi_wasi_handler_filestat_set_size_truncates_namespace_files() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_filestat_set_size"
                (func $fd_filestat_set_size (param i32 i64) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "data.txt")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func $assert_errno (param $errno i32) (param $want i32) (param $code i32)
                local.get $errno
                local.get $want
                i32.eq
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 8)
                    (i32.const 0)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 10))
                (call $assert_errno
                  (call $fd_filestat_set_size
                    (i32.load (i32.const 0))
                    (i64.const -1))
                  (i32.const 28)
                  (i32.const 11))
                (call $assert_ok
                  (call $fd_filestat_set_size
                    (i32.load (i32.const 0))
                    (i64.const 4))
                  (i32.const 12))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 13)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("data.txt", b"truncate-me".to_vec()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/data.txt").unwrap(),
            b"trun"
        );
    }

    #[test]
    fn wasmi_wasi_handler_supports_positional_fd_io_and_allocate() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_pwrite"
                (func $fd_pwrite (param i32 i32 i32 i64 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_pread"
                (func $fd_pread (param i32 i32 i32 i64 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_tell"
                (func $fd_tell (param i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_seek"
                (func $fd_seek (param i32 i64 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_allocate"
                (func $fd_allocate (param i32 i64 i64) (result i32)))
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "data.txt")
              (data (i32.const 120) "out.bin")
              (data (i32.const 200) "XY")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func $assert_i32 (param $actual i32) (param $want i32) (param $code i32)
                local.get $actual
                local.get $want
                i32.eq
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func $assert_i64 (param $actual i64) (param $want i64) (param $code i32)
                local.get $actual
                local.get $want
                i64.eq
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 8)
                    (i32.const 0)
                    (i64.const 66)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 10))

                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 120)
                    (i32.const 7)
                    (i32.const 9)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 4))
                  (i32.const 11))

                (call $assert_ok
                  (call $fd_tell
                    (i32.load (i32.const 0))
                    (i32.const 16))
                  (i32.const 12))
                (call $assert_i64
                  (i64.load (i32.const 16))
                  (i64.const 0)
                  (i32.const 13))

                (i32.store (i32.const 40) (i32.const 200))
                (i32.store (i32.const 44) (i32.const 2))
                (call $assert_ok
                  (call $fd_pwrite
                    (i32.load (i32.const 0))
                    (i32.const 40)
                    (i32.const 1)
                    (i64.const 2)
                    (i32.const 24))
                  (i32.const 14))
                (call $assert_i32
                  (i32.load (i32.const 24))
                  (i32.const 2)
                  (i32.const 15))
                (call $assert_ok
                  (call $fd_tell
                    (i32.load (i32.const 0))
                    (i32.const 16))
                  (i32.const 16))
                (call $assert_i64
                  (i64.load (i32.const 16))
                  (i64.const 0)
                  (i32.const 17))

                (call $assert_ok
                  (call $fd_seek
                    (i32.load (i32.const 0))
                    (i64.const 5)
                    (i32.const 0)
                    (i32.const 16))
                  (i32.const 18))
                (call $assert_ok
                  (call $fd_allocate
                    (i32.load (i32.const 0))
                    (i64.const 10)
                    (i64.const 3))
                  (i32.const 19))
                (call $assert_ok
                  (call $fd_tell
                    (i32.load (i32.const 0))
                    (i32.const 16))
                  (i32.const 20))
                (call $assert_i64
                  (i64.load (i32.const 16))
                  (i64.const 5)
                  (i32.const 21))

                (i32.store (i32.const 48) (i32.const 256))
                (i32.store (i32.const 52) (i32.const 13))
                (call $assert_ok
                  (call $fd_pread
                    (i32.load (i32.const 0))
                    (i32.const 48)
                    (i32.const 1)
                    (i64.const 0)
                    (i32.const 28))
                  (i32.const 22))
                (call $assert_i32
                  (i32.load (i32.const 28))
                  (i32.const 13)
                  (i32.const 23))
                (call $assert_ok
                  (call $fd_tell
                    (i32.load (i32.const 0))
                    (i32.const 16))
                  (i32.const 24))
                (call $assert_i64
                  (i64.load (i32.const 16))
                  (i64.const 5)
                  (i32.const 25))

                (i32.store (i32.const 56) (i32.const 256))
                (i32.store (i32.const 60) (i32.load (i32.const 28)))
                (call $assert_ok
                  (call $fd_write
                    (i32.load (i32.const 4))
                    (i32.const 56)
                    (i32.const 1)
                    (i32.const 32))
                  (i32.const 26))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 27))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 4)))
                  (i32.const 28)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("data.txt", b"abcdef".to_vec()),
                    ("out.bin", Vec::new()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        let expected = b"abXYef\0\0\0\0\0\0\0";
        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/data.txt").unwrap(),
            expected
        );
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/out.bin").unwrap(),
            expected
        );
    }

    #[test]
    fn wasmi_wasi_handler_positional_fd_ops_reject_invalid_offsets() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_pwrite"
                (func $fd_pwrite (param i32 i32 i32 i64 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_pread"
                (func $fd_pread (param i32 i32 i32 i64 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_allocate"
                (func $fd_allocate (param i32 i64 i64) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "data.txt")
              (data (i32.const 200) "Z")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func $assert_errno (param $errno i32) (param $want i32) (param $code i32)
                local.get $errno
                local.get $want
                i32.eq
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 8)
                    (i32.const 0)
                    (i64.const 66)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 10))

                (i32.store (i32.const 40) (i32.const 200))
                (i32.store (i32.const 44) (i32.const 1))
                (i32.store (i32.const 48) (i32.const 256))
                (i32.store (i32.const 52) (i32.const 1))

                (call $assert_errno
                  (call $fd_pwrite
                    (i32.load (i32.const 0))
                    (i32.const 40)
                    (i32.const 1)
                    (i64.const -1)
                    (i32.const 16))
                  (i32.const 28)
                  (i32.const 11))
                (call $assert_errno
                  (call $fd_pread
                    (i32.load (i32.const 0))
                    (i32.const 48)
                    (i32.const 1)
                    (i64.const -1)
                    (i32.const 20))
                  (i32.const 28)
                  (i32.const 12))
                (call $assert_errno
                  (call $fd_allocate
                    (i32.load (i32.const 0))
                    (i64.const -1)
                    (i64.const 1))
                  (i32.const 28)
                  (i32.const 13))
                (call $assert_errno
                  (call $fd_allocate
                    (i32.load (i32.const 0))
                    (i64.const 1)
                    (i64.const -1))
                  (i32.const 28)
                  (i32.const 14))
                (call $assert_errno
                  (call $fd_allocate
                    (i32.const 999)
                    (i64.const 0)
                    (i64.const 0))
                  (i32.const 28)
                  (i32.const 15))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 16)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("data.txt", b"abc".to_vec()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
        assert_eq!(
            read_file(task.namespace().as_ref(), "workspace/data.txt").unwrap(),
            b"abc"
        );
    }

    #[test]
    fn wasmi_wasi_handler_sync_and_datasync_succeed_for_file_fds() {
        let runtime = crate::Runtime::new().unwrap();
        let task = runtime
            .task_fs()
            .alloc("auto", Some(runtime.root()))
            .unwrap();
        let program = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "path_open"
                (func $path_open
                  (param i32 i32 i32 i32 i32 i64 i64 i32 i32)
                  (result i32)))
              (import "wasi_snapshot_preview1" "fd_sync"
                (func $fd_sync (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_datasync"
                (func $fd_datasync (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_close"
                (func $fd_close (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))

              (memory (export "memory") 1)
              (data (i32.const 100) "data.txt")

              (func $assert_ok (param $errno i32) (param $code i32)
                local.get $errno
                i32.eqz
                if
                else
                  local.get $code
                  call $proc_exit
                end)

              (func (export "_start")
                (call $assert_ok
                  (call $path_open
                    (i32.const 3)
                    (i32.const 0)
                    (i32.const 100)
                    (i32.const 8)
                    (i32.const 0)
                    (i64.const 64)
                    (i64.const 0)
                    (i32.const 0)
                    (i32.const 0))
                  (i32.const 10))
                (call $assert_ok
                  (call $fd_sync (i32.load (i32.const 0)))
                  (i32.const 11))
                (call $assert_ok
                  (call $fd_datasync (i32.load (i32.const 0)))
                  (i32.const 12))
                (call $assert_ok
                  (call $fd_close (i32.load (i32.const 0)))
                  (i32.const 13)))
            )
            "#,
        )
        .unwrap();

        task.namespace()
            .bind(
                fs_ref(MemFs::from_entries([
                    ("program.wasm", program),
                    ("data.txt", b"sync-me".to_vec()),
                ])),
                ".",
                "workspace",
                BindMode::Replace,
            )
            .unwrap();
        runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

        let status = runtime
            .execution_registry()
            .execute(
                &task,
                &ExecutionSpec {
                    kind: ExecutionKind::Wasi,
                    module: "program.wasm".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    cwd: Some("workspace".into()),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            )
            .unwrap();

        assert_eq!(status, ExitStatus::ExitCode(0));
    }
}
