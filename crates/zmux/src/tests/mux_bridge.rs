use super::*;

use std::{
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::atomic::AtomicUsize,
    time::Instant,
};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A stand-in daemon. The first byte of each connection picks what it does:
/// `E` echoes until EOF, `F` floods `FLOOD_BYTES` and then closes.
struct FakeDaemon {
    _directory: tempfile::TempDir,
    socket: PathBuf,
    connections: Arc<AtomicUsize>,
    /// Bytes a flood has managed to write so far.
    flooded: Arc<AtomicUsize>,
}

const FLOOD_BYTES: usize = 4 * 1024 * 1024;

impl FakeDaemon {
    fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = connections.clone();
        let flooded = Arc::new(AtomicUsize::new(0));
        let flood_counter = flooded.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                counter.fetch_add(1, Ordering::SeqCst);
                let flood_counter = flood_counter.clone();
                thread::spawn(move || {
                    let mut mode = [0];
                    if stream.read_exact(&mut mode).is_err() {
                        return;
                    }
                    match mode[0] {
                        b'E' => {
                            let mut received = Vec::new();
                            let _ = stream.read_to_end(&mut received);
                            let _ = stream.write_all(&received);
                        }
                        b'F' => {
                            let chunk = vec![b'x'; 64 * 1024];
                            for _ in 0..FLOOD_BYTES / chunk.len() {
                                if stream.write_all(&chunk).is_err() {
                                    return;
                                }
                                flood_counter.fetch_add(chunk.len(), Ordering::SeqCst);
                            }
                        }
                        _ => {}
                    }
                });
            }
        });
        Self {
            _directory: directory,
            socket,
            connections,
            flooded,
        }
    }
}

/// Both halves of a bridge over an in-process link, with the far side
/// connecting each stream to `daemon`.
fn bridge_to(daemon: &FakeDaemon) -> (MuxBridge, thread::JoinHandle<Result<()>>) {
    let (near, far) = UnixStream::pair().unwrap();
    let socket = daemon.socket.clone();
    let far_reader = far.try_clone().unwrap();
    let server = thread::spawn(move || {
        serve(
            far_reader,
            far,
            move || UnixStream::connect(&socket),
            || BridgeInfo {
                program: Some(PathBuf::from("/opt/zmux/bin/zmux")),
                endpoint: None,
                error: Some("no multiplexer is running".to_owned()),
            },
        )
    });
    let bridge = MuxBridge::connect(near.try_clone().unwrap(), near, TIMEOUT).unwrap();
    (bridge, server)
}

fn echo(bridge: &MuxBridge, payload: &[u8]) -> Vec<u8> {
    let mut stream = bridge.open().unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.write_all(b"E").unwrap();
    stream.write_all(payload).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).unwrap();
    echoed
}

#[test]
fn frames_round_trip_and_reject_what_is_not_a_frame() {
    let bytes = encode_frame(7, Kind::Data, b"payload");
    let frame = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
    assert_eq!(frame.stream, 7);
    assert_eq!(frame.kind, Kind::Data);
    assert_eq!(frame.payload, b"payload");

    assert!(read_frame(&mut [].as_slice()).unwrap().is_none());
    assert!(read_frame(&mut [0, 0, 0].as_slice()).is_err());
    let mut unknown = encode_frame(1, Kind::Data, b"");
    unknown[4] = 99;
    assert!(read_frame(&mut unknown.as_slice()).is_err());
    let mut oversized = encode_frame(1, Kind::Data, b"");
    oversized[5..9].copy_from_slice(&(MAX_FRAME + 1).to_be_bytes());
    assert!(read_frame(&mut oversized.as_slice()).is_err());
}

#[test]
fn the_greeting_carries_the_far_sides_info_and_it_can_be_asked_again() {
    let daemon = FakeDaemon::start();
    let (bridge, _server) = bridge_to(&daemon);

    let initial = bridge.initial_info();
    assert_eq!(
        initial.program.as_deref(),
        Some(Path::new("/opt/zmux/bin/zmux"))
    );
    assert!(initial.endpoint.is_none());
    for _ in 0..3 {
        assert_eq!(bridge.query_info(TIMEOUT).unwrap(), *initial);
    }
    assert_eq!(
        daemon.connections.load(Ordering::SeqCst),
        0,
        "asking for info must not open a daemon connection"
    );
}

