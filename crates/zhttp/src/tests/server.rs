use super::*;
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown};

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
fn request_parsing_reads_host_and_ignores_opaque_header_values() {
    let request = parse_request_head(
        b"GET /.local/ HTTP/1.1\r\nhOsT: localhost:8000 \r\nX-Opaque: \xff\xfe\r\n",
    )
    .unwrap();
    assert_eq!(
        request,
        HttpRequest {
            method: "GET".to_owned(),
            target: "/.local/".to_owned(),
            host: Some("localhost:8000".to_owned()),
        }
    );
    assert!(parse_request_head(b"GET /.local/\r\n").is_err());

    // HTTP/1.0 bootloaders send no Host at all.
    assert_eq!(
        parse_request_head(b"GET /firmware.bin HTTP/1.0")
            .unwrap()
            .host,
        None
    );
    // Two Host headers are ambiguous about which site is meant (RFC 9112).
    assert!(
        parse_request_head(b"GET / HTTP/1.1\r\nHost: localhost\r\nHost: evil.example").is_err()
    );
}

#[test]
fn request_paths_cannot_escape_the_served_directory() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    fs::write(root.path().join("index.html"), b"index").unwrap();
    let root = fs::canonicalize(root.path()).unwrap();

    assert_eq!(
        resolve_request_path(&root, "/firmware.bin?download=1", false).unwrap(),
        ResolvedContent::File(root.join("firmware.bin"))
    );
    assert_eq!(
        resolve_request_path(&root, "/", false).unwrap(),
        ResolvedContent::File(root.join("index.html"))
    );
    assert_eq!(
        resolve_request_path(&root, "/../outside.bin", false),
        Err(ResolveError::Forbidden)
    );
    assert_eq!(
        resolve_request_path(&root, "/%2e%2e/outside.bin", false),
        Err(ResolveError::Forbidden)
    );
}

#[test]
fn dot_prefixed_request_paths_are_refused_unless_enabled() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".git/config"), b"[remote]").unwrap();
    fs::create_dir_all(root.path().join("build")).unwrap();
    fs::write(root.path().join("build/.env"), b"TOKEN=1").unwrap();
    let root = fs::canonicalize(root.path()).unwrap();

    for target in [
        "/.git/config",
        "/%2egit/config",
        "/build/.env",
        // Refused before the filesystem is consulted, so a refusal does not
        // reveal whether the name exists.
        "/.missing",
    ] {
        assert_eq!(
            resolve_request_path(&root, target, false),
            Err(ResolveError::DotPrefixed),
            "{target}"
        );
    }
    assert_eq!(
        resolve_request_path(&root, "/.git/config", true).unwrap(),
        ResolvedContent::File(root.join(".git/config"))
    );
    assert_eq!(
        resolve_request_path(&root, "/build/.env", true).unwrap(),
        ResolvedContent::File(root.join("build/.env"))
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_cannot_reach_a_dot_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    fs::create_dir(root.join(".config")).unwrap();
    fs::write(root.join(".config/settings"), b"secret").unwrap();
    std::os::unix::fs::symlink(root.join(".config"), root.join("cfg")).unwrap();
    fs::create_dir(root.join("site")).unwrap();
    fs::write(root.join(".secret.html"), b"secret").unwrap();
    std::os::unix::fs::symlink(root.join(".secret.html"), root.join("site/index.html")).unwrap();

    // Neither request names a dot, but each resolves through one.
    assert_eq!(
        resolve_request_path(&root, "/cfg/settings", false),
        Err(ResolveError::DotPrefixed)
    );
    assert_eq!(
        resolve_request_path(&root, "/cfg/", false),
        Err(ResolveError::DotPrefixed)
    );
    assert_eq!(
        resolve_request_path(&root, "/site/", false),
        Err(ResolveError::DotPrefixed)
    );
    assert_eq!(
        resolve_request_path(&root, "/cfg/settings", true).unwrap(),
        ResolvedContent::File(root.join(".config/settings"))
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
        resolve_request_path(&canonical_root, "/", false).unwrap(),
        ResolvedContent::Directory(canonical_root)
    );
    let index = directory_index(root.path(), "/", false).unwrap();
    assert!(index.contains("href=\"/firmware%20image.bin\""));
    assert!(index.contains("notes &amp; &#39;ideas&#39;.txt"));
    assert!(!index.contains(">notes & 'ideas'.txt<"));
    assert!(index.lines().any(|line| {
        line == "<li><a href=\"/firmware%20image.bin\">firmware image.bin</a></li>"
    }));
}

