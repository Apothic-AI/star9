use std::io::SeekFrom;
use std::sync::Arc;
use std::{io, path::PathBuf};

use clap::{Parser, Subcommand};
use wanix_core::{ErrorKind, FileMode, OpenFlags, Result};
use wanix_fs::{fs_ref, open, read_dir, read_file, stat, write_file, FileSystem, LocalFs, MemFs};
use wanix_protocol::{
    p9::{serve_frame_stream, LoopbackTransport, NinePClientFs, NinePServer},
    runtime::{
        EnvironmentEntry, ExecutionKind, ExecutionSpec, PortDescriptor, PortHandoff,
        PortOpenRequest, RuntimeRequest, RuntimeResponse, StdioData, StdioSet, StdioStream,
        TaskMessage, TaskMessagePayload, WorkerHandle, WorkerSpawnRequest, WorkerStartRequest,
    },
    WanixApi,
};
#[cfg(not(target_arch = "wasm32"))]
use wanix_runtime::NativePtyExecutionHandler;
use wanix_runtime::{Runtime, WasmiWasiHandler};

#[derive(Parser)]
#[command(name = "wanix")]
#[command(about = "Rust-native Wanix runtime CLI")]
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
    All,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let runtime = Runtime::new()?;
    let api = WanixApi::new(runtime.root());
    match cli.command {
        Command::Ls { path } => {
            for entry in api.read_dir(&path)? {
                println!("{entry}");
            }
        }
        Command::Read { path } => {
            print!("{}", String::from_utf8_lossy(&api.read_file(&path)?));
        }
        Command::Write { path, value } => {
            api.write_file(&path, value.as_bytes())?;
        }
        Command::Stat { path } => {
            let stat = api.stat(&path)?;
            println!(
                "size={} mode={:o} dir={} modified_ms={}",
                stat.size, stat.mode, stat.is_dir, stat.modified_ms
            );
        }
        Command::ServeP9 { root } => {
            let server = NinePServer::new(fs_ref(LocalFs::new(root)));
            let mut stdin = io::stdin().lock();
            let mut stdout = io::stdout().lock();
            serve_frame_stream(&server, &mut stdin, &mut stdout)?;
        }
        Command::Accept { suite } => {
            print!("{}", render_acceptance_output(suite)?);
        }
        Command::Smoke => {
            let ns = runtime.namespace();
            let ram = wanix_fs::fs_ref(wanix_fs::MemFs::new());
            ns.bind(ram, ".", "tmp", wanix_vfs::BindMode::Replace)?;
            wanix_fs::write_file(
                ns.as_ref(),
                "tmp/smoke",
                b"ok\n",
                FileMode::from_perm(0o644),
            )?;
            print!(
                "{}",
                String::from_utf8_lossy(&wanix_fs::read_file(ns.as_ref(), "tmp/smoke")?)
            );
        }
    }
    Ok(())
}

fn render_acceptance_output(suite: AcceptanceCommand) -> Result<String> {
    match suite {
        AcceptanceCommand::P9 => render_p9_acceptance(),
        AcceptanceCommand::Devices => render_device_acceptance(),
        AcceptanceCommand::Wasi => render_wasi_acceptance(),
        AcceptanceCommand::Worker => render_worker_acceptance(),
        AcceptanceCommand::Native => render_native_acceptance(),
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
            b"wanix guest".to_vec(),
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
    Err(wanix_core::Error::Message(
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
        wanix_vfs::BindMode::Replace,
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
                stdin: wanix_protocol::runtime::StreamDescriptor::Null,
                stdout: wanix_protocol::runtime::StreamDescriptor::Fd(
                    wanix_protocol::runtime::FdDescriptor {
                        fd: 1,
                        kind: wanix_protocol::runtime::FdKind::File,
                        path: Some("workspace/stdout.txt".into()),
                        read: false,
                        write: true,
                    },
                ),
                stderr: wanix_protocol::runtime::StreamDescriptor::Null,
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

fn read_once(file: &mut dyn wanix_fs::FileHandle) -> Result<String> {
    let mut buf = [0_u8; 256];
    let n = file.read(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn read_task_fd(task: &wanix_task::Task, fd: u32) -> Result<String> {
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

fn render_status(status: &wanix_protocol::runtime::ExitStatus) -> String {
    match status {
        wanix_protocol::runtime::ExitStatus::ExitCode(code) => code.to_string(),
        wanix_protocol::runtime::ExitStatus::Signal(signal) => format!("signal:{signal}"),
        wanix_protocol::runtime::ExitStatus::Trap(reason) => format!("trap:{reason}"),
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
                "device vm id=1 kind=firecracker alias=guest-a state=stopped guest=wanix guest console=start\\nstop\\n\n",
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
}
