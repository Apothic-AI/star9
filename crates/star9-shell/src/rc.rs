use std::collections::BTreeMap;

use star9_core::{clean_path, Error, ErrorKind, FileMode, FsContext, Result};
use star9_fs::{fs_ref, FileSystem, MemFs, Node, PipeFs};
use star9_rc::{
    RcCommandInvocation, RcCommandResult, RcError, RcFdBindingSpec, RcHost, RcOutput,
    RcProcessGraphKind, RcProcessGraphRecord, RcProcessGraphSpec, RcProcessJobResult,
    RcProcessStageOutcome, RcProcessStageRecord, RcSession, RcStat, RcStatus,
};
use star9_task::Task;
use star9_vfs::BindMode;

use crate::{RuntimeShellHost, ShellHost, ShellSession};

pub type Star9RcSession = RcSession<Star9RcHost>;

#[derive(Clone)]
pub struct Star9RcHost {
    host: RuntimeShellHost,
    cwd: String,
    next_graph_id: u32,
}

impl Star9RcHost {
    pub fn new(host: RuntimeShellHost) -> Self {
        Self {
            host,
            cwd: ".".into(),
            next_graph_id: 1,
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
        Ok(None)
    }

    fn send_note_to_processes(&mut self, note: &str) -> star9_rc::RcResult<()> {
        match self.host.write_existing("#signal/data", note.as_bytes()) {
            Ok(()) => Ok(()),
            Err(_) => Ok(()),
        }
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
    if let Some(rest) = binding_path.strip_prefix("pipe:0/") {
        format!("{graph_root}/pipe0/{rest}")
    } else {
        format!("{graph_root}/{}", binding_path.trim_start_matches('/'))
    }
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
        assert!(out.status.is_success(), "{}", out.stderr);
        assert_eq!(out.stdout, "hello");
    }

    #[test]
    fn rc_shell_reaches_star9_devices() {
        let host = RuntimeShellHost::fresh().unwrap();
        let mut rc = RcShell::new(host);
        let out = rc.eval_line("ls '#task'");
        assert!(out.status.is_success(), "{}", out.stderr);
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
}
