//! Startup checks that do not need a graphical clipboard session.
use std::process::Command;

#[test]
fn standalone_helpers_show_help_without_opening_a_clipboard() {
    for (binary, usage) in [
        (env!("CARGO_BIN_EXE_zcopy"), "Usage: zcopy [OPTIONS]"),
        (env!("CARGO_BIN_EXE_zpaste"), "Usage: zpaste [OPTIONS]"),
    ] {
        let output = Command::new(binary).arg("--help").output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains(usage));
    }
}

#[test]
fn invalid_arguments_fail_before_clipboard_access() {
    for binary in [env!("CARGO_BIN_EXE_zcopy"), env!("CARGO_BIN_EXE_zpaste")] {
        let output = Command::new(binary).arg("--unknown").output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown"));
    }
}
