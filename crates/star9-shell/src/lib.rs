//! Host-neutral Star 9 shell core.
//!
//! The shell intentionally starts as a small Star 9 command surface rather than a
//! POSIX or Plan 9 rc compatibility layer. Commands route through Star 9
//! namespaces, task files, fd files, and device files.

pub mod rc;

use std::collections::BTreeMap;
use std::io::SeekFrom;

use star9_core::{clean_path, Error, ErrorKind, Result};
use star9_fs::{fs_ref, open, MemFs};
use star9_protocol::{
    runtime::{
        EnvironmentEntry, ExecutionKind, ExecutionSpec, FdDescriptor, FdKind, StdioSet,
        StreamDescriptor, WorkerHandle, WorkerSpawnRequest, WorkerStartRequest,
    },
    Star9Api, StatInfo,
};
use star9_runtime::{Runtime, WasmiWasiHandler};

#[cfg(not(target_arch = "wasm32"))]
use star9_runtime::NativePtyExecutionHandler;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub name: String,
    pub args: Vec<String>,
}

impl ShellCommand {
    fn new(words: Vec<String>) -> Option<Self> {
        let mut iter = words.into_iter();
        let name = iter.next()?;
        Some(Self {
            name,
            args: iter.collect(),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ShellResult {
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn failure(stderr: impl Into<String>) -> Self {
        Self {
            status: 1,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    fn append(&mut self, next: ShellResult) {
        self.status = next.status;
        self.stdout.push_str(&next.stdout);
        self.stderr.push_str(&next.stderr);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellTaskResult {
    pub task_id: String,
    pub status: String,
    pub stdout: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellStat {
    pub size: u64,
    pub mode: u32,
    pub is_dir: bool,
    pub modified_ms: u128,
}

impl From<StatInfo> for ShellStat {
    fn from(value: StatInfo) -> Self {
        Self {
            size: value.size,
            mode: value.mode,
            is_dir: value.is_dir,
            modified_ms: value.modified_ms,
        }
    }
}

pub trait ShellHost: Clone {
    fn read_file(&self, path: &str) -> Result<Vec<u8>>;
    fn write_file(&self, path: &str, data: &[u8]) -> Result<()>;
    fn append_file(&self, path: &str, data: &[u8]) -> Result<()>;
    fn write_existing(&self, path: &str, data: &[u8]) -> Result<()>;
    fn read_dir(&self, path: &str) -> Result<Vec<String>>;
    fn mkdir(&self, path: &str) -> Result<()>;
    fn remove(&self, path: &str) -> Result<()>;
    fn remove_all(&self, path: &str) -> Result<()>;
    fn rename(&self, old: &str, new: &str) -> Result<()>;
    fn copy(&self, old: &str, new: &str) -> Result<()>;
    fn stat(&self, path: &str) -> Result<ShellStat>;
    fn start_wasi(&self, module: &str, args: &[String], cwd: &str) -> Result<ShellTaskResult>;
    fn start_worker(&self, module: &str, args: &[String], cwd: &str) -> Result<ShellTaskResult>;
    fn run_native(&self, _module: &str, _args: &[String], _cwd: &str) -> Result<ShellTaskResult> {
        Err(ErrorKind::NotSupported.into())
    }
}

#[derive(Clone)]
pub struct RuntimeShellHost {
    runtime: Runtime,
    api: Star9Api,
    native_enabled: bool,
}

impl RuntimeShellHost {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            api: Star9Api::new(runtime.root()),
            runtime,
            native_enabled: false,
        }
    }

    pub fn fresh() -> Result<Self> {
        Self::new(Runtime::new()?).with_writable_workspace()
    }

    pub fn runtime(&self) -> Runtime {
        self.runtime.clone()
    }

    pub fn enable_native(mut self) -> Self {
        self.native_enabled = true;
        self
    }

    pub fn with_writable_workspace(self) -> Result<Self> {
        self.runtime.namespace().bind(
            fs_ref(MemFs::new()),
            ".",
            ".",
            star9_vfs::BindMode::Replace,
        )?;
        Ok(self)
    }

    fn execute_task(
        &self,
        kind: ExecutionKind,
        module: &str,
        args: &[String],
        cwd: &str,
        stdout_path: Option<String>,
    ) -> Result<ShellTaskResult> {
        let task = self
            .runtime
            .task_fs()
            .alloc("auto", Some(self.runtime.root()))?;
        let stdio = if let Some(path) = stdout_path {
            StdioSet {
                stdin: StreamDescriptor::Null,
                stdout: StreamDescriptor::Fd(FdDescriptor {
                    fd: 1,
                    kind: FdKind::File,
                    path: Some(path),
                    read: false,
                    write: true,
                }),
                stderr: StreamDescriptor::Null,
            }
        } else {
            StdioSet::default()
        };
        let status = self.runtime.execution_registry().execute(
            &task,
            &ExecutionSpec {
                kind,
                module: module.to_string(),
                args: args.to_vec(),
                env: shell_env_entries(),
                cwd: Some(normalize_cwd(cwd)),
                stdio,
                fds: Vec::new(),
            },
        )?;
        let stdout = task
            .with_fd_mut(1, |file| {
                file.seek(SeekFrom::Start(0))?;
                read_handle_to_string(file)
            })
            .unwrap_or_default();
        Ok(ShellTaskResult {
            task_id: task.id(),
            status: render_task_status(&status),
            stdout,
        })
    }
}

impl ShellHost for RuntimeShellHost {
    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.api.read_file(path)
    }

    fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        self.api.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &[u8]) -> Result<()> {
        self.api.append_file(path, data)
    }

    fn write_existing(&self, path: &str, data: &[u8]) -> Result<()> {
        let mut file = open(self.runtime.namespace().as_ref(), path)?;
        let written = file.write(data)?;
        if written != data.len() {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        file.close()
    }

    fn read_dir(&self, path: &str) -> Result<Vec<String>> {
        self.api.read_dir(path)
    }

    fn mkdir(&self, path: &str) -> Result<()> {
        self.api.mkdir_all(path)
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.api.remove(path)
    }

    fn remove_all(&self, path: &str) -> Result<()> {
        self.api.remove_all(path)
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        self.api.rename(old, new)
    }

    fn copy(&self, old: &str, new: &str) -> Result<()> {
        self.api.copy(old, new)
    }

    fn stat(&self, path: &str) -> Result<ShellStat> {
        self.api.stat(path).map(Into::into)
    }

    fn start_wasi(&self, module: &str, args: &[String], cwd: &str) -> Result<ShellTaskResult> {
        self.runtime
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());
        self.execute_task(ExecutionKind::Wasi, module, args, cwd, None)
    }

    fn start_worker(&self, module: &str, args: &[String], cwd: &str) -> Result<ShellTaskResult> {
        let worker = match self.runtime.handle_runtime_request(
            star9_protocol::runtime::RuntimeRequest::SpawnWorker(WorkerSpawnRequest {
                worker: WorkerHandle {
                    worker_id: format!("shell-worker-{}", next_worker_suffix()),
                    task_id: String::new(),
                },
                parent_task_id: Some(self.runtime.root().id()),
            }),
        )? {
            star9_protocol::runtime::RuntimeResponse::Worker(worker) => worker,
            _ => {
                return Err(Error::Message(
                    "runtime returned non-worker response".into(),
                ))
            }
        };
        self.runtime.handle_runtime_request(
            star9_protocol::runtime::RuntimeRequest::StartWorker(WorkerStartRequest {
                worker: worker.clone(),
                execution: ExecutionSpec {
                    kind: ExecutionKind::JsWasm,
                    module: module.to_string(),
                    args: args.to_vec(),
                    env: shell_env_entries(),
                    cwd: Some(normalize_cwd(cwd)),
                    stdio: StdioSet::default(),
                    fds: Vec::new(),
                },
            }),
        )?;
        let exit = String::from_utf8_lossy(
            &self
                .read_file(&format!("#task/{}/exit", worker.task_id))
                .unwrap_or_default(),
        )
        .trim()
        .to_string();
        Ok(ShellTaskResult {
            task_id: worker.task_id,
            status: exit,
            stdout: String::new(),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run_native(&self, module: &str, args: &[String], cwd: &str) -> Result<ShellTaskResult> {
        if !self.native_enabled {
            return Err(ErrorKind::NotSupported.into());
        }
        self.runtime
            .execution_registry()
            .register_kind(ExecutionKind::Native, NativePtyExecutionHandler::new());
        self.execute_task(ExecutionKind::Native, module, args, cwd, None)
    }
}

pub struct ShellSession<H: ShellHost> {
    host: H,
    cwd: String,
    env: BTreeMap<String, String>,
    last_status: i32,
}

impl<H: ShellHost> ShellSession<H> {
    pub fn new(host: H) -> Self {
        let mut env = BTreeMap::new();
        env.insert("prompt".into(), "star9".into());
        Self {
            host,
            cwd: ".".to_string(),
            env,
            last_status: 0,
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn set_cwd(&mut self, cwd: impl Into<String>) {
        self.cwd = cwd.into();
    }

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn prompt(&self) -> String {
        let prefix = self
            .env
            .get("prompt")
            .map(String::as_str)
            .unwrap_or("star9");
        format!("{prefix}:{}$ ", self.cwd)
    }

    pub fn eval_line(&mut self, line: &str) -> ShellResult {
        match parse_line(line) {
            Ok(commands) => {
                let mut result = ShellResult::default();
                for command in commands {
                    let next = self.eval_command(&command);
                    self.last_status = next.status;
                    result.append(next);
                }
                result
            }
            Err(err) => {
                self.last_status = 2;
                ShellResult {
                    status: 2,
                    stdout: String::new(),
                    stderr: format!("parse: {err}\n"),
                }
            }
        }
    }

    pub fn eval_argv(&mut self, name: impl Into<String>, args: &[String]) -> ShellResult {
        let command = ShellCommand {
            name: name.into(),
            args: args.to_vec(),
        };
        let result = self.eval_command(&command);
        self.last_status = result.status;
        result
    }

    fn eval_command(&mut self, command: &ShellCommand) -> ShellResult {
        let result = match command.name.as_str() {
            "pwd" => self.cmd_pwd(command),
            "cd" => self.cmd_cd(command),
            "ls" => self.cmd_ls(command),
            "cat" => self.cmd_cat(command),
            "write" => self.cmd_write(command, false),
            "append" => self.cmd_write(command, true),
            "mkdir" => self.cmd_mkdir(command),
            "rm" => self.cmd_rm(command),
            "mv" => self.cmd_binary_path(command, "mv", |host, old, new| host.rename(old, new)),
            "cp" => self.cmd_binary_path(command, "cp", |host, old, new| host.copy(old, new)),
            "stat" => self.cmd_stat(command),
            "version" => self.cmd_version(command),
            "binds" => self.cmd_binds(command),
            "tasks" => self.cmd_tasks(command),
            "fds" => self.cmd_fds(command),
            "term" => self.cmd_term(command),
            "vm" => self.cmd_vm(command),
            "net" => self.cmd_net(command),
            "wasi" => self.cmd_wasi(command),
            "worker" => self.cmd_worker(command),
            "native" => self.cmd_native(command),
            "help" => self.cmd_help(command),
            "clear" => ShellResult::success("\x1b[2J\x1b[H"),
            "true" => ShellResult::default(),
            "false" => ShellResult {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
            name => ShellResult::failure(format!("{name}: unknown command\n")),
        };
        result
    }

    fn resolve_path(&self, path: &str) -> String {
        let path = path.trim();
        if path.is_empty() || path == "." {
            return self.cwd.clone();
        }
        if path == "/" {
            return ".".to_string();
        }
        if path.starts_with('#') {
            return clean_path(path);
        }
        if path.starts_with('/') {
            return clean_path(path.trim_start_matches('/'));
        }
        if self.cwd == "." {
            clean_path(path)
        } else {
            clean_path(&format!("{}/{}", self.cwd, path))
        }
    }

    fn read_text(&self, path: &str) -> Result<String> {
        self.host
            .read_file(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    fn cmd_pwd(&self, command: &ShellCommand) -> ShellResult {
        if !command.args.is_empty() {
            return usage("pwd");
        }
        ShellResult::success(format!("{}\n", self.cwd))
    }

    fn cmd_cd(&mut self, command: &ShellCommand) -> ShellResult {
        if command.args.len() > 1 {
            return usage("cd [path]");
        }
        let path = self.resolve_path(command.args.first().map(String::as_str).unwrap_or("."));
        match self.host.stat(&path) {
            Ok(stat) if stat.is_dir => {
                self.cwd = path;
                ShellResult::default()
            }
            Ok(_) => ShellResult::failure(format!("cd: {path}: not a directory\n")),
            Err(err) => ShellResult::failure(format!("cd: {path}: {err}\n")),
        }
    }

    fn cmd_ls(&self, command: &ShellCommand) -> ShellResult {
        let paths = if command.args.is_empty() {
            vec![self.cwd.clone()]
        } else {
            command
                .args
                .iter()
                .map(|path| self.resolve_path(path))
                .collect()
        };
        let mut out = String::new();
        for (index, path) in paths.iter().enumerate() {
            match self.host.read_dir(path) {
                Ok(mut entries) => {
                    entries.sort();
                    if paths.len() > 1 {
                        if index > 0 {
                            out.push('\n');
                        }
                        out.push_str(path);
                        out.push_str(":\n");
                    }
                    for entry in entries {
                        out.push_str(&entry);
                        out.push('\n');
                    }
                }
                Err(err) => return ShellResult::failure(format!("ls: {path}: {err}\n")),
            }
        }
        ShellResult::success(out)
    }

    fn cmd_cat(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("cat <path>...");
        }
        let mut out = String::new();
        for arg in &command.args {
            let path = self.resolve_path(arg);
            match self.read_text(&path) {
                Ok(text) => out.push_str(&text),
                Err(err) => return ShellResult::failure(format!("cat: {path}: {err}\n")),
            }
        }
        ShellResult::success(out)
    }

    fn cmd_write(&self, command: &ShellCommand, append: bool) -> ShellResult {
        let name = if append { "append" } else { "write" };
        if command.args.len() < 2 {
            return usage(&format!("{name} <path> <text...>"));
        }
        let path = self.resolve_path(&command.args[0]);
        let data = command.args[1..].join(" ");
        let result = if append {
            self.host.append_file(&path, data.as_bytes())
        } else {
            self.host.write_file(&path, data.as_bytes())
        };
        match result {
            Ok(()) => ShellResult::default(),
            Err(err) => ShellResult::failure(format!("{name}: {path}: {err}\n")),
        }
    }

    fn cmd_mkdir(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("mkdir <path>...");
        }
        for arg in &command.args {
            let path = self.resolve_path(arg);
            if let Err(err) = self.host.mkdir(&path) {
                return ShellResult::failure(format!("mkdir: {path}: {err}\n"));
            }
        }
        ShellResult::default()
    }

    fn cmd_rm(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("rm [-r] <path>...");
        }
        let recursive = command
            .args
            .first()
            .is_some_and(|arg| arg == "-r" || arg == "-rf");
        let paths = if recursive {
            &command.args[1..]
        } else {
            &command.args[..]
        };
        if paths.is_empty() {
            return usage("rm [-r] <path>...");
        }
        for arg in paths {
            let path = self.resolve_path(arg);
            let result = if recursive {
                self.host.remove_all(&path)
            } else {
                self.host.remove(&path)
            };
            if let Err(err) = result {
                return ShellResult::failure(format!("rm: {path}: {err}\n"));
            }
        }
        ShellResult::default()
    }

    fn cmd_binary_path(
        &self,
        command: &ShellCommand,
        name: &str,
        action: impl Fn(&H, &str, &str) -> Result<()>,
    ) -> ShellResult {
        if command.args.len() != 2 {
            return usage(&format!("{name} <src> <dst>"));
        }
        let src = self.resolve_path(&command.args[0]);
        let dst = self.resolve_path(&command.args[1]);
        match action(&self.host, &src, &dst) {
            Ok(()) => ShellResult::default(),
            Err(err) => ShellResult::failure(format!("{name}: {src} {dst}: {err}\n")),
        }
    }

    fn cmd_stat(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("stat <path>...");
        }
        let mut out = String::new();
        for arg in &command.args {
            let path = self.resolve_path(arg);
            match self.host.stat(&path) {
                Ok(stat) => {
                    out.push_str(&format!(
                        "{path}: size={} mode={:o} dir={} modified_ms={}\n",
                        stat.size, stat.mode, stat.is_dir, stat.modified_ms
                    ));
                }
                Err(err) => return ShellResult::failure(format!("stat: {path}: {err}\n")),
            }
        }
        ShellResult::success(out)
    }

    fn cmd_version(&self, command: &ShellCommand) -> ShellResult {
        if !command.args.is_empty() {
            return usage("version");
        }
        match self.read_text("#star9/version") {
            Ok(text) => ShellResult::success(text),
            Err(err) => ShellResult::failure(format!("version: {err}\n")),
        }
    }

    fn cmd_binds(&self, command: &ShellCommand) -> ShellResult {
        if command.args.len() > 1 {
            return usage("binds [task-id]");
        }
        let task = command.args.first().map(String::as_str).unwrap_or("1");
        let path = format!("#task/{task}/binds");
        match self.read_text(&path) {
            Ok(text) => ShellResult::success(text),
            Err(err) => ShellResult::failure(format!("binds: {task}: {err}\n")),
        }
    }

    fn cmd_tasks(&self, command: &ShellCommand) -> ShellResult {
        if !command.args.is_empty() {
            return usage("tasks");
        }
        match self.host.read_dir("#task") {
            Ok(mut entries) => {
                entries.sort();
                ShellResult::success(entries.join("\n") + "\n")
            }
            Err(err) => ShellResult::failure(format!("tasks: {err}\n")),
        }
    }

    fn cmd_fds(&self, command: &ShellCommand) -> ShellResult {
        if command.args.len() > 1 {
            return usage("fds [task-id]");
        }
        let task = command.args.first().map(String::as_str).unwrap_or("1");
        match self.host.read_dir(&format!("#task/{task}/fd")) {
            Ok(mut entries) => {
                entries.sort();
                ShellResult::success(entries.join("\n") + "\n")
            }
            Err(err) => ShellResult::failure(format!("fds: {task}: {err}\n")),
        }
    }

    fn cmd_term(&self, command: &ShellCommand) -> ShellResult {
        match command.args.as_slice() {
            [] => self.list_device("#term", "term"),
            [cmd] if cmd == "list" => self.list_device("#term", "term"),
            [cmd] if cmd == "new" => match self.read_text("#term/new") {
                Ok(id) => ShellResult::success(id),
                Err(err) => ShellResult::failure(format!("term new: {err}\n")),
            },
            [cmd, id, text @ ..] if cmd == "write" && !text.is_empty() => {
                let path = format!("#term/{id}/program");
                let data = text.join(" ");
                match self.host.write_existing(&path, data.as_bytes()) {
                    Ok(()) => ShellResult::default(),
                    Err(err) => ShellResult::failure(format!("term write: {id}: {err}\n")),
                }
            }
            _ => usage("term [list|new|write <id> <text...>]"),
        }
    }

    fn cmd_vm(&self, command: &ShellCommand) -> ShellResult {
        match command.args.as_slice() {
            [] => self.list_device("#vm", "vm"),
            [cmd] if cmd == "list" => self.list_device("#vm", "vm"),
            [cmd] if cmd == "new" => self.vm_new("vm"),
            [cmd, kind] if cmd == "new" => self.vm_new(kind),
            [cmd, id] if matches!(cmd.as_str(), "start" | "stop" | "reset") => {
                let path = format!("#vm/{id}/ctl");
                match self.host.write_existing(&path, cmd.as_bytes()) {
                    Ok(()) => ShellResult::default(),
                    Err(err) => ShellResult::failure(format!("vm {cmd}: {id}: {err}\n")),
                }
            }
            [cmd, id] if matches!(cmd.as_str(), "state" | "console" | "config") => {
                match self.read_text(&format!("#vm/{id}/{cmd}")) {
                    Ok(text) => ShellResult::success(text),
                    Err(err) => ShellResult::failure(format!("vm {cmd}: {id}: {err}\n")),
                }
            }
            _ => usage("vm [list|new [kind]|start <id>|stop <id>|reset <id>|state <id>|console <id>|config <id>]"),
        }
    }

    fn vm_new(&self, kind: &str) -> ShellResult {
        match self.read_text(&format!("#vm/new/{kind}")) {
            Ok(id) => ShellResult::success(id),
            Err(err) => ShellResult::failure(format!("vm new: {kind}: {err}\n")),
        }
    }

    fn cmd_net(&self, command: &ShellCommand) -> ShellResult {
        match command.args.as_slice() {
            [] => self.list_device("#net", "net"),
            [cmd] if cmd == "list" => self.list_device("#net", "net"),
            [cmd] if cmd == "new" => match self.read_text("#net/new") {
                Ok(id) => ShellResult::success(id),
                Err(err) => ShellResult::failure(format!("net new: {err}\n")),
            },
            [cmd, id, rest @ ..] if matches!(cmd.as_str(), "ctl" | "dial" | "announce") => {
                let data = if cmd == "ctl" {
                    rest.join(" ")
                } else {
                    format!("{cmd} {}", rest.join(" "))
                };
                match self
                    .host
                    .write_existing(&format!("#net/{id}/ctl"), data.as_bytes())
                {
                    Ok(()) => ShellResult::default(),
                    Err(err) => ShellResult::failure(format!("net {cmd}: {id}: {err}\n")),
                }
            }
            [cmd, id] if matches!(cmd.as_str(), "status" | "local" | "remote") => {
                match self.read_text(&format!("#net/{id}/{cmd}")) {
                    Ok(text) => ShellResult::success(text),
                    Err(err) => ShellResult::failure(format!("net {cmd}: {id}: {err}\n")),
                }
            }
            _ => usage("net [list|new|dial <id> <addr>|announce <id> <addr>|ctl <id> <cmd...>|status <id>|local <id>|remote <id>]"),
        }
    }

    fn list_device(&self, path: &str, name: &str) -> ShellResult {
        match self.host.read_dir(path) {
            Ok(mut entries) => {
                entries.sort();
                ShellResult::success(entries.join("\n") + "\n")
            }
            Err(err) => ShellResult::failure(format!("{name}: {err}\n")),
        }
    }

    fn cmd_wasi(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("wasi <module> [args...]");
        }
        let module = self.resolve_path(&command.args[0]);
        match self.host.start_wasi(&module, &command.args[1..], &self.cwd) {
            Ok(task) => ShellResult::success(format_task("wasi", task)),
            Err(err) => ShellResult::failure(format!("wasi: {module}: {err}\n")),
        }
    }

    fn cmd_worker(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("worker <module> [args...]");
        }
        match self
            .host
            .start_worker(&command.args[0], &command.args[1..], &self.cwd)
        {
            Ok(task) => ShellResult::success(format_task("worker", task)),
            Err(err) => ShellResult::failure(format!("worker: {}: {err}\n", command.args[0])),
        }
    }

    fn cmd_native(&self, command: &ShellCommand) -> ShellResult {
        if command.args.is_empty() {
            return usage("native <module> [args...]");
        }
        match self
            .host
            .run_native(&command.args[0], &command.args[1..], &self.cwd)
        {
            Ok(task) => ShellResult::success(format_task("native", task)),
            Err(err) => ShellResult::failure(format!("native: {}: {err}\n", command.args[0])),
        }
    }

    fn cmd_help(&self, command: &ShellCommand) -> ShellResult {
        if command.args.len() > 1 {
            return usage("help [command]");
        }
        let text = match command.args.first().map(String::as_str) {
            Some("vm") => "vm [list|new [kind]|start <id>|stop <id>|reset <id>|state <id>|console <id>|config <id>]\n",
            Some("net") => "net [list|new|dial <id> <addr>|announce <id> <addr>|ctl <id> <cmd...>|status <id>|local <id>|remote <id>]\n",
            Some("term") => "term [list|new|write <id> <text...>]\n",
            Some("write") => "write <path> <text...>\n",
            Some("append") => "append <path> <text...>\n",
            Some("wasi") => "wasi <module> [args...]\n",
            Some("worker") => "worker <module> [args...]\n",
            Some("native") => "native <module> [args...]  # opt-in native CLI mode only\n",
            Some(name) => return ShellResult::failure(format!("help: no detailed help for {name}\n")),
            None => HELP,
        };
        ShellResult::success(text)
    }
}

pub fn parse_line(line: &str) -> Result<Vec<ShellCommand>> {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    let mut at_word_start = true;

    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            current.push(ch);
            escaped = false;
            at_word_start = false;
            index += 1;
            continue;
        }
        match quote {
            Quote::Single => match ch {
                '\'' => quote = Quote::None,
                _ => current.push(ch),
            },
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                _ => current.push(ch),
            },
            Quote::None => match ch {
                '\\' => escaped = true,
                '\'' => {
                    quote = Quote::Single;
                    at_word_start = false;
                }
                '"' => {
                    quote = Quote::Double;
                    at_word_start = false;
                }
                ';' => {
                    push_word(&mut words, &mut current);
                    if let Some(command) = ShellCommand::new(std::mem::take(&mut words)) {
                        commands.push(command);
                    }
                    at_word_start = true;
                }
                c if c.is_whitespace() => {
                    push_word(&mut words, &mut current);
                    at_word_start = true;
                }
                '#' if at_word_start && is_comment_marker(&chars, index) => break,
                _ => {
                    current.push(ch);
                    at_word_start = false;
                }
            },
        }
        index += 1;
    }

    if escaped {
        current.push('\\');
    }
    match quote {
        Quote::None => {}
        Quote::Single => return Err(Error::Message("unterminated single quote".into())),
        Quote::Double => return Err(Error::Message("unterminated double quote".into())),
    }
    push_word(&mut words, &mut current);
    if let Some(command) = ShellCommand::new(words) {
        commands.push(command);
    }
    Ok(commands)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}

fn is_comment_marker(chars: &[char], index: usize) -> bool {
    chars
        .get(index + 1)
        .map_or(true, |next| next.is_whitespace() || *next == ';')
}

fn normalize_cwd(cwd: &str) -> String {
    if cwd.trim().is_empty() {
        ".".into()
    } else {
        cwd.to_string()
    }
}

fn shell_env_entries() -> Vec<EnvironmentEntry> {
    vec![EnvironmentEntry {
        name: "STAR9_SHELL".into(),
        value: "1".into(),
    }]
}

fn render_task_status(status: &star9_protocol::runtime::ExitStatus) -> String {
    match status {
        star9_protocol::runtime::ExitStatus::ExitCode(code) => code.to_string(),
        star9_protocol::runtime::ExitStatus::Signal(signal) => format!("signal:{signal}"),
        star9_protocol::runtime::ExitStatus::Trap(reason) => format!("trap:{reason}"),
    }
}

fn read_handle_to_string(file: &mut dyn star9_fs::FileHandle) -> Result<String> {
    let mut out = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn next_worker_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

fn format_task(kind: &str, task: ShellTaskResult) -> String {
    let mut out = format!("{kind} task={} status={}\n", task.task_id, task.status);
    if !task.stdout.is_empty() {
        out.push_str(&task.stdout);
    }
    out
}

fn usage(text: &str) -> ShellResult {
    ShellResult {
        status: 2,
        stdout: String::new(),
        stderr: format!("usage: {text}\n"),
    }
}

const HELP: &str = "\
Star 9 shell commands:
  pwd, cd, ls, cat, write, append, mkdir, rm, mv, cp, stat
  version, binds, tasks, fds
  term, vm, net
  wasi, worker, native
  help [command]
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_quotes_escapes_and_sequences() {
        let parsed = parse_line("write a 'hello world'; append a \"!\"\\;").unwrap();
        assert_eq!(
            parsed,
            vec![
                ShellCommand {
                    name: "write".into(),
                    args: vec!["a".into(), "hello world".into()]
                },
                ShellCommand {
                    name: "append".into(),
                    args: vec!["a".into(), "!;".into()]
                }
            ]
        );
    }

    #[test]
    fn parser_keeps_star9_device_paths_from_becoming_comments() {
        let parsed = parse_line("ls #task").unwrap();
        assert_eq!(parsed[0].args, vec!["#task"]);
        assert!(parse_line("# comment").unwrap().is_empty());
    }

    #[test]
    fn shell_runs_file_commands_through_runtime_host() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut shell = ShellSession::new(host);
        let result = shell.eval_line("mkdir demo; write demo/hello 'hello world'; cat demo/hello");
        assert_eq!(result.status, 0, "{}", result.stderr);
        assert_eq!(result.stdout, "hello world");
    }

    #[test]
    fn shell_tracks_cwd_and_devices() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut shell = ShellSession::new(host);
        assert_eq!(shell.eval_line("mkdir work; cd work; pwd").stdout, "work\n");
        let tasks = shell.eval_line("ls #task");
        assert_eq!(tasks.status, 0, "{}", tasks.stderr);
        assert!(tasks.stdout.contains("1/"));
    }

    #[test]
    fn shell_controls_deterministic_vm_via_files() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut shell = ShellSession::new(host);
        let created = shell.eval_line("vm new v86");
        assert_eq!(created.status, 0, "{}", created.stderr);
        let id = created.stdout.trim();
        assert!(!id.is_empty());
        assert_eq!(shell.eval_line(&format!("vm start {id}")).status, 0);
        assert_eq!(
            shell.eval_line(&format!("vm state {id}")).stdout,
            "running\n"
        );
    }
}
