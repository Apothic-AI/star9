use star9_core::{clean_path, Error, ErrorKind, Result};
use star9_rc::{
    RcCommandInvocation, RcCommandResult, RcError, RcHost, RcOutput, RcSession, RcStat, RcStatus,
};

use crate::{RuntimeShellHost, ShellHost, ShellSession};

pub type Star9RcSession = RcSession<Star9RcHost>;

#[derive(Clone)]
pub struct Star9RcHost {
    host: RuntimeShellHost,
    cwd: String,
}

impl Star9RcHost {
    pub fn new(host: RuntimeShellHost) -> Self {
        Self {
            host,
            cwd: ".".into(),
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
}
