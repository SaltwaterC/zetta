use super::*;
use std::net::Shutdown;

#[test]
fn percent_decoding_handles_urls_and_rejects_invalid_input() {
    assert_eq!(
        percent_decode("/firmware%20image.bin").as_deref(),
        Some("/firmware image.bin")
    );
    assert_eq!(
        percent_decode("/%66irmware.bin").as_deref(),
        Some("/firmware.bin")
    );
    assert!(percent_decode("/%xx").is_none());
    assert!(percent_decode("/%00").is_none());
}

#[test]
fn request_parsing_ignores_opaque_header_values() {
    let request =
        parse_request_line(b"GET /.local/ HTTP/1.1\r\nHost: localhost\r\nX-Opaque: \xff\xfe\r\n")
            .unwrap();
    assert_eq!(
        request,
        HttpRequest {
            method: "GET".to_owned(),
            target: "/.local/".to_owned(),
        }
    );
    assert!(parse_request_line(b"GET /.local/\r\n").is_err());
}

#[test]
fn request_paths_cannot_escape_the_served_directory() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    fs::write(root.path().join("index.html"), b"index").unwrap();
    let root = fs::canonicalize(root.path()).unwrap();

    assert_eq!(
        resolve_request_path(&root, "/firmware.bin?download=1").unwrap(),
        ResolvedContent::File(root.join("firmware.bin"))
    );
    assert_eq!(
        resolve_request_path(&root, "/").unwrap(),
        ResolvedContent::File(root.join("index.html"))
    );
    assert_eq!(
        resolve_request_path(&root, "/../outside.bin"),
        Err(ResolveError::Forbidden)
    );
    assert_eq!(
        resolve_request_path(&root, "/%2e%2e/outside.bin"),
        Err(ResolveError::Forbidden)
    );
}

#[test]
fn directory_indexes_escape_labels_and_encode_links() {
    assert_eq!(html_escape("<notes>"), "&lt;notes&gt;");

    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware image.bin"), b"firmware").unwrap();
    fs::write(root.path().join("notes & 'ideas'.txt"), b"notes").unwrap();
    let canonical_root = fs::canonicalize(root.path()).unwrap();

    assert_eq!(
        resolve_request_path(&canonical_root, "/").unwrap(),
        ResolvedContent::Directory(canonical_root)
    );
    let index = directory_index(root.path(), "/").unwrap();
    assert!(index.contains("href=\"/firmware%20image.bin\""));
    assert!(index.contains("notes &amp; &#39;ideas&#39;.txt"));
    assert!(!index.contains(">notes & 'ideas'.txt<"));
    assert!(index.lines().any(|line| {
        line == "<li><a href=\"/firmware%20image.bin\">firmware image.bin</a></li>"
    }));
}

fn send_request(address: SocketAddr, request: &[u8]) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(("127.0.0.1", address.port()))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(request)?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(response)
}

fn localhost_tcp_available() -> bool {
    TcpListener::bind(("127.0.0.1", 0)).is_ok()
}

#[test]
fn idle_nonblocking_connections_are_not_malformed_requests() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP connection test: localhost TCP is unavailable");
        return;
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    server.set_nonblocking(true).unwrap();

    assert_eq!(read_request(&mut server), Ok(None));
}

#[test]
fn server_serves_get_and_head_requests() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP transfer: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware contents").unwrap();
    let server = start_http_server(root.path(), 0).unwrap();

    let get = send_request(
        server.address,
        b"GET /firmware.bin HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    assert!(get.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(get.ends_with(b"\r\n\r\nfirmware contents"));

    let head = send_request(
        server.address,
        b"HEAD /firmware.bin HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    assert!(head.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(head.ends_with(b"\r\n\r\n"));
    assert!(
        std::str::from_utf8(&head)
            .unwrap()
            .contains("Content-Length: 17\r\n")
    );

    let index = send_request(server.address, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    assert!(index.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let index = std::str::from_utf8(&index).unwrap();
    assert!(index.contains("<h1>Index of /</h1>"));
    assert!(index.contains("href=\"/firmware.bin\""));
}

#[test]
fn server_rejects_traversal_and_unsupported_methods() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP transfer: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let server = start_http_server(root.path(), 0).unwrap();

    let traversal = send_request(
        server.address,
        b"GET /%2e%2e/secret HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    assert!(traversal.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));

    let post = send_request(
        server.address,
        b"POST /firmware.bin HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    assert!(post.starts_with(b"HTTP/1.1 405 Method Not Allowed\r\n"));
    assert!(
        std::str::from_utf8(&post)
            .unwrap()
            .contains("Allow: GET, HEAD\r\n")
    );
}

#[test]
fn concurrent_directory_requests_are_consistent() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP transfer: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".local")).unwrap();
    fs::write(root.path().join(".local/firmware.bin"), b"firmware").unwrap();
    let server = start_http_server(root.path(), 0).unwrap();

    let requests = (0..16)
        .map(|_| {
            let address = server.address;
            thread::spawn(move || {
                send_request(address, b"GET /.local/ HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap()
            })
        })
        .collect::<Vec<_>>();
    for request in requests {
        let response = request.join().unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(
            response
                .windows(b"href=\"/.local/firmware.bin\"".len())
                .any(|window| window == b"href=\"/.local/firmware.bin\"")
        );
    }
}
