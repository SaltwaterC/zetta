//! The relay and the bridge together, over in-process pipes, with a fake agent
//! where the Windows pipe would be.

use super::*;
use crate::{
    bridge,
    test_support::{ScratchDir, agent_frame},
};
use std::{
    io::{ErrorKind, pipe},
    os::unix::fs::PermissionsExt,
};

/// Answers each request with its body prefixed by `answer`.
fn fake_agent(answer: u8) -> io::Result<UnixStream> {
    let (ours, theirs) = UnixStream::pair()?;
    thread::spawn(move || {
        let mut agent = theirs;
        while let Ok(Some(request)) = read_agent_frame(&mut agent) {
            let mut body = vec![answer];
            body.extend_from_slice(&request[4..]);
            if agent.write_all(&agent_frame(&body)).is_err() {
                break;
            }
        }
    });
    Ok(ours)
}

/// Starts a relay and a bridge wired to each other, returning the socket.
/// Both run until the test process ends: each holds the other's input open.
fn start<C, S>(scratch: &ScratchDir, connect: C) -> PathBuf
where
    C: Fn() -> io::Result<S> + Send + Sync + 'static,
    S: Read + Write + 'static,
{
    let relay = Relay::bind(scratch.path()).unwrap();
    let socket = relay.socket_path().to_owned();
    let (relay_input, bridge_output) = pipe().unwrap();
    let (bridge_input, relay_output) = pipe().unwrap();
    thread::spawn(move || relay.serve(relay_input, relay_output));
    thread::spawn(move || bridge::run(bridge_input, bridge_output, connect));
    socket
}

fn round_trip(client: &mut UnixStream, body: &[u8]) -> Option<Vec<u8>> {
    client.write_all(&agent_frame(body)).unwrap();
    read_agent_frame(client)
        .unwrap()
        .map(|frame| frame[4..].to_vec())
}

#[test]
fn requests_reach_the_agent_and_replies_come_back() {
    let scratch = ScratchDir::new("relay-round-trip");
    let socket = start(&scratch, || fake_agent(12));
    let mut client = UnixStream::connect(&socket).unwrap();
    assert_eq!(round_trip(&mut client, &[11]), Some(vec![12, 11]));
    assert_eq!(
        round_trip(&mut client, &[13, 1, 2]),
        Some(vec![12, 13, 1, 2])
    );
}

#[test]
fn concurrent_clients_each_get_their_own_replies() {
    let scratch = ScratchDir::new("relay-concurrent");
    let socket = start(&scratch, || fake_agent(12));
    let clients: Vec<_> = (0..8_u8)
        .map(|index| {
            let socket = socket.clone();
            thread::spawn(move || {
                let mut client = UnixStream::connect(&socket).unwrap();
                for round in 0..20_u8 {
                    assert_eq!(
                        round_trip(&mut client, &[index, round]),
                        Some(vec![12, index, round])
                    );
                }
            })
        })
        .collect();
    for client in clients {
        client.join().unwrap();
    }
}

#[test]
fn an_unreachable_agent_closes_the_client_instead_of_hanging_it() {
    let scratch = ScratchDir::new("relay-unreachable");
    let socket = start(&scratch, || -> io::Result<UnixStream> {
        Err(io::Error::from(ErrorKind::NotFound))
    });
    let mut client = UnixStream::connect(&socket).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let _ = client.write_all(&agent_frame(&[11]));
    match read_agent_frame(&mut client) {
        Ok(None) => {}
        Err(error)
            if error.kind() != ErrorKind::WouldBlock && error.kind() != ErrorKind::TimedOut => {}
        other => panic!("expected the client to be closed, got {other:?}"),
    }
}

#[test]
fn a_client_that_leaves_does_not_disturb_the_next() {
    let scratch = ScratchDir::new("relay-leave");
    let socket = start(&scratch, || fake_agent(12));
    let mut first = UnixStream::connect(&socket).unwrap();
    first.write_all(&agent_frame(&[11])).unwrap();
    drop(first);
    let mut second = UnixStream::connect(&socket).unwrap();
    assert_eq!(round_trip(&mut second, &[11]), Some(vec![12, 11]));
}

#[test]
fn the_socket_is_private_and_removed_when_the_windows_side_goes() {
    let scratch = ScratchDir::new("relay-cleanup");
    let relay = Relay::bind(scratch.path()).unwrap();
    let socket = relay.socket_path().to_owned();
    let directory = socket.parent().unwrap().to_owned();
    let mode = fs::metadata(&directory).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);

    let (input, windows_side) = pipe().unwrap();
    let serving = thread::spawn(move || {
        let result = relay.serve(input, io::sink());
        drop(relay);
        result
    });
    let mut client = UnixStream::connect(&socket).unwrap();
    drop(windows_side);
    serving.join().unwrap().unwrap();
    assert!(
        !directory.exists(),
        "the socket directory outlived the relay"
    );
    let mut rest = Vec::new();
    assert_eq!(client.read_to_end(&mut rest).unwrap_or(0), 0);
}

#[test]
fn a_message_only_the_relay_sends_is_a_protocol_error() {
    let scratch = ScratchDir::new("relay-protocol");
    let relay = Relay::bind(scratch.path()).unwrap();
    let mut input = Vec::new();
    Message::Open(1).write_to(&mut input).unwrap();
    let error = relay.serve(io::Cursor::new(input), io::sink()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
}
