//! What a datagram's sequence number lets it do to the receiver. Only an
//! authenticated in-order datagram is contact from the peer: a captured one
//! replayed later must not refresh liveness (or, for `zosh-server`, move the
//! roaming peer), however long ago it was captured.

use super::*;
use crate::crypto::{Ocb, SEQ_MASK};
use crate::replay::REPLAY_WINDOW;
use rand::RngCore;

fn pair() -> ([u8; 16], Transport, Transport) {
    let mut key = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key);
    let server = Transport::new_server(Ocb::new(&key).unwrap());
    let client = Transport::new_client(Ocb::new(&key).unwrap());
    (key, server, client)
}

fn seq_of(datagram: &[u8]) -> u64 {
    u64::from_be_bytes(datagram[..8].try_into().unwrap()) & SEQ_MASK
}

/// `datagram` re-sealed under `seq`, as the peer would have sent it.
fn resealed(key: &[u8; 16], datagram: &[u8], seq: u64) -> Vec<u8> {
    let ocb = Ocb::new(key).unwrap();
    let (dir_seq, plaintext) = ocb.open_datagram(datagram).unwrap();
    ocb.seal_datagram((dir_seq & !SEQ_MASK) | seq, &plaintext)
}

fn an_hour_ago() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(3600))
        .unwrap_or_else(Instant::now)
}

#[test]
fn a_datagram_older_than_the_window_does_not_refresh_liveness() {
    let (key, mut server, mut client) = pair();
    client.set_pending(b"captured".to_vec());
    let captured = client.tick().remove(0);
    let first = seq_of(&captured);
    assert!(server.recv_state(&captured).is_some());
    // More distinct sequence numbers than any replay memory holds.
    for seq in first + 1..=first + REPLAY_WINDOW + 600 {
        server.recv_state(&resealed(&key, &captured, seq));
    }
    server.last_recv = an_hour_ago();
    let stale = server.last_recv();

    let replay = server.receive(&captured);

    assert!(!replay.authenticated, "too old to tell from a replay");
    assert!(!replay.in_order);
    assert!(replay.state.is_none());
    assert_eq!(server.last_recv(), stale, "a replay refreshed liveness");
}

#[test]
fn an_unseen_reordered_datagram_is_accepted_without_refreshing_liveness() {
    let (key, mut server, mut client) = pair();
    client.set_pending(b"one".to_vec());
    let one = client.tick().remove(0);
    let base = seq_of(&one) + 100;
    let overtaking = server.receive(&resealed(&key, &one, base + 1));
    assert!(overtaking.authenticated && overtaking.in_order);
    assert!(overtaking.state.is_some());
    server.last_recv = an_hour_ago();
    let stale = server.last_recv();

    client.set_pending(b"two".to_vec());
    client.force_next_send();
    let two = client.tick().remove(0);
    let overtaken = server.receive(&resealed(&key, &two, base));

    assert!(overtaken.authenticated, "reordering is not a replay");
    assert!(!overtaken.in_order);
    assert!(overtaken.state.is_some(), "SSP still takes its state");
    assert_eq!(server.last_recv(), stale);
}

#[test]
fn a_forged_datagram_cannot_move_the_window() {
    let (key, mut server, mut client) = pair();
    client.set_pending(b"x".to_vec());
    let datagram = client.tick().remove(0);
    let first = seq_of(&datagram);
    assert!(server.receive(&datagram).in_order);

    let mut forged = resealed(&key, &datagram, first + 10 * REPLAY_WINDOW);
    let last = forged.len() - 1;
    forged[last] ^= 1;
    let rejected = server.receive(&forged);
    assert!(!rejected.authenticated && !rejected.in_order);

    let next = server.receive(&resealed(&key, &datagram, first + 1));
    assert!(next.authenticated && next.in_order);
}

#[test]
fn an_in_order_datagram_refreshes_liveness() {
    let (key, mut server, mut client) = pair();
    client.set_pending(b"x".to_vec());
    let datagram = client.tick().remove(0);
    assert!(server.receive(&datagram).in_order);
    server.last_recv = an_hour_ago();
    let stale = server.last_recv();

    // A repeat of a state SSP already has still counts as contact.
    let repeat = server.receive(&resealed(&key, &datagram, seq_of(&datagram) + 1));

    assert!(repeat.in_order && repeat.state.is_none());
    assert!(server.last_recv() > stale);
}
