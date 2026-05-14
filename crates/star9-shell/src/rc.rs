use std::collections::BTreeMap;

#[cfg(not(target_arch = "wasm32"))]
use std::io::SeekFrom;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{mpsc, Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

use star9_core::{clean_path, Error, ErrorKind, FileMode, FsContext, Result};
#[cfg(not(target_arch = "wasm32"))]
use star9_fs::write_file;
use star9_fs::{fs_ref, FileSystem, MemFs, Node, PipeFs};
#[cfg(not(target_arch = "wasm32"))]
use star9_protocol::runtime::{
    EnvironmentEntry, ExecutionKind, ExecutionSpec, FdDescriptor, FdKind, StdioSet,
    StreamDescriptor,
};
#[cfg(not(target_arch = "wasm32"))]
use star9_rc::RcExecutableStageSpec;
use star9_rc::{
    RcCommandInvocation, RcCommandResult, RcError, RcExecutableGraphSpec, RcFdBindingSpec, RcHost,
    RcOutput, RcProcessGraphKind, RcProcessGraphRecord, RcProcessGraphSpec, RcProcessJobResult,
    RcProcessStageOutcome, RcProcessStageRecord, RcSession, RcStartedProcessJob, RcStat, RcStatus,
};
#[cfg(not(target_arch = "wasm32"))]
use star9_runtime::WasmiWasiHandler;
use star9_task::Task;
use star9_vfs::BindMode;

use crate::{RuntimeShellHost, ShellHost, ShellSession};

#[cfg(not(target_arch = "wasm32"))]
use star9_runtime::NativePtyExecutionHandler;

pub type Star9RcSession = RcSession<Star9RcHost>;

#[derive(Clone)]
pub struct Star9RcHost {
    host: RuntimeShellHost,
    cwd: String,
    next_graph_id: u32,
    #[cfg(not(target_arch = "wasm32"))]
    running_jobs: Arc<Mutex<BTreeMap<u32, mpsc::Receiver<RcProcessJobResult>>>>,
}

impl Star9RcHost {
    pub fn new(host: RuntimeShellHost) -> Self {
        Self {
            host,
            cwd: ".".into(),
            next_graph_id: 1,
            #[cfg(not(target_arch = "wasm32"))]
            running_jobs: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn runtime_host(&self) -> RuntimeShellHost {
        self.host.clone()
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

    fn graph_root(&mut self) -> Result<String> {
        let graph_id = self.next_graph_id;
        self.next_graph_id += 1;
        let root = format!(".rc/graphs/rcgraph{graph_id}");
        self.host.runtime().namespace().bind(
            fs_ref(MemFs::new()),
            ".",
            &root,
            BindMode::Replace,
        )?;
        Ok(root)
    }

    fn open_task_file(task: &Task, path: &str) -> Result<star9_fs::BoxFile> {
        task.namespace().open(&FsContext::new(), path)
    }

    fn install_standard_fds(task: &Task) -> Result<()> {
        for (fd, name) in [(0, "stdin"), (1, "stdout"), (2, "stderr")] {
            let node = Node::file(name, Vec::new(), FileMode::from_perm(0o666));
            task.set_fd(fd, node.open(&FsContext::new(), ".")?, name);
        }
        Ok(())
    }

    fn install_task_binding(
        task: &Task,
        binding: &RcFdBindingSpec,
        graph_root: &str,
    ) -> Result<RcFdBindingSpec> {
        let path = resolve_graph_binding_path(graph_root, &binding.path);
        let file = Self::open_task_file(task, &path)?;
        task.set_fd(binding.fd, file, path.clone());
        Ok(RcFdBindingSpec {
            fd: binding.fd,
            path,
            readable: binding.readable,
            writable: binding.writable,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn next_graph_root(&mut self) -> star9_rc::RcResult<String> {
        self.graph_root().map_err(to_rc_error)
    }
}

impl RcHost for Star9RcHost {
    fn current_dir(&self) -> String {
        self.cwd.clone()
    }

    fn set_current_dir(&mut self, path: &str) -> star9_rc::RcResult<()> {
        let path = self.resolve_path(path);
        match self.host.stat(&path) {
            Ok(stat) if stat.is_dir => {
                self.cwd = path;
                Ok(())
            }
            Ok(_) => Err(rc_error("not a directory")),
            Err(err) => Err(to_rc_error(err)),
        }
    }

    fn read_file(&mut self, path: &str) -> star9_rc::RcResult<Vec<u8>> {
        self.host
            .read_file(&self.resolve_path(path))
            .map_err(to_rc_error)
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> star9_rc::RcResult<()> {
        self.host
            .write_file(&self.resolve_path(path), data)
            .map_err(to_rc_error)
    }

    fn append_file(&mut self, path: &str, data: &[u8]) -> star9_rc::RcResult<()> {
        self.host
            .append_file(&self.resolve_path(path), data)
            .map_err(to_rc_error)
    }

    fn read_dir(&mut self, path: &str) -> star9_rc::RcResult<Vec<String>> {
        self.host
            .read_dir(&self.resolve_path(path))
            .map_err(to_rc_error)
    }

    fn stat(&mut self, path: &str) -> star9_rc::RcResult<RcStat> {
        self.host
            .stat(&self.resolve_path(path))
            .map(|stat| RcStat {
                is_dir: stat.is_dir,
            })
            .map_err(to_rc_error)
    }

    fn run_command(
        &mut self,
        invocation: RcCommandInvocation,
    ) -> star9_rc::RcResult<RcCommandResult> {
        let mut shell = ShellSession::new(self.host.clone());
        shell.set_cwd(self.cwd.clone());
        let result = if invocation.name.ends_with(".wasm") || invocation.name.ends_with(".wat") {
            let mut args = vec![invocation.name.clone()];
            args.extend(invocation.args.clone());
            shell.eval_argv("wasi", &args)
        } else if invocation.name.ends_with(".js") || invocation.name.ends_with(".mjs") {
            let mut args = vec![invocation.name.clone()];
            args.extend(invocation.args.clone());
            shell.eval_argv("worker", &args)
        } else {
            shell.eval_argv(invocation.name, &invocation.args)
        };
        Ok(RcCommandResult {
            status: RcStatus::from_code(result.status),
            stdout: result.stdout,
            stderr: result.stderr,
        })
    }

    fn load_environment(&mut self) -> star9_rc::RcResult<Option<BTreeMap<String, Vec<u8>>>> {
        Ok(Some(self.host.runtime().env_registry().snapshot()))
    }

    fn store_environment(&mut self, env: &BTreeMap<String, Vec<u8>>) -> star9_rc::RcResult<()> {
        self.host.runtime().env_registry().replace_all(env.clone());
        Ok(())
    }

    fn prepare_process_graph(
        &mut self,
        spec: &RcProcessGraphSpec,
    ) -> star9_rc::RcResult<Option<RcProcessGraphRecord>> {
        let graph_root = self.graph_root().map_err(to_rc_error)?;
        let graph_id = graph_root
            .rsplit('/')
            .next()
            .unwrap_or(&graph_root)
            .to_string();
        if matches!(
            spec.kind,
            RcProcessGraphKind::Pipeline
                | RcProcessGraphKind::ProcessSubstitutionRead
                | RcProcessGraphKind::ProcessSubstitutionWrite
        ) {
            self.host
                .runtime()
                .namespace()
                .bind(
                    fs_ref(PipeFs::new(false)),
                    ".",
                    &format!("{graph_root}/pipe0"),
                    BindMode::Replace,
                )
                .map_err(to_rc_error)?;
        }

        let mut stages = Vec::new();
        for stage in &spec.stages {
            let task = self
                .host
                .runtime()
                .task_fs()
                .alloc("auto", Some(self.host.runtime().root()))
                .map_err(to_rc_error)?;
            task.set_cmd(stage.command.clone());
            task.set_dir(stage.cwd.clone());
            task.set_env(stage.env.iter().map(|(name, values)| {
                if values.is_empty() {
                    format!("{name}=()")
                } else {
                    format!("{name}={}", values.join("\0"))
                }
            }));
            task.set_exit("planned");
            Self::install_standard_fds(&task).map_err(to_rc_error)?;
            let mut fd_bindings = Vec::new();
            for binding in &stage.fd_bindings {
                fd_bindings.push(
                    Self::install_task_binding(&task, binding, &graph_root).map_err(to_rc_error)?,
                );
            }
            stages.push(RcProcessStageRecord {
                command: stage.command.clone(),
                task_id: Some(task.id()),
                fd_bindings,
            });
        }

        Ok(Some(RcProcessGraphRecord {
            graph_id,
            kind: spec.kind.clone(),
            job_id: spec.job_id,
            stages,
        }))
    }

    fn finish_process_graph(
        &mut self,
        record: &RcProcessGraphRecord,
        outcomes: &[RcProcessStageOutcome],
    ) -> star9_rc::RcResult<()> {
        for (stage, outcome) in record.stages.iter().zip(outcomes.iter()) {
            let Some(task_id) = &stage.task_id else {
                continue;
            };
            if let Ok(task) = self.host.runtime().task_fs().lookup(task_id) {
                task.set_exit(outcome.status.to_string());
            }
        }
        Ok(())
    }

    fn wait_process_job(
        &mut self,
        _job_id: Option<u32>,
    ) -> star9_rc::RcResult<Option<Vec<RcProcessJobResult>>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut jobs = self.running_jobs.lock().unwrap();
            let receivers = if let Some(job_id) = _job_id {
                let Some(receiver) = jobs.remove(&job_id) else {
                    return Ok(None);
                };
                vec![(job_id, receiver)]
            } else if jobs.is_empty() {
                return Ok(None);
            } else {
                std::mem::take(&mut *jobs).into_iter().collect()
            };
            drop(jobs);

            let mut results = Vec::new();
            for (job_id, receiver) in receivers {
                match receiver.recv() {
                    Ok(result) => results.push(result),
                    Err(_) => results.push(RcProcessJobResult {
                        id: job_id,
                        status: RcStatus::from_status("failed"),
                        stdout: String::new(),
                        stderr: format!("wait: {job_id}: job provider closed\n"),
                    }),
                }
            }
            Ok(Some(results))
        }

        #[cfg(target_arch = "wasm32")]
        Ok(None)
    }

    fn send_note_to_processes(&mut self, note: &str) -> star9_rc::RcResult<()> {
        match self.host.write_existing("#signal/data", note.as_bytes()) {
            Ok(()) => Ok(()),
            Err(_) => Ok(()),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn execute_process_graph(
        &mut self,
        spec: RcExecutableGraphSpec,
    ) -> star9_rc::RcResult<Option<RcOutput>> {
        if spec.kind != RcProcessGraphKind::Pipeline {
            return Ok(None);
        }
        if !can_execute_external_graph(&spec, self.host.native_enabled()) {
            return Ok(None);
        }
        let graph_root = self.next_graph_root()?;
        execute_external_graph(self.host.clone(), graph_root, spec)
    }

    #[cfg(target_arch = "wasm32")]
    fn execute_process_graph(
        &mut self,
        _spec: RcExecutableGraphSpec,
    ) -> star9_rc::RcResult<Option<RcOutput>> {
        Ok(None)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_process_graph_job(
        &mut self,
        spec: RcExecutableGraphSpec,
    ) -> star9_rc::RcResult<Option<RcStartedProcessJob>> {
        if spec.kind != RcProcessGraphKind::Background {
            return Ok(None);
        }
        let Some(job_id) = spec.job_id else {
            return Ok(None);
        };
        if !can_execute_external_graph(&spec, self.host.native_enabled()) {
            return Ok(None);
        }
        let graph_root = self.next_graph_root()?;
        let host = self.host.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = execute_external_graph(host, graph_root, spec);
            let job = match result {
                Ok(Some(out)) => RcProcessJobResult {
                    id: job_id,
                    status: out.status,
                    stdout: out.stdout,
                    stderr: out.stderr,
                },
                Ok(None) => RcProcessJobResult {
                    id: job_id,
                    status: RcStatus::from_status("provider-missing"),
                    stdout: String::new(),
                    stderr: format!("job {job_id}: no external execution provider\n"),
                },
                Err(err) => RcProcessJobResult {
                    id: job_id,
                    status: RcStatus::from_status("failed"),
                    stdout: String::new(),
                    stderr: format!("job {job_id}: {err}\n"),
                },
            };
            let _ = tx.send(job);
        });
        self.running_jobs.lock().unwrap().insert(job_id, rx);
        Ok(Some(RcStartedProcessJob { id: job_id }))
    }

    #[cfg(target_arch = "wasm32")]
    fn start_process_graph_job(
        &mut self,
        _spec: RcExecutableGraphSpec,
    ) -> star9_rc::RcResult<Option<RcStartedProcessJob>> {
        Ok(None)
    }

    fn rfork(&mut self, flags: &str) -> star9_rc::RcResult<()> {
        if flags.chars().all(|flag| flag == 'e') {
            Ok(())
        } else {
            Err(RcError::new(format!(
                "unsupported Star 9 rfork flags {flags}"
            )))
        }
    }
}

pub struct RcShell {
    session: Star9RcSession,
}

impl RcShell {
    pub fn new(host: RuntimeShellHost) -> Self {
        Self {
            session: RcSession::new(Star9RcHost::new(host)),
        }
    }

    pub fn eval_line(&mut self, source: &str) -> RcOutput {
        self.session.eval_source(source)
    }

    pub fn set_argv0(&mut self, argv0: impl Into<String>) {
        self.session.set_argv0(argv0);
    }

    pub fn set_args(&mut self, args: Vec<String>) {
        self.session.set_args(args);
    }

    pub fn prompt(&self) -> String {
        self.session.prompt()
    }

    pub fn cwd(&self) -> String {
        self.session.host().current_dir()
    }

    pub fn last_status(&self) -> String {
        self.session.last_status().to_string()
    }

    pub fn session(&self) -> &Star9RcSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Star9RcSession {
        &mut self.session
    }
}

fn to_rc_error(err: Error) -> RcError {
    RcError::new(err.to_string())
}

fn rc_error(message: &str) -> RcError {
    RcError::new(message)
}

fn resolve_graph_binding_path(graph_root: &str, binding_path: &str) -> String {
    if let Some(rest) = binding_path.strip_prefix("pipe:") {
        let (pipe, path) = rest.split_once('/').unwrap_or((rest, "."));
        format!("{graph_root}/pipe{pipe}/{path}")
    } else {
        format!("{graph_root}/{}", binding_path.trim_start_matches('/'))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn can_execute_external_graph(spec: &RcExecutableGraphSpec, native_enabled: bool) -> bool {
    !spec.stages.is_empty()
        && spec.stages.iter().all(|stage| {
            stage_execution_kind(stage, native_enabled)
                .map(|kind| kind != ExecutionKind::JsWasm)
                .unwrap_or(false)
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn execute_external_graph(
    host: RuntimeShellHost,
    graph_root: String,
    spec: RcExecutableGraphSpec,
) -> star9_rc::RcResult<Option<RcOutput>> {
    if !can_execute_external_graph(&spec, host.native_enabled()) {
        return Ok(None);
    }
    for index in 0..spec.stages.len().saturating_sub(1) {
        host.runtime()
            .namespace()
            .bind(
                fs_ref(PipeFs::new(true)),
                ".",
                &format!("{graph_root}/pipe{index}"),
                BindMode::Replace,
            )
            .map_err(to_rc_error)?;
    }

    if spec.stages.iter().any(|stage| {
        stage_execution_kind(stage, host.native_enabled()) == Some(ExecutionKind::Wasi)
    }) {
        host.runtime()
            .execution_registry()
            .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());
    }
    if spec.stages.iter().any(|stage| {
        stage_execution_kind(stage, host.native_enabled()) == Some(ExecutionKind::Native)
    }) {
        host.runtime()
            .execution_registry()
            .register_kind(ExecutionKind::Native, NativePtyExecutionHandler::new());
    }

    let mut stages = Vec::new();
    for (index, stage) in spec.stages.iter().enumerate() {
        let task = host
            .runtime()
            .task_fs()
            .alloc("auto", Some(host.runtime().root()))
            .map_err(to_rc_error)?;
        let exec = build_execution_spec(&host, &graph_root, index, stage).map_err(to_rc_error)?;
        stages.push((task, exec, stage.fd_bindings.clone()));
    }

    let mut handles = Vec::new();
    for (task, exec, fd_bindings) in &stages {
        let registry = host.runtime().execution_registry();
        let task = task.clone();
        let exec = exec.clone();
        let fd_bindings = fd_bindings.clone();
        handles.push(thread::spawn(move || {
            let result = registry.execute(&task, &exec);
            for binding in fd_bindings.iter().filter(|binding| binding.writable) {
                let _ = task.close_fd(binding.fd);
            }
            result
        }));
    }

    let mut statuses = Vec::new();
    for handle in handles {
        let status = handle
            .join()
            .map_err(|_| RcError::new("execution graph stage panicked"))?
            .map_err(to_rc_error)?;
        statuses.push(exit_status_to_rc(&status));
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    for ((task, _, bindings), _) in stages.iter().zip(statuses.iter()) {
        if !bindings
            .iter()
            .any(|binding| binding.fd == 1 && binding.writable)
        {
            stdout.push_str(&read_task_fd_string(task, 1).map_err(to_rc_error)?);
        }
        if !bindings
            .iter()
            .any(|binding| binding.fd == 2 && binding.writable)
        {
            stderr.push_str(&read_task_fd_string(task, 2).map_err(to_rc_error)?);
        }
    }

    let status = statuses
        .iter()
        .cloned()
        .reduce(|left, right| RcStatus::pipeline(&left, &right))
        .unwrap_or_else(RcStatus::success);
    Ok(Some(RcOutput {
        status,
        stdout,
        stderr,
        exited: false,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn build_execution_spec(
    host: &RuntimeShellHost,
    graph_root: &str,
    index: usize,
    stage: &RcExecutableStageSpec,
) -> Result<ExecutionSpec> {
    let (kind, module, args) = stage_execution(stage, host.native_enabled())
        .ok_or_else(|| Error::path("exec", stage.argv.join(" "), ErrorKind::NotSupported))?;
    let stdin_path = if !stage.stdin.is_empty() {
        let path = format!("{graph_root}/stage{index}-stdin");
        write_file(
            host.runtime().namespace().as_ref(),
            &path,
            stage.stdin.as_bytes(),
            FileMode::from_perm(0o644),
        )?;
        Some(path)
    } else {
        None
    };

    let stdin = stream_for_fd(stage, graph_root, 0)
        .or_else(|| stdin_path.map(|path| fd_stream(0, FdKind::File, path, true, false)))
        .unwrap_or(StreamDescriptor::Null);
    let stdout = stream_for_fd(stage, graph_root, 1).unwrap_or(StreamDescriptor::Inherit);
    let stderr = stream_for_fd(stage, graph_root, 2).unwrap_or(StreamDescriptor::Inherit);
    let fds = stage
        .fd_bindings
        .iter()
        .filter(|binding| !matches!(binding.fd, 0..=2))
        .map(|binding| fd_descriptor_from_binding(binding, graph_root))
        .collect();

    Ok(ExecutionSpec {
        kind,
        module,
        args,
        env: rc_env_entries(&stage.env),
        cwd: Some(normalize_graph_cwd(&stage.cwd)),
        stdio: StdioSet {
            stdin,
            stdout,
            stderr,
        },
        fds,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn stream_for_fd(
    stage: &RcExecutableStageSpec,
    graph_root: &str,
    fd: u32,
) -> Option<StreamDescriptor> {
    stage
        .fd_bindings
        .iter()
        .find(|binding| binding.fd == fd)
        .map(|binding| {
            let descriptor = fd_descriptor_from_binding(binding, graph_root);
            StreamDescriptor::Fd(descriptor)
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn fd_descriptor_from_binding(binding: &RcFdBindingSpec, graph_root: &str) -> FdDescriptor {
    FdDescriptor {
        fd: binding.fd,
        kind: if binding.path.starts_with("pipe:") {
            FdKind::Pipe
        } else {
            FdKind::File
        },
        path: Some(resolve_executable_binding_path(graph_root, &binding.path)),
        read: binding.readable,
        write: binding.writable,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_executable_binding_path(graph_root: &str, binding_path: &str) -> String {
    if let Some(rest) = binding_path.strip_prefix("pipe:") {
        let (pipe, path) = rest.split_once('/').unwrap_or((rest, "."));
        format!("{graph_root}/pipe{pipe}/{path}")
    } else {
        binding_path.to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn fd_stream(fd: u32, kind: FdKind, path: String, read: bool, write: bool) -> StreamDescriptor {
    StreamDescriptor::Fd(FdDescriptor {
        fd,
        kind,
        path: Some(path),
        read,
        write,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn stage_execution_kind(
    stage: &RcExecutableStageSpec,
    native_enabled: bool,
) -> Option<ExecutionKind> {
    stage_execution(stage, native_enabled).map(|(kind, _, _)| kind)
}

#[cfg(not(target_arch = "wasm32"))]
fn stage_execution(
    stage: &RcExecutableStageSpec,
    native_enabled: bool,
) -> Option<(ExecutionKind, String, Vec<String>)> {
    let (command, rest) = stage.argv.split_first()?;
    match command.as_str() {
        "wasi" => {
            let (module, args) = rest.split_first()?;
            Some((ExecutionKind::Wasi, module.clone(), args.to_vec()))
        }
        "worker" => {
            let (module, args) = rest.split_first()?;
            Some((ExecutionKind::JsWasm, module.clone(), args.to_vec()))
        }
        "native" if native_enabled => {
            let (module, args) = rest.split_first()?;
            Some((ExecutionKind::Native, module.clone(), args.to_vec()))
        }
        command if command.ends_with(".wasm") || command.ends_with(".wat") => {
            Some((ExecutionKind::Wasi, command.to_string(), rest.to_vec()))
        }
        command if command.ends_with(".js") || command.ends_with(".mjs") => {
            Some((ExecutionKind::JsWasm, command.to_string(), rest.to_vec()))
        }
        command if native_enabled => {
            Some((ExecutionKind::Native, command.to_string(), rest.to_vec()))
        }
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn rc_env_entries(env: &BTreeMap<String, Vec<String>>) -> Vec<EnvironmentEntry> {
    env.iter()
        .map(|(name, values)| EnvironmentEntry {
            name: name.clone(),
            value: values.join("\0"),
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn normalize_graph_cwd(cwd: &str) -> String {
    if cwd.trim().is_empty() {
        ".".into()
    } else {
        cwd.to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn exit_status_to_rc(status: &star9_protocol::runtime::ExitStatus) -> RcStatus {
    match status {
        star9_protocol::runtime::ExitStatus::ExitCode(code) => RcStatus::from_code(*code),
        star9_protocol::runtime::ExitStatus::Signal(signal) => {
            RcStatus::from_status(format!("signal:{signal}"))
        }
        star9_protocol::runtime::ExitStatus::Trap(reason) => {
            RcStatus::from_status(format!("trap:{reason}"))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_task_fd_string(task: &Task, fd: u32) -> Result<String> {
    task.with_fd_mut(fd, |file| {
        let _ = file.seek(SeekFrom::Start(0));
        let mut out = Vec::new();
        let mut buf = [0_u8; 8192];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(err) if err.kind() == ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(err),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    })
}

pub fn rc_to_star9_result(output: RcOutput) -> Result<crate::ShellResult> {
    let status = if output.status.is_success() { 0 } else { 1 };
    Ok(crate::ShellResult {
        status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub fn not_supported(message: &str) -> Error {
    Error::path("rc", message, ErrorKind::NotSupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_shell_runs_against_star9_namespace() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("mkdir demo; write demo/hello hello; cat demo/hello");
        assert!(
            out.status.is_success(),
            "status={} stdout={:?} stderr={:?}",
            out.status,
            out.stdout,
            out.stderr
        );
        assert_eq!(out.stdout, "hello");
    }

    #[test]
    fn rc_shell_reaches_star9_devices() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("ls '#task'");
        assert!(
            out.status.is_success(),
            "status={} stdout={:?} stderr={:?}",
            out.status,
            out.stdout,
            out.stderr
        );
        assert!(out.stdout.contains("1/"), "{}", out.stdout);
    }

    #[test]
    fn rc_shell_can_use_star9_service_mount_commands() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line(
            "mkdir exported; write exported/hello ok; srv root rootsrv; mount rootsrv n/root; cat n/root/exported/hello",
        );
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "ok");
    }

    #[test]
    fn rc_shell_syncs_environment_with_env_device() {
        let host = RuntimeShellHost::fresh().unwrap();
        let runtime = host.runtime();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("color=(red blue); cat '#env/color'");
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "red\0blue");

        runtime
            .env_registry()
            .replace_all(BTreeMap::from([("shape".to_string(), b"circle".to_vec())]));
        let out = rc.eval_line("echo $shape");
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "circle\n");
    }

    #[test]
    fn rc_shell_pipeline_creates_task_fd_graph() {
        let host = RuntimeShellHost::fresh().unwrap();
        let runtime = host.runtime();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("echo hello | cat");
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "hello\n");

        let tasks = runtime.task_fs().tasks();
        let echo = tasks
            .iter()
            .find(|task| task.cmd() == "echo hello")
            .expect("pipeline left task");
        let cat = tasks
            .iter()
            .find(|task| task.cmd() == "cat")
            .expect("pipeline right task");
        assert_eq!(echo.exit(), "0");
        assert_eq!(cat.exit(), "0");
        let fds = tasks
            .iter()
            .flat_map(|task| task.fd_entries())
            .map(|(_, path)| path)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data"), "{fds}");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data1"), "{fds}");
    }

    #[test]
    fn rc_shell_background_wait_records_task_job() {
        let host = RuntimeShellHost::fresh().unwrap();
        let runtime = host.runtime();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("echo bg & wait 1");
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "[1]\nbg\n[1] 0\n");

        let task = runtime
            .task_fs()
            .tasks()
            .into_iter()
            .find(|task| task.cmd() == "echo bg")
            .expect("background task");
        assert_eq!(task.exit(), "0");
    }

    #[test]
    fn rc_shell_process_substitution_creates_task_fd_graph() {
        let host = RuntimeShellHost::fresh().unwrap();
        let runtime = host.runtime();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("cat <{echo proc}");
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "proc\n");

        let task = runtime
            .task_fs()
            .tasks()
            .into_iter()
            .find(|task| task.cmd().contains("echo proc"))
            .expect("process substitution task");
        assert_eq!(task.exit(), "0");
        let fds = task
            .fd_entries()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data"), "{fds}");
    }

    #[test]
    fn rc_shell_runs_external_wasi_pipeline_through_provider_fds() {
        let host = RuntimeShellHost::fresh().unwrap();
        let runtime = host.runtime();
        host.write_file("producer.wasm", &wasi_producer()).unwrap();
        host.write_file("cat.wasm", &wasi_cat()).unwrap();
        let mut rc = RcShell::new(host);

        let out = rc.eval_line("wasi producer.wasm | wasi cat.wasm");
        assert!(
            out.status.is_success(),
            "status={} stdout={:?} stderr={:?}",
            out.status,
            out.stdout,
            out.stderr
        );
        assert_eq!(out.status.to_string(), "0|0", "{out:?}");
        assert_eq!(out.stdout, "pipe-ok\n", "{out:?}");

        let tasks = runtime.task_fs().tasks();
        let producer = tasks
            .iter()
            .find(|task| task.cmd() == "producer.wasm")
            .expect("producer task");
        let consumer = tasks
            .iter()
            .find(|task| task.cmd() == "cat.wasm")
            .expect("consumer task");
        assert_eq!(producer.exit(), "0");
        assert_eq!(consumer.exit(), "0");
        let fds = tasks
            .iter()
            .flat_map(|task| task.fd_entries())
            .map(|(_, path)| path)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data"), "{fds}");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data1"), "{fds}");
    }

    #[test]
    fn rc_shell_runs_three_stage_wasi_pipeline_through_provider_fds() {
        let host = RuntimeShellHost::fresh().unwrap();
        let runtime = host.runtime();
        host.write_file("producer.wasm", &wasi_producer()).unwrap();
        host.write_file("cat.wasm", &wasi_cat()).unwrap();
        let mut rc = RcShell::new(host);

        let out = rc.eval_line("wasi producer.wasm | wasi cat.wasm | wasi cat.wasm");
        assert!(
            out.status.is_success(),
            "status={} stdout={:?} stderr={:?}",
            out.status,
            out.stdout,
            out.stderr
        );
        assert_eq!(out.status.to_string(), "0|0|0", "{out:?}");
        assert_eq!(out.stdout, "pipe-ok\n", "{out:?}");

        let fds = runtime
            .task_fs()
            .tasks()
            .iter()
            .flat_map(|task| task.fd_entries())
            .map(|(_, path)| path)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data"), "{fds}");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe0/data1"), "{fds}");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe1/data"), "{fds}");
        assert!(fds.contains(".rc/graphs/rcgraph1/pipe1/data1"), "{fds}");
    }

    #[test]
    fn rc_shell_runs_external_background_job_and_waits_through_provider() {
        let host = RuntimeShellHost::fresh().unwrap();
        host.write_file("producer.wasm", &wasi_producer()).unwrap();
        let mut rc = RcShell::new(host);

        let out = rc.eval_line("wasi producer.wasm & wait 1");
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "[1]\npipe-ok\n[1] 0\n");
    }

    #[test]
    fn rc_shell_reports_unavailable_plan9_service_providers() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("srv -nqC tcp!9p.io sources /n/sources");
        assert!(!out.status.is_success());
        assert!(
            out.stderr.contains("provider not configured"),
            "{}",
            out.stderr
        );
    }

    fn wasi_producer() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))
              (memory (export "memory") 1)
              (data (i32.const 128) "pipe-ok\n")
              (func $assert_ok (param $errno i32)
                local.get $errno
                i32.eqz
                if
                else
                  (call $proc_exit (local.get $errno))
                end)
              (func (export "_start")
                (i32.store (i32.const 0) (i32.const 128))
                (i32.store (i32.const 4) (i32.const 8))
                (call $assert_ok
                  (call $fd_write
                    (i32.const 1)
                    (i32.const 0)
                    (i32.const 1)
                    (i32.const 8)))))
            "#,
        )
        .unwrap()
    }

    fn wasi_cat() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_read"
                (func $fd_read (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit"
                (func $proc_exit (param i32)))
              (memory (export "memory") 1)
              (func $assert_ok (param $errno i32)
                local.get $errno
                i32.eqz
                if
                else
                  (call $proc_exit (local.get $errno))
                end)
              (func (export "_start")
                (i32.store (i32.const 0) (i32.const 128))
                (i32.store (i32.const 4) (i32.const 64))
                (call $assert_ok
                  (call $fd_read
                    (i32.const 0)
                    (i32.const 0)
                    (i32.const 1)
                    (i32.const 8)))
                (i32.store (i32.const 16) (i32.const 128))
                (i32.store (i32.const 20) (i32.load (i32.const 8)))
                (call $assert_ok
                  (call $fd_write
                    (i32.const 1)
                    (i32.const 16)
                    (i32.const 1)
                    (i32.const 24)))))
            "#,
        )
        .unwrap()
    }
}
