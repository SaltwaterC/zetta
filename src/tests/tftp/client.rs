use std::ffi::OsString;
#[cfg(feature = "tftp-server")]
use std::time::Duration;

use super::*;

#[cfg(feature = "tftp-server")]
fn localhost_udp_available() -> bool {
    let Ok(receiver) = UdpSocket::bind(("127.0.0.1", 0)) else {
        return false;
    };
    let Ok(sender) = UdpSocket::bind(("127.0.0.1", 0)) else {
        return false;
    };
    if receiver
        .set_read_timeout(Some(Duration::from_millis(100)))
        .is_err()
        || sender
            .send_to(b"probe", receiver.local_addr().unwrap())
            .is_err()
    {
        return false;
    }
    let mut buffer = [0; 5];
    receiver.recv(&mut buffer).is_ok()
}

#[test]
fn parses_get_and_put_commands_with_defaults() {
    assert_eq!(
        parse_tftp_args([
            OsString::from("get"),
            OsString::from("192.0.2.1"),
            OsString::from("images/boot.bin"),
        ])
        .unwrap(),
        TftpCommand::Get {
            host: "192.0.2.1".to_owned(),
            remote: "images/boot.bin".to_owned(),
            local: PathBuf::from("boot.bin"),
            port: 69,
        }
    );
    assert_eq!(
        parse_tftp_args([
            OsString::from("put"),
            OsString::from("--port"),
            OsString::from("1069"),
            OsString::from("localhost"),
            OsString::from("local.bin"),
            OsString::from("remote.bin"),
        ])
        .unwrap(),
        TftpCommand::Put {
            host: "localhost".to_owned(),
            local: PathBuf::from("local.bin"),
            remote: "remote.bin".to_owned(),
            port: 1069,
        }
    );
}

#[cfg(feature = "tftp-server")]
#[test]
fn client_downloads_from_the_embedded_server() {
    if !localhost_udp_available() {
        eprintln!("skipping TFTP transfer: localhost UDP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let contents = (0..5000)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(root.path().join("firmware.bin"), &contents).unwrap();
    let server = crate::tftp::start_server(root.path(), 0, false).unwrap();
    let destination = output.path().join("received.bin");

    download(
        "127.0.0.1",
        server.address.port(),
        "firmware.bin",
        &destination,
    )
    .unwrap();

    assert_eq!(std::fs::read(destination).unwrap(), contents);
    drop(server);
}

#[cfg(feature = "tftp-server")]
#[test]
fn missing_server_file_returns_an_error() {
    if !localhost_udp_available() {
        eprintln!("skipping TFTP transfer: localhost UDP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let server = crate::tftp::start_server(root.path(), 0, false).unwrap();
    let error = download(
        "127.0.0.1",
        server.address.port(),
        "missing.bin",
        &output.path().join("missing.bin"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("TFTP server error"));
    drop(server);
}

#[cfg(feature = "tftp-server")]
#[test]
fn client_uploads_to_the_embedded_server() {
    if !localhost_udp_available() {
        eprintln!("skipping TFTP transfer: localhost UDP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::NamedTempFile::new().unwrap();
    let contents = (0..5000)
        .map(|index| (index % 239) as u8)
        .collect::<Vec<_>>();
    std::fs::write(source.path(), &contents).unwrap();
    let server = crate::tftp::start_server(root.path(), 0, true).unwrap();

    upload(
        "127.0.0.1",
        server.address.port(),
        source.path(),
        "uploaded.bin",
    )
    .unwrap();

    assert_eq!(
        std::fs::read(root.path().join("uploaded.bin")).unwrap(),
        contents
    );
    drop(server);
}
