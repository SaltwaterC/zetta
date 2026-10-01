//! `Transport::next_deadline` is what lets a caller sleep instead of polling
//! `tick`, so the property every test here pins is the one that makes that
//! safe: `tick` never has anything to send before the deadline it reported,
//! and it does have something once that deadline passes.

use super::*;
use crate::crypto::Ocb;
use rand::RngCore;

fn pair() -> (Transport, Transport) {
    let mut key = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key);
    let server = Transport::new_server(Ocb::new(&key).unwrap());
    let client = Transport::new_client(Ocb::new(&key).unwrap());
    (server, client)
}

/// Slack for the clock moving between `next_deadline` and `tick`, each of
/// which reads it separately.
const CLOCK_SLACK: Duration = Duration::from_millis(20);

/// Asserts that `tick` stays quiet while the reported deadline is still
/// comfortably in the future, and returns that deadline.
fn quiet_until_deadline(transport: &mut Transport) -> Instant {
    let deadline = transport.next_deadline().expect("a deadline is scheduled");
    assert!(
        deadline > Instant::now() + CLOCK_SLACK,
        "expected a future deadline"
    );
    assert!(
        transport.tick().is_empty(),
        "tick sent before the deadline it reported"
    );
    deadline
}

fn due_now(transport: &Transport) -> bool {
    transport
        .next_deadline()
        .is_some_and(|deadline| deadline <= Instant::now())
}

fn deliver(datagrams: Vec<Vec<u8>>, to: &mut Transport) {
    for datagram in datagrams {
        to.recv(&datagram);
    }
}

#[test]
fn an_idle_transport_is_next_due_at_its_heartbeat() {
    let (mut server, _) = pair();
    let deadline = quiet_until_deadline(&mut server);
    assert_eq!(deadline, server.last_send + HEARTBEAT_INTERVAL);
}

#[test]
fn a_forced_send_is_due_at_once() {
    let (mut server, _) = pair();
    server.force_next_send();
    assert!(due_now(&server));
    assert!(!server.tick().is_empty());
    // Forcing is consumed by the send it caused.
    quiet_until_deadline(&mut server);
}

#[test]
fn an_unacknowledged_state_is_due_for_retransmission_after_the_rto() {
    let (mut server, _) = pair();
    let state = server.set_pending(b"screen".to_vec());
    assert!(due_now(&server), "an unpaced new state goes out at once");
    assert!(!server.tick().is_empty());

    let sent = server.outbound_states[0].last_sent.unwrap();
    let deadline = quiet_until_deadline(&mut server);
    assert_eq!(deadline, sent + server.rto + ACK_DELAY);

    server.backdate_sent_state_for_test(state, server.rto + ACK_DELAY);
    assert!(due_now(&server));
    assert!(!server.tick().is_empty(), "the retransmission was not sent");
}

#[test]
fn a_peer_gone_quiet_slows_retransmission_to_the_heartbeat() {
    let (mut server, _) = pair();
    server.set_pending(b"screen".to_vec());
    assert!(!server.tick().is_empty());
    server.backdate_last_remote_state_for_test(ACTIVE_RETRY_TIMEOUT * 2);

    let sent = server.outbound_states[0].last_sent.unwrap();
    let deadline = quiet_until_deadline(&mut server);
    assert_eq!(deadline, sent + HEARTBEAT_INTERVAL);
}

#[test]
fn a_state_sent_as_the_peer_goes_quiet_is_retried_on_the_heartbeat() {
    let (mut server, _) = pair();
    server.set_pending(b"screen".to_vec());
    assert!(!server.tick().is_empty());
    // The active window closes before the active retry would come due, so
    // by the time it would, `tick` is applying the heartbeat rule instead.
    server.backdate_last_remote_state_for_test(ACTIVE_RETRY_TIMEOUT - ACK_DELAY);

    let sent = server.outbound_states[0].last_sent.unwrap();
    assert!(sent + server.rto + ACK_DELAY >= server.last_remote_state + ACTIVE_RETRY_TIMEOUT);
    let deadline = quiet_until_deadline(&mut server);
    assert_eq!(deadline, sent + HEARTBEAT_INTERVAL);
}

#[test]
fn received_data_is_acknowledged_once_the_delayed_ack_expires() {
    let (mut server, mut client) = pair();
    client.set_pending(b"keys".to_vec());
    deliver(client.tick(), &mut server);
    assert!(server.pending_data_ack);

    let deadline = quiet_until_deadline(&mut server);
    assert_eq!(deadline, server.pending_ack_since.unwrap() + ACK_DELAY);

    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    assert!(due_now(&server));
    assert!(!server.tick().is_empty(), "the delayed ack was not sent");
}

#[test]
fn a_paced_state_waits_for_the_send_interval() {
    let (_, mut client) = pair();
    client.set_pending(b"a".to_vec());
    assert!(due_now(&client), "the first state is allowed out at once");
    assert!(!client.tick().is_empty());

    client.set_pending(b"ab".to_vec());
    let deadline = quiet_until_deadline(&mut client);
    assert_eq!(deadline, client.last_send + client.send_interval());
}

#[test]
fn shutdown_is_due_at_once_then_paced_until_it_times_out() {
    let (mut server, _) = pair();
    server.start_shutdown();
    assert!(due_now(&server));
    assert!(!server.tick().is_empty());

    let deadline = quiet_until_deadline(&mut server);
    assert_eq!(deadline, server.last_send + server.send_interval());

    server.shutdown_tries = SHUTDOWN_RETRIES;
    assert!(server.shutdown_timed_out());
    assert_eq!(server.next_deadline(), None);
    assert!(server.tick().is_empty());
}

#[test]
fn a_shutdown_waits_no_longer_than_its_timeout() {
    let (mut server, _) = pair();
    server.start_shutdown();
    assert!(!server.tick().is_empty());
    let timeout = Instant::now() + Duration::from_millis(50);
    server.shutdown_started = Some(timeout - ACTIVE_RETRY_TIMEOUT);
    assert_eq!(server.next_deadline(), Some(timeout));
}

#[test]
fn an_acknowledged_shutdown_has_nothing_scheduled() {
    let (mut server, mut client) = pair();
    server.start_shutdown();
    deliver(server.tick(), &mut client);
    client.force_next_send();
    deliver(client.tick(), &mut server);
    assert!(server.shutdown_acknowledged());
    assert_eq!(server.next_deadline(), None);
}

#[test]
fn a_kept_compressor_writes_what_a_fresh_encoder_would() {
    // Not a deadline, but the other Zetta change to this file: the kept
    // stream must put the same bytes on the wire as a fresh encoder, whatever
    // it compressed before.
    let mut kept = KeptCompressor::default();
    let mut payloads = vec![Vec::new(), b"ack".to_vec(), vec![b'x'; 20_000]];
    let mut random = vec![0u8; 5000];
    rand::thread_rng().fill_bytes(&mut random);
    payloads.push(random);
    payloads.push(b"after a large one".to_vec());
    for payload in payloads {
        let compressed = kept.compress(&payload);
        assert_eq!(compressed, zlib_compress(&payload));
        assert_eq!(zlib_decompress(&compressed).unwrap(), payload);
    }
}