#[test]
fn directory_indexes_hide_dot_entries_unless_enabled() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".env"), b"TOKEN=1").unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();

    let refused = directory_index(root.path(), "/", false).unwrap();
    assert!(refused.contains("href=\"/firmware.bin\""));
    assert!(!refused.contains(".git"));
    assert!(!refused.contains(".env"));

    let served = directory_index(root.path(), "/", true).unwrap();
    assert!(served.contains("href=\"/.git/\""));
    assert!(served.contains("href=\"/.env\""));
}

#[test]
fn host_check_answers_this_machine_and_refuses_other_names() {
    let local = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
    let policy = HostPolicy::new(
        &HttpServerOptions {
            allowed_hosts: vec!["builds.lan".to_owned()],
            ..HttpServerOptions::default()
        },
        Some("Workstation.example.com"),
    );
    for host in [
        None,
        Some(""),
        Some("192.168.1.20"),
        Some("192.168.1.20:8000"),
        Some("127.0.0.1:8000"),
        Some("127.1.2.3"),
        Some("[::1]:8000"),
        Some("localhost"),
        Some("LOCALHOST.:8000"),
        Some("workstation"),
        Some("workstation.local"),
        Some("workstation.example.com."),
        Some("Builds.LAN:8000"),
    ] {
        assert!(policy.allows(host, local), "{host:?} should be answered");
    }
    for host in [
        "evil.example",
        // An address of some other machine is a rebinding target as much as a
        // name is.
        "192.168.1.21",
        "localhost.evil.example",
        "workstation.evil.example",
        "::1",
        "[::1",
        "localhost:http",
        "local host",
    ] {
        assert!(
            !policy.allows(Some(host), local),
            "{host:?} should be refused"
        );
    }

    // The address the connection arrived on is this machine's whatever its
    // family, including IPv4 carried in IPv6.
    let mapped = IpAddr::V6(Ipv4Addr::new(192, 168, 1, 20).to_ipv6_mapped());
    assert!(policy.allows(Some("192.168.1.20"), mapped));
    let link_local = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
    assert!(policy.allows(Some("[FE80::0001]:8000"), link_local));

    let any = HostPolicy::new(
        &HttpServerOptions {
            allow_any_host: true,
            ..HttpServerOptions::default()
        },
        None,
    );
    assert!(any.allows(Some("evil.example"), local));
}

#[test]
fn allowed_host_names_normalise_like_request_hosts() {
    assert_eq!(
        normalize_allowed_host("Builds.LAN.").as_deref(),
        Ok("builds.lan")
    );
    assert_eq!(
        normalize_allowed_host("10.0.0.5").as_deref(),
        Ok("10.0.0.5")
    );
    assert_eq!(
        normalize_allowed_host("[FE80::0001]").as_deref(),
        Ok("fe80::1")
    );
    assert_eq!(normalize_allowed_host("fe80::1").as_deref(), Ok("fe80::1"));
    assert!(normalize_allowed_host("builds.lan:8000").is_err());
    assert!(normalize_allowed_host("").is_err());
    assert!(normalize_allowed_host("builds lan").is_err());
    assert!(normalize_allowed_host("builds.lan/firmware").is_err());
}

