//! Public CLI checks for the clipboard helper boundary.
#![cfg(all(feature = "clipboard", unix))]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

fn wait_until_executable(path: &Path, probe: &str) {
    // A parallel test can fork while this file is open for writing and briefly
    // keep that descriptor alive. Once a probe runs, no writer can reappear.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let run = Command::new(path)
            .arg(probe)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match run {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            run => {
                assert!(run.unwrap().success(), "the probe run of {path:?} failed");
                return;
            }
        }
    }
}

fn staged_zetta(directory: &Path) -> std::path::PathBuf {
    let binary = directory.join("zetta");
    std::fs::copy(env!("CARGO_BIN_EXE_zetta"), &binary).unwrap();
    wait_until_executable(&binary, "--version");
    binary
}

#[test]
fn copy_proxy_forwards_arguments_and_standard_streams() {
    let directory = tempfile::tempdir().unwrap();
    let zetta = staged_zetta(directory.path());
    let helper = directory.path().join("zcopy");
    std::fs::write(
        &helper,
        "#!/bin/sh\ntest \"$1\" = --zetta-test-probe && exit 0\nprintf 'args:%s,%s\\n' \"$1\" \"$2\"\ncat\nprintf 'helper stderr\\n' >&2\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&helper, permissions).unwrap();
    wait_until_executable(&helper, "--zetta-test-probe");

    let mut child = Command::new(&zetta)
        .args(["copy", "-pboard", "general"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("héllo".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "args:-pboard,general\nhéllo"
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "helper stderr\n");
}

#[test]
fn missing_helper_is_reported_after_validation() {
    let directory = tempfile::tempdir().unwrap();
    let zetta = staged_zetta(directory.path());
    let invalid = Command::new(&zetta)
        .args(["paste", "--unknown"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("unknown paste option"));
    let missing = Command::new(&zetta).arg("paste").output().unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr)
            .contains("install the clipboard helpers beside zetta")
    );
}
