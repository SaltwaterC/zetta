use super::*;
use std::ffi::OsString;

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

#[test]
fn request_parser_accepts_options_and_rejects_malformed_packets() {
    let packet = request_packet(OP_RRQ, "boot/kernel", None);
    let request = parse_request(&packet).unwrap();
    assert!(!request.write);
    assert_eq!(request.filename, "boot/kernel");
    assert_eq!(request.mode, "octet");
    assert_eq!(
        request.options[0],
        ("blksize".to_owned(), "1428".to_owned())
    );

    assert!(parse_request(&[0, OP_RRQ as u8, b'x']).is_err());
}

#[test]
fn option_negotiation_bounds_block_size_and_reports_transfer_size() {
    let options = vec![
        ("blksize".to_owned(), "4096".to_owned()),
        ("tsize".to_owned(), "0".to_owned()),
        ("unknown".to_owned(), "value".to_owned()),
    ];
    assert_eq!(
        negotiated_options(&options, 12345),
        (
            4096,
            vec![
                ("blksize".to_owned(), "4096".to_owned()),
                ("tsize".to_owned(), "12345".to_owned()),
            ]
        )
    );
    assert_eq!(
        negotiated_options(&[("blksize".to_owned(), "7".to_owned())], 1),
        (DEFAULT_BLOCK_SIZE, Vec::new())
    );
}

#[test]
fn write_option_negotiation_preserves_the_client_transfer_size() {
    assert_eq!(
        negotiated_write_options(&[
            ("blksize".to_owned(), "2048".to_owned()),
            ("tsize".to_owned(), "12345".to_owned()),
        ]),
        (
            2048,
            Some(12345),
            vec![
                ("blksize".to_owned(), "2048".to_owned()),
                ("tsize".to_owned(), "12345".to_owned()),
            ]
        )
    );
}

#[test]
fn interrupted_socket_operations_are_retryable() {
    let interrupted = io::Error::from(io::ErrorKind::Interrupted);
    assert!(socket_operation_was_interrupted(&interrupted));
    assert!(!socket_operation_was_interrupted(&io::Error::from(
        io::ErrorKind::TimedOut
    )));
}

#[test]
fn server_log_lines_use_human_readable_utc_timestamps() {
    let timestamp = OffsetDateTime::from_unix_timestamp(0).unwrap();
    assert_eq!(
        format_log_line(
            "GET \"boot.bin\" -> 127.0.0.1:1234 (10 bytes)\r\n",
            timestamp
        ),
        "[1970-01-01 00:00:00 UTC] GET \"boot.bin\" -> 127.0.0.1:1234 (10 bytes)\r\n"
    );
}

#[test]
fn duplicate_active_requests_share_one_transfer() {
    let active_requests = Mutex::new(HashSet::new());
    let request = RequestKey {
        peer: "127.0.0.1:12345".parse().unwrap(),
        write: false,
        filename: "boot.bin".to_owned(),
    };

    assert!(register_active_request(&active_requests, &request));
    assert!(!register_active_request(&active_requests, &request));
    remove_active_request(&active_requests, &request);
    assert!(register_active_request(&active_requests, &request));
}

#[test]
fn server_paths_cannot_escape_the_served_directory() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("inside.bin"), b"inside").unwrap();
    assert_eq!(
        safe_server_path(root.path(), "inside.bin").unwrap(),
        fs::canonicalize(root.path().join("inside.bin")).unwrap()
    );
    assert!(safe_server_path(root.path(), "../outside.bin").is_err());
    assert!(safe_server_path(root.path(), "/outside.bin").is_err());
}

#[test]
fn incomplete_uploads_are_removed_and_completed_uploads_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let partial_path = root.path().join("partial.bin");
    {
        let mut upload = PendingUpload::create(root.path(), "partial.bin").unwrap();
        upload.write_all(b"partial").unwrap();
    }
    assert!(!partial_path.exists());

    let complete_path = root.path().join("complete.bin");
    let mut upload = PendingUpload::create(root.path(), "complete.bin").unwrap();
    upload.write_all(b"complete").unwrap();
    upload.finish().unwrap();
    assert_eq!(fs::read(complete_path).unwrap(), b"complete");
    assert!(PendingUpload::create(root.path(), "complete.bin").is_err());
    assert!(PendingUpload::create(root.path(), "../outside.bin").is_err());
}

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
    fs::write(root.path().join("firmware.bin"), &contents).unwrap();
    let server = start_server(root.path(), 0).unwrap();
    let destination = output.path().join("received.bin");

    download(
        "127.0.0.1",
        server.address.port(),
        "firmware.bin",
        &destination,
    )
    .unwrap();

    assert_eq!(fs::read(destination).unwrap(), contents);
    drop(server);
}

#[test]
fn missing_server_file_returns_an_error() {
    if !localhost_udp_available() {
        eprintln!("skipping TFTP transfer: localhost UDP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let server = start_server(root.path(), 0).unwrap();
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
    fs::write(source.path(), &contents).unwrap();
    let server = start_server(root.path(), 0).unwrap();

    upload(
        "127.0.0.1",
        server.address.port(),
        source.path(),
        "uploaded.bin",
    )
    .unwrap();

    assert_eq!(
        fs::read(root.path().join("uploaded.bin")).unwrap(),
        contents
    );
    drop(server);
}