#[test]
fn request_slots_limit_each_peer_and_the_whole_server() {
    let first = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    let second = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
    let third = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
    let slots = RequestSlots::new(3, 2);

    let a = slots.acquire(first).unwrap();
    let b = slots.acquire(first).unwrap();
    assert_eq!(slots.acquire(first).err(), Some(SlotRefusal::PeerBusy));
    // One peer at its limit leaves the rest of the server to others.
    let c = slots.acquire(second).unwrap();
    assert_eq!(slots.acquire(third).err(), Some(SlotRefusal::ServerBusy));

    drop(a);
    let a = slots.acquire(first).unwrap();
    drop((a, b, c));
    let counts = slots.in_use.lock().unwrap();
    assert_eq!(counts.total, 0);
    assert!(counts.per_peer.is_empty());
}

#[test]
fn response_allowance_adds_a_floor_rate_to_the_grace() {
    let limits = ServerLimits::DEFAULT;
    assert_eq!(response_allowance(&limits, 0), RESPONSE_GRACE);
    assert_eq!(
        response_allowance(&limits, 16 * 1024),
        RESPONSE_GRACE + Duration::from_secs(1)
    );
    // A 64 MiB image gets over an hour on top of the grace.
    assert_eq!(
        response_allowance(&limits, 64 * 1024 * 1024),
        RESPONSE_GRACE + Duration::from_secs(4096)
    );
    // Saturates rather than overflowing.
    assert_eq!(
        response_allowance(&limits, u64::MAX),
        RESPONSE_GRACE + Duration::from_nanos(u64::MAX)
    );
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

fn start_test_server(
    root: &Path,
    options: HttpServerOptions,
    limits: ServerLimits,
) -> OpenHttpServer {
    start_http_server_with_limits(root, 0, options, limits).unwrap()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn idle_nonblocking_connections_are_not_malformed_requests() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP connection test: localhost TCP is unavailable");
        return;
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    server.set_nonblocking(true).unwrap();

    assert_eq!(
        read_request(&server, Instant::now() + HEADER_DEADLINE, CLIENT_TIMEOUT),
        Ok(None)
    );
}

#[test]
fn server_serves_get_and_head_requests() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP transfer: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware contents").unwrap();
    let server = start_http_server(root.path(), 0, HttpServerOptions::default()).unwrap();

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
    let server = start_http_server(root.path(), 0, HttpServerOptions::default()).unwrap();

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
fn server_refuses_dotfiles_unless_started_with_serve_dotfiles() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP transfer: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".env"), b"TOKEN=1").unwrap();
    let request = b"GET /.env HTTP/1.1\r\nHost: localhost\r\n\r\n";

    let refusing = start_http_server(root.path(), 0, HttpServerOptions::default()).unwrap();
    let refused = send_request(refusing.address, request).unwrap();
    assert!(refused.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));
    assert!(!contains(&refused, "TOKEN"));

    let serving = start_http_server(
        root.path(),
        0,
        HttpServerOptions {
            serve_dotfiles: true,
            ..HttpServerOptions::default()
        },
    )
    .unwrap();
    let served = send_request(serving.address, request).unwrap();
    assert!(served.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(served.ends_with(b"TOKEN=1"));
}

