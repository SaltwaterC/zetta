//! Exercises the public CLI against the shared server used by Zetta panes.
#![cfg(all(feature = "tftp-client", feature = "tftp-server"))]
use std::process::Command;

#[test]
fn standalone_client_uploads_and_downloads_through_the_pane_server() {
    let root = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let server = ztftp::start_server(root.path(), 0, true).unwrap();
    let source = local.path().join("source.bin");
    let destination = local.path().join("download.bin");
    let contents = vec![42; 5000];
    std::fs::write(&source, &contents).unwrap();
    let port = server.address.port().to_string();
    let upload = Command::new(env!("CARGO_BIN_EXE_ztftp"))
        .args(["put", "--port", &port, "127.0.0.1"])
        .arg(&source)
        .arg("remote.bin")
        .output()
        .unwrap();
    assert!(
        upload.status.success(),
        "{}",
        String::from_utf8_lossy(&upload.stderr)
    );
    assert_eq!(
        std::fs::read(root.path().join("remote.bin")).unwrap(),
        contents
    );
    let download = Command::new(env!("CARGO_BIN_EXE_ztftp"))
        .args(["get", "--port", &port, "127.0.0.1", "remote.bin"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        download.status.success(),
        "{}",
        String::from_utf8_lossy(&download.stderr)
    );
    assert_eq!(std::fs::read(destination).unwrap(), contents);
}

#[test]
fn standalone_server_help_and_errors_have_correct_exit_status() {
    let help = Command::new(env!("CARGO_BIN_EXE_ztftp"))
        .args(["server", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for option in ["--root", "--port", "--writable"] {
        assert!(text.contains(option));
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_ztftp"))
        .args(["server", "--port", "0"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--port"));
}

#[test]
fn standalone_server_serves_files_until_its_process_stops() {
    use std::io::BufRead as _;
    use std::process::Stdio;
    struct ServerProcess(std::process::Child);
    impl Drop for ServerProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    let reservation = std::net::UdpSocket::bind(("0.0.0.0", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port().to_string();
    drop(reservation);
    let mut server = ServerProcess(
        Command::new(env!("CARGO_BIN_EXE_ztftp"))
            .args(["server", "--port", &port, "--root"])
            .arg(root.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut banner = String::new();
    std::io::BufReader::new(server.0.stderr.take().unwrap())
        .read_line(&mut banner)
        .unwrap();
    assert!(banner.contains("read only"), "{banner}");
    let destination = output.path().join("received.bin");
    ztftp::parse_tftp_args([
        std::ffi::OsString::from("get"),
        "--port".into(),
        port.into(),
        "127.0.0.1".into(),
        "firmware.bin".into(),
        destination.as_os_str().to_owned(),
    ])
    .unwrap()
    .run()
    .unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), b"firmware");
}
