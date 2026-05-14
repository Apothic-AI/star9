use std::borrow::Cow;
use std::io::{IsTerminal, Read, SeekFrom};
use std::sync::Arc;
use std::{io, path::PathBuf};

use clap::{Parser, Subcommand};
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, Reedline, Signal};
use star9_core::{ErrorKind, FileMode, OpenFlags, Result};
use star9_fs::{fs_ref, open, read_dir, read_file, stat, write_file, FileSystem, LocalFs, MemFs};
use star9_protocol::{
    p9::{serve_frame_stream, LoopbackTransport, NinePClientFs, NinePServer, TcpStreamTransport},
    runtime::{
        EnvironmentEntry, ExecutionKind, ExecutionSpec, PortDescriptor, PortHandoff,
        PortOpenRequest, RuntimeRequest, RuntimeResponse, StdioData, StdioSet, StdioStream,
        TaskMessage, TaskMessagePayload, WorkerHandle, WorkerSpawnRequest, WorkerStartRequest,
    },
    Star9Api,
};
use star9_rc::RcOutput;
#[cfg(not(target_arch = "wasm32"))]
use star9_runtime::NativePtyExecutionHandler;
use star9_runtime::{Runtime, WasmiWasiHandler};
use star9_shell::{rc::RcShell, RuntimeShellHost, ShellResult, ShellSession};

