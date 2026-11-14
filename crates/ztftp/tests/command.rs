//! Exercises the public CLI against the shared server used by Zetta panes.
#![cfg(all(feature = "tftp-client", feature = "tftp-server"))]
use std::io::BufRead as _;
use std::net::UdpSocket;
use std::process::{Command, Stdio};

struct ServerProcess {
    child: std::process::Child,
    port: u16,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn available_port() -> u16 {
    // Match the server's wildcard bind, including ports held on loopback.
    UdpSocket::bind(("0.0.0.0", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The CLI refuses port zero, so another socket can claim a candidate between
/// releasing it here and the child binding it. Retry bind failures on fresh
/// ports, with a limit so a persistent failure still fails the test.
fn start_server(root: &std::path::Path, ports: impl IntoIterator<Item = u16>) -> ServerProcess {
    const PORT_ATTEMPTS: usize = 20;
    let mut last_banner = String::new();
    for port in ports.into_iter().take(PORT_ATTEMPTS) {
        let mut process = ServerProcess {
            child: Command::new(env!("CARGO_BIN_EXE_ztftp"))
                .args(["server", "--port", &port.to_string(), "--root"])
                .arg(root)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            port,
        };
        let mut banner = String::new();
        std::io::BufReader::new(process.child.stderr.take().unwrap())
            .read_line(&mut banner)
            .unwrap();
        if banner.starts_with("Serving ") {
            assert!(banner.contains("read only"), "{banner}");
            return process;
        }
        // Use our context rather than OS error text, which varies by platform.
        assert!(
            banner.starts_with("ztftp: binding TFTP server to UDP port "),
            "{banner}"
        );
        last_banner = banner;
    }
    panic!("no free port after {PORT_ATTEMPTS} attempts: {last_banner}");
}

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
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    let server = start_server(root.path(), std::iter::repeat_with(available_port));
    let destination = output.path().join("received.bin");
    ztftp::parse_tftp_args([
        std::ffi::OsString::from("get"),
        "--port".into(),
        server.port.to_string().into(),
        "127.0.0.1".into(),
        "firmware.bin".into(),
        destination.as_os_str().to_owned(),
    ])
    .unwrap()
    .run()
    .unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), b"firmware");
}

#[test]
fn standalone_server_retries_an_occupied_port() {
    let root = tempfile::tempdir().unwrap();
    let occupied = UdpSocket::bind(("0.0.0.0", 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    let ports = std::iter::once(port).chain(std::iter::repeat_with(available_port));
    let server = start_server(root.path(), ports);
    assert_ne!(server.port, port);
}
