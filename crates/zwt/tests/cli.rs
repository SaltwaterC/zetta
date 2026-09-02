use std::process::Command;

#[test]
fn standalone_binary_prints_its_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_zwt"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("Zetta Git worktree workflow\n\nUsage: zwt <COMMAND>"));
    assert!(stdout.contains("zwt abort [OPTIONS]"));
    assert!(stdout.contains("zwt sync [COMMIT]"));
    assert!(stdout.contains("zwt config"));
    assert!(!stdout.contains("zwt rerere"));
}

#[test]
fn standalone_binary_reports_invalid_operations() {
    let output = Command::new(env!("CARGO_BIN_EXE_zwt"))
        .arg("not-an-operation")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown zwt operation"));
}