#[derive(Parser)]
#[command(name = "star9")]
#[command(about = "Rust-native Star 9 runtime CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Ls {
        path: String,
    },
    Read {
        path: String,
    },
    Write {
        path: String,
        value: String,
    },
    Stat {
        path: String,
    },
    ServeP9 {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    Shell {
        #[arg(short = 'c', long = "command")]
        command: Option<String>,
        #[arg(long)]
        native: bool,
        #[arg(long, help = "Use the small Star 9 admin shell instead of rc")]
        simple: bool,
        #[arg(long, hide = true, conflicts_with = "simple")]
        rc: bool,
        script: Option<PathBuf>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Rc {
        #[arg(short = 'c', long = "command")]
        command: Option<String>,
        #[arg(long)]
        native: bool,
        script: Option<PathBuf>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Accept {
        #[command(subcommand)]
        suite: AcceptanceCommand,
    },
    Smoke,
}

#[derive(Clone, Subcommand)]
enum AcceptanceCommand {
    P9,
    Devices,
    Wasi,
    Worker,
    Native,
    NativeP9,
    NativeTcp,
    All,
}

fn main() {
    match run() {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let runtime = Runtime::new()?;
    let api = Star9Api::new(runtime.root());
    match cli.command {
        Command::Ls { path } => {
            for entry in api.read_dir(&path)? {
                println!("{entry}");
            }
            Ok(0)
        }
        Command::Read { path } => {
            print!("{}", String::from_utf8_lossy(&api.read_file(&path)?));
            Ok(0)
        }
        Command::Write { path, value } => {
            api.write_file(&path, value.as_bytes())?;
            Ok(0)
        }
        Command::Stat { path } => {
            let stat = api.stat(&path)?;
            println!(
                "size={} mode={:o} dir={} modified_ms={}",
                stat.size, stat.mode, stat.is_dir, stat.modified_ms
            );
            Ok(0)
        }
        Command::ServeP9 { root } => {
            let server = NinePServer::new(fs_ref(LocalFs::new(root)));
            let mut stdin = io::stdin().lock();
            let mut stdout = io::stdout().lock();
            serve_frame_stream(&server, &mut stdin, &mut stdout)?;
            Ok(0)
        }
        Command::Shell {
            command,
            native,
            simple,
            rc: _,
            script,
            args,
        } => {
            let host = RuntimeShellHost::new(runtime).with_writable_workspace()?;
            let host = if native { host.enable_native() } else { host };
            if simple {
                if !args.is_empty() {
                    return Err(star9_core::Error::Message(
                        "shell --simple does not accept script arguments".into(),
                    ));
                }
                run_shell(host, command, script)
            } else {
                run_rc_shell(host, command, script, args)
            }
        }
        Command::Rc {
            command,
            native,
            script,
            args,
        } => {
            let host = RuntimeShellHost::new(runtime).with_writable_workspace()?;
            let host = if native { host.enable_native() } else { host };
            run_rc_shell(host, command, script, args)
        }
        Command::Accept { suite } => {
            print!("{}", render_acceptance_output(suite)?);
            Ok(0)
        }
        Command::Smoke => {
            let ns = runtime.namespace();
            let ram = star9_fs::fs_ref(star9_fs::MemFs::new());
            ns.bind(ram, ".", "tmp", star9_vfs::BindMode::Replace)?;
            star9_fs::write_file(
                ns.as_ref(),
                "tmp/smoke",
                b"ok\n",
                FileMode::from_perm(0o644),
            )?;
            print!(
                "{}",
                String::from_utf8_lossy(&star9_fs::read_file(ns.as_ref(), "tmp/smoke")?)
            );
            Ok(0)
        }
    }
}

fn run_shell(
    host: RuntimeShellHost,
    command: Option<String>,
    script: Option<PathBuf>,
) -> Result<i32> {
    let mut shell = ShellSession::new(host);
    if let Some(command) = command {
        return Ok(print_shell_result(shell.eval_line(&command)));
    }
    if let Some(script) = script {
        let source = std::fs::read_to_string(&script).map_err(|err| {
            star9_core::Error::Message(format!("shell: failed to read {}: {err}", script.display()))
        })?;
        return Ok(run_shell_script(&mut shell, &source));
    }
    if !io::stdin().is_terminal() {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source).map_err(|err| {
            star9_core::Error::Message(format!("shell: failed to read stdin: {err}"))
        })?;
        return Ok(run_shell_script(&mut shell, &source));
    }
    run_interactive_shell(&mut shell)
}

fn run_shell_script(shell: &mut ShellSession<RuntimeShellHost>, source: &str) -> i32 {
    let mut status = 0;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        status = print_shell_result(shell.eval_line(trimmed));
    }
    status
}

fn run_interactive_shell(shell: &mut ShellSession<RuntimeShellHost>) -> Result<i32> {
    let mut editor = Reedline::create();
    loop {
        let prompt = Star9Prompt(shell.prompt());
        match editor
            .read_line(&prompt)
            .map_err(|err| star9_core::Error::Message(format!("shell: {err}")))?
        {
            Signal::Success(line) => {
                let _ = print_shell_result(shell.eval_line(&line));
            }
            Signal::CtrlD => return Ok(0),
            Signal::CtrlC => {
                eprintln!("^C");
            }
            Signal::ExternalBreak(line) => {
                let _ = print_shell_result(shell.eval_line(&line));
            }
            _ => {}
        }
    }
}

fn run_rc_shell(
    host: RuntimeShellHost,
    command: Option<String>,
    script: Option<PathBuf>,
    args: Vec<String>,
) -> Result<i32> {
    let mut shell = RcShell::new(host);
    if let Some(command) = command {
        shell.set_args(args);
        return Ok(print_rc_result(shell.eval_line(&command)));
    }
    if let Some(script) = script {
        let source = std::fs::read_to_string(&script).map_err(|err| {
            star9_core::Error::Message(format!("rc: failed to read {}: {err}", script.display()))
        })?;
        shell.set_argv0(script.display().to_string());
        shell.set_args(args);
        return Ok(print_rc_result(shell.eval_line(&source)));
    }
    if !io::stdin().is_terminal() {
        let mut source = String::new();
        io::stdin().read_to_string(&mut source).map_err(|err| {
            star9_core::Error::Message(format!("rc: failed to read stdin: {err}"))
        })?;
        return Ok(print_rc_result(shell.eval_line(&source)));
    }
    run_interactive_rc_shell(&mut shell)
}

fn run_interactive_rc_shell(shell: &mut RcShell) -> Result<i32> {
    let mut editor = Reedline::create();
    loop {
        let prompt = Star9Prompt(shell.prompt());
        match editor
            .read_line(&prompt)
            .map_err(|err| star9_core::Error::Message(format!("rc: {err}")))?
        {
            Signal::Success(line) => {
                let _ = print_rc_result(shell.eval_line(&line));
            }
            Signal::CtrlD => return Ok(0),
            Signal::CtrlC => {
                eprintln!("^C");
            }
            Signal::ExternalBreak(line) => {
                let _ = print_rc_result(shell.eval_line(&line));
            }
            _ => {}
        }
    }
}

fn print_shell_result(result: ShellResult) -> i32 {
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    result.status
}

fn print_rc_result(result: RcOutput) -> i32 {
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    if result.status.is_success() {
        0
    } else {
        1
    }
}

struct Star9Prompt(String);

impl Prompt for Star9Prompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(&self.0)
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("::: ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        Cow::Owned(format!("(reverse-search: {}) ", history_search.term))
    }
}

