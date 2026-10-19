//! Public CLI checks against a real standalone HTTP server process.
use std::io::{BufRead as _, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

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

fn start_server(root: &std::path::Path, options: &[&str]) -> ServerProcess {
    let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let mut child = Command::new(env!("CARGO_BIN_EXE_zhttp"))
        .args(["server", "--port", &port.to_string(), "--root"])
        .arg(root)
        .args(options)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let process = ServerProcess { child, port };
    let mut banner = String::new();
    std::io::BufReader::new(stderr)
        .read_line(&mut banner)
        .unwrap();
    assert!(banner.contains("Serving "), "{banner}");
    process
}

fn request(port: u16, path: &str, host: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn standalone_server_serves_files_and_preserves_access_policy_defaults() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    std::fs::write(root.path().join(".secret"), b"secret").unwrap();
    let server = start_server(root.path(), &[]);
    let response = request(server.port, "/firmware.bin", "localhost");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("firmware"));
    assert!(!request(server.port, "/.secret", "localhost").contains("secret"));
    assert!(request(server.port, "/firmware.bin", "other.example").starts_with("HTTP/1.1 421"));
}

#[test]
fn standalone_server_applies_dotfile_and_allowed_host_options() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".secret"), b"secret").unwrap();
    let server = start_server(root.path(), &["-d", "-a", "Builds.LAN."]);
    let response = request(server.port, "/.secret", "builds.lan");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("secret"));
}

#[test]
fn standalone_help_and_invalid_port_use_correct_exit_status() {
    let help = Command::new(env!("CARGO_BIN_EXE_zhttp"))
        .args(["server", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for option in [
        "--root",
        "--port",
        "--serve-dotfiles",
        "--allow-host",
        "--allow-any-host",
    ] {
        assert!(text.contains(option));
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_zhttp"))
        .args(["server", "--port", "0"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--port"));
}