/// A DNS-rebinding page reaches the server under its own name, so its requests
/// carry that name in `Host`; the server must answer them with 421 and say
/// which flag admits a name the user does want.
#[test]
fn server_refuses_hosts_that_are_not_this_machine() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP transfer: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    let foreign = b"GET /firmware.bin HTTP/1.1\r\nHost: evil.example:8000\r\n\r\n";

    let server = start_http_server(root.path(), 0, HttpServerOptions::default()).unwrap();
    let refused = send_request(server.address, foreign).unwrap();
    assert!(refused.starts_with(b"HTTP/1.1 421 Misdirected Request\r\n"));
    assert!(contains(&refused, "--allow-host evil.example"));
    assert!(!refused.ends_with(b"firmware"));

    let head = send_request(
        server.address,
        b"HEAD / HTTP/1.1\r\nHost: evil.example\r\n\r\n",
    )
    .unwrap();
    assert!(head.starts_with(b"HTTP/1.1 421 Misdirected Request\r\n"));
    assert!(head.ends_with(b"\r\n\r\n"));

    // HTTP/1.0 bootloaders send no Host, and the address the request arrived
    // on is this machine.
    let bootloader = send_request(server.address, b"GET /firmware.bin HTTP/1.0\r\n\r\n").unwrap();
    assert!(bootloader.ends_with(b"\r\n\r\nfirmware"));
    let by_address = send_request(
        server.address,
        format!(
            "GET /firmware.bin HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            server.address.port()
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(by_address.ends_with(b"\r\n\r\nfirmware"));

    for options in [
        HttpServerOptions {
            allowed_hosts: vec!["evil.example".to_owned()],
            ..HttpServerOptions::default()
        },
        HttpServerOptions {
            allow_any_host: true,
            ..HttpServerOptions::default()
        },
    ] {
        let admitting = start_http_server(root.path(), 0, options).unwrap();
        let admitted = send_request(admitting.address, foreign).unwrap();
        assert!(admitted.ends_with(b"\r\n\r\nfirmware"));
    }
}

/// Short limits for the socket tests, so a deadline can pass in milliseconds.
fn test_limits() -> ServerLimits {
    ServerLimits {
        client_timeout: Duration::from_secs(10),
        header_deadline: Duration::from_secs(10),
        response_grace: Duration::from_millis(500),
        response_min_bytes_per_second: 64 * 1024 * 1024,
        max_requests: 32,
        max_requests_per_peer: 8,
    }
}

/// Reads until the server closes the connection, or `timeout` passes; returns
/// what arrived and whether the server closed (a reset counts as closing).
fn read_until_closed(stream: &mut TcpStream, timeout: Duration) -> (Vec<u8>, bool) {
    stream.set_read_timeout(Some(timeout)).unwrap();
    let mut response = Vec::new();
    let closed = match stream.read_to_end(&mut response) {
        Ok(_) => true,
        Err(error) => !matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ),
    };
    (response, closed)
}