fn render_acceptance_output(suite: AcceptanceCommand) -> Result<String> {
    match suite {
        AcceptanceCommand::P9 => render_p9_acceptance(),
        AcceptanceCommand::Devices => render_device_acceptance(),
        AcceptanceCommand::Wasi => render_wasi_acceptance(),
        AcceptanceCommand::Worker => render_worker_acceptance(),
        AcceptanceCommand::Native => render_native_acceptance(),
        AcceptanceCommand::NativeP9 => render_native_p9_acceptance(),
        AcceptanceCommand::NativeTcp => render_native_tcp_acceptance(),
        AcceptanceCommand::All => Ok(format!(
            "{}{}{}{}",
            render_p9_acceptance()?,
            render_device_acceptance()?,
            render_wasi_acceptance()?,
            render_worker_acceptance()?
        )),
    }
}

fn render_p9_acceptance() -> Result<String> {
    let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
    let server = Arc::new(NinePServer::new(fs_ref(mem.clone())));
    let client = NinePClientFs::connect(Arc::new(LoopbackTransport::new(server)))?;

    let root = join_dir_names(&client, ".")?;
    let original = String::from_utf8_lossy(&read_file(&client, "dir/file.txt")?).into_owned();
    write_file(
        &client,
        "dir/created.txt",
        b"created",
        FileMode::from_perm(0o644),
    )?;
    let mut created = client.open_file("dir/created.txt", OpenFlags::RDWR, FileMode::empty())?;
    created.write_at(b"XX", 2)?;
    created.close()?;
    let updated = String::from_utf8_lossy(&read_file(&client, "dir/created.txt")?).into_owned();
    let entries = join_dir_names(&client, "dir")?;

    client.mkdir("empty", FileMode::DIR | FileMode::from_perm(0o755))?;
    client.remove("dir/created.txt")?;
    client.remove("empty")?;

    Ok(format!(
        "p9 root={} read={} write={} entries={} removed_file={} removed_dir={}\n",
        root,
        original.trim_end(),
        updated.trim_end(),
        entries,
        is_not_found(&client, "dir/created.txt"),
        is_not_found(&client, "empty"),
    ))
}

