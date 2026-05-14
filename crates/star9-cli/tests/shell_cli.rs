use std::process::Command;

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