/// A peer sending its head one byte at a time never trips the per-read
/// timeout; the head deadline is what releases its slot.
#[test]
fn a_request_head_must_arrive_before_the_header_deadline() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP deadline test: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut server = start_test_server(
        root.path(),
        HttpServerOptions::default(),
        ServerLimits {
            client_timeout: Duration::from_secs(2),
            header_deadline: Duration::from_millis(300),
            ..test_limits()
        },
    );
    let mut stream = TcpStream::connect(("127.0.0.1", server.address.port())).unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    let mut trickle = stream.try_clone().unwrap();
    let trickler = thread::spawn(move || {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(4) {
            if trickle.write_all(b"X").is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    let started = Instant::now();
    let (response, closed) = read_until_closed(&mut stream, Duration::from_millis(2500));
    let elapsed = started.elapsed();
    assert!(
        closed,
        "the server was still waiting for the head after {elapsed:?}"
    );
    assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
    // Closing with unread trickle bytes can reset TCP and discard any suffix
    // of the response, including part of the status line.
    let status = b"HTTP/1.1 408 Request Timeout\r\n";
    assert!(
        status.starts_with(&response) || response.starts_with(status),
        "unexpected response: {:?}",
        String::from_utf8_lossy(&response)
    );
    let _ = stream.shutdown(Shutdown::Both);
    trickler.join().unwrap();

    // The rejection is logged before the response is written, so verify the
    // timeout even when the reset discarded all of the response bytes.
    let mut logs = Vec::new();
    let mut buffer = [0; 1024];
    while !contains(&logs, "the request head did not arrive in time") {
        let count = server.reader.read(&mut buffer).unwrap();
        assert_ne!(
            count, 0,
            "server closed its log before recording the timeout"
        );
        logs.extend_from_slice(&buffer[..count]);
    }
}

/// A peer that stops reading holds its slot for at most the response deadline,
/// not for as long as it keeps the socket open.
#[test]
fn a_response_read_too_slowly_is_cut_off() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP deadline test: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    // Sparse, so the size costs no disk; far beyond what socket buffers hold.
    let size = 64 * 1024 * 1024;
    File::create(root.path().join("image.bin"))
        .unwrap()
        .set_len(size)
        .unwrap();
    fs::write(root.path().join("small.bin"), vec![7; 4 * 1024 * 1024]).unwrap();
    let server = start_test_server(root.path(), HttpServerOptions::default(), test_limits());

    // A prompt reader gets the whole file under the same limits.
    let mut prompt = TcpStream::connect(("127.0.0.1", server.address.port())).unwrap();
    prompt
        .write_all(b"GET /small.bin HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let (response, closed) = read_until_closed(&mut prompt, Duration::from_secs(5));
    assert!(closed);
    assert!(response.ends_with(&[7; 1024]));
    assert!(contains(&response, "Content-Length: 4194304\r\n"));
    assert!(response.len() > 4 * 1024 * 1024);

    // The whole image is due 0.5 s + 1 s after the response starts; this
    // reader starts reading only after that.
    let mut stalled = TcpStream::connect(("127.0.0.1", server.address.port())).unwrap();
    stalled
        .write_all(b"GET /image.bin HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(2500));
    let (response, closed) = read_until_closed(&mut stalled, Duration::from_secs(5));
    assert!(closed);
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(
        (response.len() as u64) < size,
        "the stalled reader still received all {} bytes",
        response.len()
    );
}

/// One LAN peer opening idle connections takes at most its own share of the
/// request slots, so a flash from another device is not stalled behind it.
#[test]
fn one_peer_cannot_take_every_request_slot() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP slot test: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("firmware.bin"), b"firmware").unwrap();
    let server = start_test_server(
        root.path(),
        HttpServerOptions::default(),
        ServerLimits {
            max_requests: 4,
            max_requests_per_peer: 2,
            ..test_limits()
        },
    );
    let request = b"GET /firmware.bin HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let idle = (0..2)
        .map(|_| TcpStream::connect(("127.0.0.1", server.address.port())).unwrap())
        .collect::<Vec<_>>();

    // The refusal is written without reading the request, so this connection
    // sends nothing: closing over unread bytes would reset it and could
    // discard the response before the test reads it.
    let mut third = TcpStream::connect(("127.0.0.1", server.address.port())).unwrap();
    let (refused, closed) = read_until_closed(&mut third, Duration::from_secs(5));
    assert!(closed);
    assert!(
        refused.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"),
        "{}",
        String::from_utf8_lossy(&refused)
    );

    // Closing them returns the peer's slots.
    drop(idle);
    let served = (0..50).any(|_| {
        let response = send_request(server.address, request).unwrap_or_default();
        if response.ends_with(b"\r\n\r\nfirmware") {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
        false
    });
    assert!(served, "the peer's slots were not released");
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
    // Sixteen at once from one address is more than a peer's share by default.
    let server = start_test_server(
        root.path(),
        HttpServerOptions {
            serve_dotfiles: true,
            ..HttpServerOptions::default()
        },
        ServerLimits {
            max_requests_per_peer: 16,
            ..ServerLimits::DEFAULT
        },
    );

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
        assert!(contains(&response, "href=\"/.local/firmware.bin\""));
    }
}

/// The accept loop blocks instead of polling a shutdown flag, so dropping the
/// control writer must actively unblock it. Without the loopback wakeup the
/// worker would stay parked in `accept` until the next real connection.
#[test]
fn dropping_the_control_writer_stops_the_accept_loop() {
    if !localhost_tcp_available() {
        eprintln!("skipping HTTP shutdown test: localhost TCP is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let server = start_http_server(root.path(), 0, HttpServerOptions::default()).unwrap();
    let address = server.address;

    // Confirm the worker is accepting before shutting it down.
    assert!(send_request(address, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").is_ok());

    drop(server);

    // Once the worker has exited, nothing holds the listening socket, so the
    // port becomes bindable again. Poll briefly to absorb worker teardown.
    let released = (0..100).any(|_| {
        if TcpListener::bind(("0.0.0.0", address.port())).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        false
    });
    assert!(
        released,
        "HTTP server worker did not release {address} after shutdown"
    );
}