fn render_device_acceptance() -> Result<String> {
    let runtime = Runtime::new()?;
    let ns = runtime.namespace();
    let ns_ref = ns.as_ref();

    let term_id = read_trimmed(ns_ref, "#term/new")?;
    let mut program_reader = open(ns_ref, &format!("#term/{term_id}/program"))?;
    let mut program_writer = open(ns_ref, &format!("#term/{term_id}/program"))?;
    let mut raw_reader = open(ns_ref, &format!("#term/{term_id}/raw"))?;
    let mut raw_writer = open(ns_ref, &format!("#term/{term_id}/raw"))?;
    program_writer.write(b"run")?;
    let first_program = read_once(program_reader.as_mut())?;
    program_writer.write(b"\nnext\n")?;
    let second_program = read_once(program_reader.as_mut())?;
    raw_writer.write(b"\nraw\n")?;
    let raw_program = read_once(raw_reader.as_mut())?;
    program_reader.close()?;
    program_writer.close()?;
    raw_reader.close()?;
    raw_writer.close()?;

    let mut data_reader = open(ns_ref, &format!("#term/{term_id}/data"))?;
    let mut data_writer = open(ns_ref, &format!("#term/{term_id}/data"))?;
    data_writer.write(b"screen")?;
    let term_data = read_once(data_reader.as_mut())?;
    data_reader.close()?;
    data_writer.close()?;
    let term_screen = read_text(ns_ref, &format!("#term/{term_id}/screen"))?;

    let mut winch_reader = open(ns_ref, &format!("#term/{term_id}/winch/data"))?;
    let mut winch_writer = open(ns_ref, &format!("#term/{term_id}/winch/data"))?;
    winch_writer.write(b"120x40")?;
    let winch = read_once(winch_reader.as_mut())?;
    winch_reader.close()?;
    winch_writer.close()?;

    write_handle(ns_ref, &format!("#term/{term_id}/size"), b"132x43")?;
    write_handle(ns_ref, &format!("#term/{term_id}/ctl"), b"reset")?;
    let term_state = read_trimmed(ns_ref, &format!("#term/{term_id}/state"))?;
    let term_size = read_trimmed(ns_ref, &format!("#term/{term_id}/size"))?;

    let vm_id = read_trimmed(ns_ref, "#vm/new/firecracker")?;
    write_handle(ns_ref, &format!("#vm/{vm_id}/alias"), b"guest-a")?;
    write_handle(ns_ref, &format!("#vm/{vm_id}/config"), b"mem=128M cpu=1")?;
    runtime.set_vm_guest(
        &vm_id,
        fs_ref(MemFs::from_entries([(
            "etc/issue",
            b"star9 guest".to_vec(),
        )])),
    )?;
    write_handle(ns_ref, &format!("#vm/{vm_id}/ctl"), b"start")?;
    write_handle(ns_ref, &format!("#vm/{vm_id}/ctl"), b"stop")?;
    let vm_kind = read_trimmed(ns_ref, &format!("#vm/{vm_id}/kind"))?;
    let vm_alias = read_trimmed(ns_ref, &format!("#vm/{vm_id}/alias"))?;
    let vm_state = read_trimmed(ns_ref, &format!("#vm/{vm_id}/state"))?;
    let vm_console = escape_text(&read_text(ns_ref, &format!("#vm/{vm_id}/console"))?);
    let vm_guest = read_trimmed(ns_ref, &format!("#vm/{vm_id}/guest/etc/issue"))?;

    let listener_id = read_trimmed(ns_ref, "#net/new")?;
    let client_id = read_trimmed(ns_ref, "#net/new")?;
    write_handle(
        ns_ref,
        &format!("#net/{listener_id}/ctl"),
        b"announce service:7",
    )?;
    write_handle(ns_ref, &format!("#net/{client_id}/ctl"), b"dial service:7")?;
    let client_status = read_trimmed(ns_ref, &format!("#net/{client_id}/status"))?;
    let mut listen = open(ns_ref, &format!("#net/{listener_id}/listen"))?;
    let accepted_id = read_once(listen.as_mut())?.trim().to_string();
    listen.close()?;
    let accepted_status = read_trimmed(ns_ref, &format!("#net/{accepted_id}/status"))?;

    let mut accepted_reader = open(ns_ref, &format!("#net/{accepted_id}/data"))?;
    let mut client_writer = open(ns_ref, &format!("#net/{client_id}/data"))?;
    client_writer.write(b"payload")?;
    let payload = read_once(accepted_reader.as_mut())?;
    accepted_reader.close()?;
    client_writer.close()?;

    let mut client_reader = open(ns_ref, &format!("#net/{client_id}/data"))?;
    let mut accepted_writer = open(ns_ref, &format!("#net/{accepted_id}/data"))?;
    accepted_writer.write(b"reply")?;
    let reply = read_once(client_reader.as_mut())?;
    client_reader.close()?;
    accepted_writer.close()?;

    write_handle(ns_ref, &format!("#net/{client_id}/ctl"), b"hangup")?;
    let closed = read_trimmed(ns_ref, &format!("#net/{client_id}/status"))?;

    Ok(format!(
        concat!(
            "device term id={} program={} raw={} data={} screen={} winch={} state={} size={}\n",
            "device vm id={} kind={} alias={} state={} guest={} console={}\n",
            "device net listener={} client={} accepted={} client_status={} accepted_status={} data={} reply={} closed={}\n"
        ),
        term_id,
        escape_text(&(first_program + &second_program)),
        escape_text(&raw_program),
        escape_text(&term_data),
        escape_text(&term_screen),
        escape_text(&winch),
        term_state,
        term_size,
        vm_id,
        vm_kind,
        vm_alias,
        vm_state,
        vm_guest,
        vm_console,
        listener_id,
        client_id,
        accepted_id,
        client_status,
        accepted_status,
        escape_text(&payload),
        escape_text(&reply),
        closed,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn render_native_p9_acceptance() -> Result<String> {
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| star9_core::Error::Message(format!("native 9p bind failed: {err}")))?;
    let addr = listener
        .local_addr()
        .map_err(|err| star9_core::Error::Message(format!("native 9p local_addr failed: {err}")))?;
    let server = NinePServer::new(fs_ref(MemFs::from_entries([(
        "dir/file.txt",
        b"hello".to_vec(),
    )])));

    let serving = thread::spawn(move || -> Result<usize> {
        let (stream, _) = listener
            .accept()
            .map_err(|err| star9_core::Error::Message(format!("native 9p accept failed: {err}")))?;
        let mut reader = stream
            .try_clone()
            .map_err(|err| star9_core::Error::Message(format!("native 9p clone failed: {err}")))?;
        let mut writer = stream;
        serve_frame_stream(&server, &mut reader, &mut writer)
    });

    let stream = TcpStream::connect(addr)
        .map_err(|err| star9_core::Error::Message(format!("native 9p connect failed: {err}")))?;
    let client = NinePClientFs::connect(Arc::new(TcpStreamTransport::new(stream)))?;
    let read = read_trimmed(&client, "dir/file.txt")?;
    write_file(
        &client,
        "dir/created.txt",
        b"created",
        FileMode::from_perm(0o644),
    )?;
    let created = read_trimmed(&client, "dir/created.txt")?;
    drop(client);
    let served = serving
        .join()
        .map_err(|_| star9_core::Error::Message("native 9p server thread panicked".into()))??;

    Ok(format!(
        "native-p9 addr={} read={} created={} frames={}\n",
        addr, read, created, served
    ))
}

#[cfg(target_arch = "wasm32")]
fn render_native_p9_acceptance() -> Result<String> {
    Err(star9_core::Error::Message(
        "native 9p acceptance requires a non-wasm host".into(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn render_native_tcp_acceptance() -> Result<String> {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| star9_core::Error::Message(format!("native tcp bind failed: {err}")))?;
    let addr = listener.local_addr().map_err(|err| {
        star9_core::Error::Message(format!("native tcp local_addr failed: {err}"))
    })?;

    let server = thread::spawn(move || -> io::Result<Vec<u8>> {
        let (mut stream, _) = listener.accept()?;
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request)?;
        stream.write_all(b"pong")?;
        Ok(request.to_vec())
    });

    let mut client = TcpStream::connect(addr)
        .map_err(|err| star9_core::Error::Message(format!("native tcp connect failed: {err}")))?;
    client
        .write_all(b"ping")
        .map_err(|err| star9_core::Error::Message(format!("native tcp write failed: {err}")))?;
    let mut response = [0_u8; 4];
    client
        .read_exact(&mut response)
        .map_err(|err| star9_core::Error::Message(format!("native tcp read failed: {err}")))?;
    let request = server
        .join()
        .map_err(|_| star9_core::Error::Message("native tcp server thread panicked".into()))?
        .map_err(|err| star9_core::Error::Message(format!("native tcp accept failed: {err}")))?;

    Ok(format!(
        "native-tcp addr={} request={} response={}\n",
        addr,
        String::from_utf8_lossy(&request),
        String::from_utf8_lossy(&response)
    ))
}

#[cfg(target_arch = "wasm32")]
fn render_native_tcp_acceptance() -> Result<String> {
    Err(star9_core::Error::Message(
        "native tcp acceptance requires a non-wasm host".into(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn render_native_acceptance() -> Result<String> {
    let runtime = Runtime::new()?;
    runtime
        .execution_registry()
        .register_kind(ExecutionKind::Native, NativePtyExecutionHandler::new());
    let task = runtime.task_fs().alloc("auto", Some(runtime.root()))?;
    let status = runtime.execution_registry().execute(
        &task,
        &ExecutionSpec {
            kind: ExecutionKind::Native,
            module: "/bin/sh".into(),
            args: vec!["-c".into(), "printf native-pty-ok".into()],
            env: Vec::new(),
            cwd: None,
            stdio: StdioSet::default(),
            fds: Vec::new(),
        },
    )?;
    let stdout = escape_text(&read_task_fd(&task, 1)?);
    Ok(format!(
        "native module=/bin/sh exit={} stdout={}\n",
        render_status(&status),
        stdout
    ))
}

#[cfg(target_arch = "wasm32")]
fn render_native_acceptance() -> Result<String> {
    Err(star9_core::Error::Message(
        "native acceptance requires a non-wasm host".into(),
    ))
}

fn render_worker_acceptance() -> Result<String> {
    let runtime = Runtime::new()?;
    let host = runtime.protocol_host();

    let source = expect_worker(host.handle_request(RuntimeRequest::SpawnWorker(
        WorkerSpawnRequest {
            worker: worker_request("worker-a"),
            parent_task_id: None,
        },
    ))?)?;
    let target = expect_worker(host.handle_request(RuntimeRequest::SpawnWorker(
        WorkerSpawnRequest {
            worker: worker_request("worker-b"),
            parent_task_id: None,
        },
    ))?)?;

    expect_unit(
        host.handle_request(RuntimeRequest::StartWorker(WorkerStartRequest {
            worker: source.clone(),
            execution: ExecutionSpec {
                kind: ExecutionKind::Wasi,
                module: "cli-worker.wasm".into(),
                args: vec!["--smoke".into()],
                env: vec![EnvironmentEntry {
                    name: "MODE".into(),
                    value: "acceptance".into(),
                }],
                cwd: Some("/work".into()),
                stdio: StdioSet::default(),
                fds: Vec::new(),
            },
        }))?,
    )?;

    let opened = expect_port(
        host.handle_request(RuntimeRequest::OpenPort(PortOpenRequest {
            worker: source.clone(),
            port: PortDescriptor {
                port_id: "events".into(),
                name: "event-bus".into(),
            },
        }))?,
    )?;
    let handed = expect_port(
        host.handle_request(RuntimeRequest::HandoffPort(PortHandoff {
            worker: source.clone(),
            target_task_id: target.task_id.clone(),
            port: opened.clone(),
        }))?,
    )?;
    let _ = host.handle_request(RuntimeRequest::PostMessage(TaskMessage {
        task_id: source.task_id.clone(),
        worker_id: Some(source.worker_id.clone()),
        sequence: 1,
        payload: TaskMessagePayload::StdioData(StdioData {
            stream: StdioStream::Stdout,
            data: b"accepted".to_vec(),
            eof: false,
        }),
    }))?;

    let source_task = runtime.task_fs().lookup(&source.task_id)?;
    let target_task = runtime.task_fs().lookup(&target.task_id)?;
    let stdout = escape_text(&read_task_fd(&source_task, 1)?);

    Ok(format!(
        concat!(
            "worker source={} task={} task_worker={} cmd={} dir={} env={} exit={} fds={} stdout={}\n",
            "worker target={} task={} parent={}\n",
            "worker port={} name={} handed_to={}\n"
        ),
        source.worker_id,
        source.task_id,
        source_task.worker().unwrap_or_default(),
        source_task.cmd(),
        source_task.dir(),
        source_task.env().join(","),
        source_task.exit(),
        format_fds(&source_task.fd_entries()),
        stdout,
        target.worker_id,
        target.task_id,
        target_task
            .parent()
            .map(|task| task.id())
            .unwrap_or_default(),
        handed.port_id,
        handed.name,
        target.task_id,
    ))
}

fn render_wasi_acceptance() -> Result<String> {
    let runtime = Runtime::new()?;
    let task = runtime.task_fs().alloc("auto", Some(runtime.root()))?;
    let program = include_bytes!("../../../tests/fixtures/wasi-preview1-smoke.wasm").to_vec();
    task.namespace().bind(
        fs_ref(MemFs::from_entries([
            ("program.wasm", program),
            ("stdout.txt", Vec::new()),
        ])),
        ".",
        "workspace",
        star9_vfs::BindMode::Replace,
    )?;
    runtime
        .execution_registry()
        .register_kind(ExecutionKind::Wasi, WasmiWasiHandler::new());

    let status = runtime.execution_registry().execute(
        &task,
        &ExecutionSpec {
            kind: ExecutionKind::Wasi,
            module: "program.wasm".into(),
            args: vec!["acceptance".into()],
            env: vec![EnvironmentEntry {
                name: "MODE".into(),
                value: "acceptance".into(),
            }],
            cwd: Some("workspace".into()),
            stdio: StdioSet {
                stdin: star9_protocol::runtime::StreamDescriptor::Null,
                stdout: star9_protocol::runtime::StreamDescriptor::Fd(
                    star9_protocol::runtime::FdDescriptor {
                        fd: 1,
                        kind: star9_protocol::runtime::FdKind::File,
                        path: Some("workspace/stdout.txt".into()),
                        read: false,
                        write: true,
                    },
                ),
                stderr: star9_protocol::runtime::StreamDescriptor::Null,
            },
            fds: Vec::new(),
        },
    )?;
    let stdout = escape_text(&read_text(
        task.namespace().as_ref(),
        "workspace/stdout.txt",
    )?);
    Ok(format!(
        "wasi fixture=preview1-smoke exit={} stdout={}\n",
        render_status(&status),
        stdout,
    ))
}

fn join_dir_names(fsys: &dyn FileSystem, path: &str) -> Result<String> {
    Ok(read_dir(fsys, path)?
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>()
        .join(","))
}

fn read_trimmed(fsys: &dyn FileSystem, path: &str) -> Result<String> {
    Ok(read_text(fsys, path)?.trim().to_string())
}

fn read_text(fsys: &dyn FileSystem, path: &str) -> Result<String> {
    Ok(String::from_utf8_lossy(&read_file(fsys, path)?).into_owned())
}

fn read_once(file: &mut dyn star9_fs::FileHandle) -> Result<String> {
    let mut buf = [0_u8; 256];
    let n = file.read(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn read_task_fd(task: &star9_task::Task, fd: u32) -> Result<String> {
    task.with_fd_mut(fd, |file| {
        file.seek(SeekFrom::Start(0))?;
        read_once(file)
    })
}

fn write_handle(fsys: &dyn FileSystem, path: &str, data: &[u8]) -> Result<()> {
    let mut file = open(fsys, path)?;
    let written = file.write(data)?;
    if written != data.len() {
        return Err(ErrorKind::UnexpectedEof.into());
    }
    file.close()
}

fn is_not_found(fsys: &dyn FileSystem, path: &str) -> bool {
    matches!(stat(fsys, path), Err(err) if err.kind() == ErrorKind::NotFound)
}

fn escape_text(value: &str) -> String {
    value.escape_default().collect()
}

fn worker_request(worker_id: &str) -> WorkerHandle {
    WorkerHandle {
        worker_id: worker_id.into(),
        task_id: "ignored".into(),
    }
}

fn expect_worker(response: RuntimeResponse) -> Result<WorkerHandle> {
    match response {
        RuntimeResponse::Worker(handle) => Ok(handle),
        _ => Err(ErrorKind::Invalid.into()),
    }
}

fn expect_port(response: RuntimeResponse) -> Result<PortDescriptor> {
    match response {
        RuntimeResponse::Port(port) => Ok(port),
        _ => Err(ErrorKind::Invalid.into()),
    }
}

fn expect_unit(response: RuntimeResponse) -> Result<()> {
    match response {
        RuntimeResponse::Unit => Ok(()),
        _ => Err(ErrorKind::Invalid.into()),
    }
}

fn format_fds(entries: &[(u32, String)]) -> String {
    entries
        .iter()
        .map(|(fd, path)| format!("{fd}:{path}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn render_status(status: &star9_protocol::runtime::ExitStatus) -> String {
    match status {
        star9_protocol::runtime::ExitStatus::ExitCode(code) => code.to_string(),
        star9_protocol::runtime::ExitStatus::Signal(signal) => format!("signal:{signal}"),
        star9_protocol::runtime::ExitStatus::Trap(reason) => format!("trap:{reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p9_acceptance_output_is_stable() {
        assert_eq!(
            render_p9_acceptance().unwrap(),
            "p9 root=dir read=hello write=crXXted entries=created.txt,file.txt removed_file=true removed_dir=true\n"
        );
    }

    #[test]
    fn device_acceptance_output_is_stable() {
        assert_eq!(
            render_device_acceptance().unwrap(),
            concat!(
                "device term id=1 program=run\\r\\nnext\\r\\n raw=\\nraw\\n data=screen screen=screen winch=120x40 state=ready size=80x24\n",
                "device vm id=1 kind=firecracker alias=guest-a state=stopped guest=star9 guest console=start\\nstop\\n\n",
                "device net listener=1 client=2 accepted=3 client_status=connected local=local:10001 remote=service:7 accepted_status=connected local=service:7 remote=local:10001 data=payload reply=reply closed=closed local=local:10001 remote=service:7\n"
            )
        );
    }

    #[test]
    fn worker_acceptance_output_is_stable() {
        assert_eq!(
            render_worker_acceptance().unwrap(),
            concat!(
                "worker source=worker-a task=2 task_worker=worker-a cmd=cli-worker.wasm --smoke dir=/work env=MODE=acceptance exit=started fds=0:stdin,1:stdout,2:stderr stdout=accepted\n",
                "worker target=worker-b task=3 parent=1\n",
                "worker port=events name=event-bus handed_to=3\n"
            )
        );
    }

    #[test]
    fn wasi_acceptance_output_is_stable() {
        assert_eq!(
            render_wasi_acceptance().unwrap(),
            "wasi fixture=preview1-smoke exit=0 stdout=compiled-wasi-ok\\n\n"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_p9_acceptance_uses_real_tcp_stream_transport() {
        let output = render_native_p9_acceptance().unwrap();
        assert!(output.starts_with("native-p9 addr=127.0.0.1:"));
        assert!(output.contains(" read=hello created=created frames="));
    }
}
