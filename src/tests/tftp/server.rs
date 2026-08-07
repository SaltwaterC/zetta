use super::*;
use crate::tftp::request_packet;

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
    let tempdir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tempdir.path()).unwrap();
    fs::write(root.join("inside.bin"), b"inside").unwrap();
    assert_eq!(
        safe_server_path(&root, "inside.bin").unwrap(),
        fs::canonicalize(root.join("inside.bin")).unwrap()
    );
    assert!(safe_server_path(&root, "../outside.bin").is_err());
    assert!(safe_server_path(&root, "/outside.bin").is_err());
}

#[test]
fn incomplete_uploads_are_removed_and_completed_uploads_are_preserved() {
    let tempdir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tempdir.path()).unwrap();
    let partial_path = root.join("partial.bin");
    {
        let mut upload = PendingUpload::create(&root, "partial.bin").unwrap();
        upload.write_all(b"partial").unwrap();
    }
    assert!(!partial_path.exists());

    let complete_path = root.join("complete.bin");
    let mut upload = PendingUpload::create(&root, "complete.bin").unwrap();
    upload.write_all(b"complete").unwrap();
    upload.finish().unwrap();
    assert_eq!(fs::read(complete_path).unwrap(), b"complete");
    assert!(PendingUpload::create(&root, "complete.bin").is_err());
    assert!(PendingUpload::create(&root, "../outside.bin").is_err());
}
