//! A datagram longer than stock Mosh's receive MTU is refused before it is
//! decrypted, even when it would have authenticated: no conforming peer sends
//! one, and opening it costs an allocation and a pass the size of the packet.

use super::*;
use crate::crypto::{Ocb, DIR_TO_SERVER, RECEIVE_MTU};

const KEY: [u8; 16] = [7; 16];

/// An authentic client-to-server datagram exactly `length` bytes long.
fn datagram(seq: u64, length: usize) -> Vec<u8> {
    let overhead = MIN_DATAGRAM;
    let plaintext = vec![0xa5; length - overhead];
    let wire = Ocb::new(&KEY)
        .unwrap()
        .seal_datagram(DIR_TO_SERVER | seq, &plaintext);
    assert_eq!(wire.len(), length);
    wire
}

#[test]
fn a_datagram_at_the_receive_mtu_still_authenticates() {
    let mut server = Transport::new_server(Ocb::new(&KEY).unwrap());
    assert!(server.receive(&datagram(1, RECEIVE_MTU)).authenticated);
    assert!(Ocb::new(&KEY)
        .unwrap()
        .open_datagram(&datagram(1, RECEIVE_MTU))
        .is_some());
}

#[test]
fn a_datagram_past_the_receive_mtu_is_refused_unopened() {
    let mut server = Transport::new_server(Ocb::new(&KEY).unwrap());
    let oversize = datagram(1, RECEIVE_MTU + 1);

    let received = server.receive(&oversize);

    assert!(!received.authenticated, "an oversize datagram was opened");
    assert!(received.state.is_none());
    assert!(Ocb::new(&KEY).unwrap().open_datagram(&oversize).is_none());
    // Refused before the replay window saw it, so its sequence number is
    // still usable by a datagram of a legal size.
    assert!(server.receive(&datagram(1, RECEIVE_MTU)).authenticated);
}
