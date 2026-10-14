//! End-to-end tests of the experimental executable's hosted child console.
#![cfg(all(windows, feature = "harness"))]
use std::process::Command;

fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_console-server")
}
#[test]
fn cmd_and_its_nested_child_write_to_the_private_console() {
    let path = std::env::temp_dir().join(format!(
        "zetta-condrv-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(executable())
        .args([
            "--command",
            "cmd.exe /d /c \"echo parent & cmd.exe /d /c echo nested\"",
            "--capture",
        ])
        .arg(&path)
        .args(["--timeout", "5"])
        .output()
        .unwrap();
    let captured = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(captured.contains("parent"), "{captured:?}");
    assert!(captured.contains("nested"), "{captured:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"child_exit_code\": 0"));
}
#[test]
fn ctrl_c_reaches_a_registered_child_handler() {
    let output = Command::new(executable())
        .args(["--ctrl-c", "--timeout", "5"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"ctrl_c_handled\": true"));
}
#[test]
fn timeout_cleans_up_the_owned_console_tree() {
    let start = std::time::Instant::now();
    let command = format!("\"{}\" --control-child", executable());
    let output = Command::new(executable())
        .args(["--command", &command, "--timeout", "1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    assert!(start.elapsed().as_secs() < 10);
}

#[test]
fn native_console_api_probe_includes_ansi_and_unicode_titles() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/console_probe.py");
    let command = format!("python.exe \"{}\"", script.display());
    let output = Command::new(executable())
        .args(["--command", &command, "--timeout", "10"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("\"child_exit_code\": 0"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