#[test]
fn streams_are_independent_daemon_connections_with_half_close() {
    let daemon = FakeDaemon::start();
    let (bridge, _server) = bridge_to(&daemon);

    let bridge = Arc::new(bridge);
    let workers = (0..8)
        .map(|index| {
            let bridge = bridge.clone();
            thread::spawn(move || {
                let payload = format!("stream {index} \0\u{ff} bytes").into_bytes();
                assert_eq!(echo(&bridge, &payload), payload);
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(daemon.connections.load(Ordering::SeqCst), 8);
}

#[test]
fn a_transfer_larger_than_the_window_arrives_whole() {
    let daemon = FakeDaemon::start();
    let (bridge, _server) = bridge_to(&daemon);

    let payload = (0..3 * WINDOW as usize + 12_345)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let mut stream = bridge.open().unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let sent = payload.clone();
    let sender = thread::spawn(move || {
        writer.write_all(b"E").unwrap();
        writer.write_all(&sent).unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
    });
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).unwrap();
    sender.join().unwrap();
    assert_eq!(echoed.len(), payload.len());
    assert!(echoed == payload);
}

/// The reason the bridge has flow control at all: a pane nobody is reading
/// must not hold up a control reply on the same link.
#[test]
fn a_stream_nobody_reads_does_not_block_the_others() {
    let daemon = FakeDaemon::start();
    let (bridge, _server) = bridge_to(&daemon);

    let mut stalled = bridge.open().unwrap();
    stalled.write_all(b"F").unwrap();
    // Let the flood fill its window and stop.
    thread::sleep(Duration::from_millis(300));
    // What sits between the daemon and the reader is the window plus the
    // socket buffers on either side of the link, not the whole flood: the far
    // side stops reading the daemon once the near side stops granting credit.
    let buffered = daemon.flooded.load(Ordering::SeqCst);
    assert!(
        buffered < FLOOD_BYTES / 2,
        "{buffered} of {FLOOD_BYTES} bytes left the daemon for a reader that reads nothing"
    );

    let started = Instant::now();
    assert_eq!(echo(&bridge, b"still answered"), b"still answered");
    assert!(started.elapsed() < Duration::from_secs(5));

    // And the stalled stream still gets everything once it is read.
    stalled.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut flooded = Vec::new();
    stalled.read_to_end(&mut flooded).unwrap();
    assert_eq!(flooded.len(), FLOOD_BYTES);
}

#[test]
fn a_stream_the_far_side_cannot_connect_ends_instead_of_hanging() {
    let (near, far) = UnixStream::pair().unwrap();
    let far_reader = far.try_clone().unwrap();
    thread::spawn(move || {
        serve(
            far_reader,
            far,
            || Err(io::ErrorKind::ConnectionRefused.into()),
            BridgeInfo::default,
        )
    });
    let bridge = MuxBridge::connect(near.try_clone().unwrap(), near, TIMEOUT).unwrap();

    let mut stream = bridge.open().unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut received = Vec::new();
    assert_eq!(stream.read_to_end(&mut received).unwrap(), 0);
    assert!(bridge.is_alive(), "one refused stream is not a dead link");
}

#[test]
fn a_dead_link_ends_every_stream_and_refuses_new_ones() {
    let daemon = FakeDaemon::start();
    let (near, far) = UnixStream::pair().unwrap();
    let socket = daemon.socket.clone();
    let far_reader = far.try_clone().unwrap();
    let far_closer = far.try_clone().unwrap();
    thread::spawn(move || {
        serve(
            far_reader,
            far,
            move || UnixStream::connect(&socket),
            BridgeInfo::default,
        )
    });
    let bridge = MuxBridge::connect(near.try_clone().unwrap(), near, TIMEOUT).unwrap();
    let mut open = bridge.open().unwrap();
    open.write_all(b"E").unwrap();

    far_closer.shutdown(Shutdown::Both).unwrap();
    open.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut received = Vec::new();
    let _ = open.read_to_end(&mut received);
    let deadline = Instant::now() + TIMEOUT;
    while bridge.is_alive() {
        assert!(
            Instant::now() < deadline,
            "the bridge did not notice its link end"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(bridge.open().is_err());
}

#[test]
fn something_that_is_not_a_bridge_is_named_as_such() {
    let (near, mut far) = UnixStream::pair().unwrap();
    far.write_all(b"zmux: unknown command proxy-mux\n").unwrap();
    drop(far);
    let error = MuxBridge::connect(near.try_clone().unwrap(), near, TIMEOUT)
        .err()
        .expect("a stray line is not a greeting");
    assert!(format!("{error:#}").contains("proxy-mux"), "{error:#}");
}
