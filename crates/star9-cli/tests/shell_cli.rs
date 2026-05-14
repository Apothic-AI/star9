use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn shell_command_mode_runs_file_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_star9"))
        .args([
            "shell",
            "-c",
            "mkdir demo; write demo/hello hello; cat demo/hello",
        ])
        .output()
        .expect("star9 shell command runs");

    assert!(
        output.status.success(),
        "status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
}

#[test]
fn shell_command_mode_lists_task_device() {
    let output = Command::new(env!("CARGO_BIN_EXE_star9"))
        .args(["shell", "-c", "ls #task"])
        .output()
        .expect("star9 shell command runs");

    assert!(
        output.status.success(),
        "status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1/"), "{stdout}");
    assert!(stdout.contains("new/"), "{stdout}");
}

#[test]
fn rc_command_mode_runs_rc_language_features() {
    let output = Command::new(env!("CARGO_BIN_EXE_star9"))
        .args([
            "rc",
            "-c",
            "x=(one two); fn twice { echo $1 $1 }; for(i in $x) twice $i",
        ])
        .output()
        .expect("star9 rc command runs");

    assert!(
        output.status.success(),
        "status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "one one\ntwo two\n"
    );
}

#[test]
fn shell_rc_flag_runs_rc_language_features() {
    let output = Command::new(env!("CARGO_BIN_EXE_star9"))
        .args(["shell", "--rc", "-c", "echo a | cat"])
        .output()
        .expect("star9 shell --rc command runs");

    assert!(
        output.status.success(),
        "status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "a\n");
}

#[test]
fn rc_script_mode_sets_argv0_and_script_args() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let script = std::env::temp_dir().join(format!("star9-rc-cli-{id}.rc"));
    std::fs::write(&script, "echo script-$1-$0\n").expect("write rc script");

    let output = Command::new(env!("CARGO_BIN_EXE_star9"))
        .arg("rc")
        .arg(&script)
        .arg("world")
        .output()
        .expect("star9 rc script runs");
    let _ = std::fs::remove_file(&script);

    assert!(
        output.status.success(),
        "status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("script-world-{}\n", script.display())
    );
}
