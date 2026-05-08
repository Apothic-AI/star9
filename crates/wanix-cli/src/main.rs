use clap::{Parser, Subcommand};
use wanix_core::{FileMode, Result};
use wanix_protocol::WanixApi;
use wanix_runtime::Runtime;

#[derive(Parser)]
#[command(name = "wanix")]
#[command(about = "Rust-native Wanix runtime CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Ls { path: String },
    Read { path: String },
    Write { path: String, value: String },
    Stat { path: String },
    Smoke,
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
