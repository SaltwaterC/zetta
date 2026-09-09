//! Server-facing Mosh protocol boundary.
//!
//! The wire protocol remains stock Mosh protocol v2.  The important distinction
//! for a server is that an inbound packet is *not* a byte-stream fragment: once
//! SSP accepts a complete remote state we need the state relationship
//! (`old_num -> new_num`) as well as its UserStream diff.  Exposing that here
//! prevents the PTY layer from replaying a cumulative/retransmitted UserStream.

use crate::terminal_state::TerminalQuery;
use anyhow::{Result, anyhow};
use flate2::read::ZlibDecoder;
use moshcatty::Ocb;
use moshcatty::crypto::{DIR_TO_CLIENT, DIR_TO_SERVER};
use moshcatty::fragment::{Assembler, Fragment, MAX_INSTRUCTION_BYTES};
use moshcatty::pb::{HostInstruction, TransportInstruction};
use moshcatty::transport::Transport;
use std::io::Read;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedState {
    pub old_num: u64,
    pub new_num: u64,
    pub ack_num: u64,
    pub throwaway_num: u64,
    pub diff: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ReceiveOutcome {
    /// True when the underlying Mosh transport authenticated this datagram.
    /// This is intentionally independent of whether the datagram completed an
    /// SSP state, so authenticated fragments can move the roaming peer address.
    pub authenticated: bool,
    /// Present only when SSP accepted a complete, non-duplicate remote state.
    pub state: Option<ReceivedState>,
}

/// Encode the standard host instructions together with zosh's optional
/// terminal-query extension. MoshCatty's host protobuf intentionally only
/// knows stock fields, so extension instructions are appended as ordinary
/// repeated `HostMessage.instruction` fields. Stock clients skip field 20.
pub(crate) fn encode_host_message(
    instructions: &[HostInstruction],
    queries: &[TerminalQuery],
) -> Vec<u8> {
    let mut message = HostInstruction::encode_message(instructions);
    for query in queries {
        let mut query_message = Vec::new();
        append_tag_varint(&mut query_message, 1, query.id);
        append_tag_bytes(&mut query_message, 2, &query.bytes);

        let mut instruction = Vec::new();
        append_tag_bytes(&mut instruction, 20, &query_message);
        append_tag_bytes(&mut message, 1, &instruction);
    }
    message
}

const WIRE_VARINT: u64 = 0;
const WIRE_BYTES: u64 = 2;

fn append_tag_varint(buffer: &mut Vec<u8>, field: u64, value: u64) {
    append_varint(buffer, (field << 3) | WIRE_VARINT);
    append_varint(buffer, value);
}

fn append_tag_bytes(buffer: &mut Vec<u8>, field: u64, value: &[u8]) {
    append_varint(buffer, (field << 3) | WIRE_BYTES);
    append_varint(buffer, value.len() as u64);
    buffer.extend_from_slice(value);
}

fn append_varint(buffer: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buffer.push((value as u8) | 0x80);
        value >>= 7;
    }
    buffer.push(value as u8);
}

/// Stock-protocol server transport.  MoshCatty supplies the protocol-v2 OCB,
/// fragmentation and SSP state machine; this wrapper deliberately exposes the
/// complete accepted state needed by a server rather than collapsing it to the
/// raw diff bytes.
pub struct ServerTransport {
    inner: Transport,
    observer: WireStateObserver,
}

impl ServerTransport {
    pub fn new(ocb: Ocb) -> Self {
        Self {
            observer: WireStateObserver::new(ocb.clone()),
            inner: Transport::new_server(ocb),
        }
    }

    pub fn receive(&mut self, packet: &[u8]) -> Result<ReceiveOutcome> {
        // Observe the same authenticated wire instruction before feeding it to
        // SSP.  The observer does not decide acceptance; the canonical SSP
        // implementation below still owns replay, old-state, ACK and quench
        // semantics.
        let observed = self.observer.observe(packet);
        let before = self.inner.last_recv();
        let had_state = self.inner.has_received_authenticated();
        let accepted_diff = self.inner.recv(packet);
        let authenticated = self.inner.last_recv() > before
            || (!had_state && self.inner.has_received_authenticated());

        let state = match accepted_diff {
            None => None,
            Some(diff) => {
                let instruction = observed.ok_or_else(|| {
                    anyhow!(
                        "Mosh SSP accepted a complete state that the server state observer did not reconstruct"
                    )
                })?;

                // The observer is diagnostic/state metadata only.  The diff
                // returned by the canonical SSP receiver remains authoritative.
                if instruction.diff != diff {
                    return Err(anyhow!(
                        "Mosh SSP state observer diverged from the accepted UserStream diff"
                    ));
                }

                Some(ReceivedState {
                    old_num: instruction.old_num,
                    new_num: instruction.new_num,
                    ack_num: instruction.ack_num,
                    throwaway_num: instruction.throwaway_num,
                    diff,
                })
            }
        };

        Ok(ReceiveOutcome {
            authenticated,
            state,
        })
    }

    pub fn set_pending(&mut self, diff: Vec<u8>) -> u64 {
        self.inner.set_pending(diff)
    }

    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        self.inner.tick()
    }

    /// Send on the next `tick` rather than at the next scheduled
    /// deadline.  SSP would already answer a non-empty diff within its
    /// 100 ms delayed-ack window; this is what turns a keep-alive's
    /// answer into the same loop pass.
    pub fn force_next_send(&mut self) {
        self.inner.force_next_send();
    }

    pub fn acked_by_remote(&self) -> u64 {
        self.inner.acked_by_remote()
    }

    pub fn ack_num(&self) -> u64 {
        self.inner.ack_num()
    }

    pub fn take_rebase_required(&mut self) -> bool {
        self.inner.take_rebase_required()
    }

    pub fn has_received_authenticated(&self) -> bool {
        self.inner.has_received_authenticated()
    }

    pub fn last_recv(&self) -> Instant {
        self.inner.last_recv()
    }

    pub fn crypto_exhausted(&self) -> bool {
        self.inner.crypto_exhausted()
    }

    pub fn start_shutdown(&mut self) {
        self.inner.start_shutdown();
    }

    pub fn shutdown_acknowledged(&self) -> bool {
        self.inner.shutdown_acknowledged()
    }

    pub fn shutdown_timed_out(&self) -> bool {
        self.inner.shutdown_timed_out()
    }

    pub fn counterparty_shutdown_ack_sent(&self) -> bool {
        self.inner.counterparty_shutdown_ack_sent()
    }
}

/// Side-car decoder for the metadata hidden by the current dependency's
/// diff-only receive convenience API.  It uses the exact same public OCB,
/// fragment and protobuf codecs; SSP acceptance remains exclusively in
/// `Transport::recv` above.
struct WireStateObserver {
    ocb: Ocb,
    assembler: Assembler,
}

impl WireStateObserver {
    fn new(ocb: Ocb) -> Self {
        Self {
            ocb,
            assembler: Assembler::new(),
        }
    }

    fn observe(&mut self, packet: &[u8]) -> Option<TransportInstruction> {
        let (dir_seq, plaintext) = self.ocb.open_datagram(packet)?;
        // A server accepts only client -> server packets. Keep the observer's
        // fragment stream aligned with the canonical SSP receiver even if a
        // valid session-key holder reflects a server packet back at us.
        if dir_seq & DIR_TO_CLIENT != DIR_TO_SERVER {
            return None;
        }
        // Mosh encrypted plaintext begins with timestamp + timestamp_reply.
        let fragment_bytes = plaintext.get(4..)?;
        let fragment = Fragment::decode(fragment_bytes).ok()?;
        let compressed = self.assembler.add(fragment)?;
        let protobuf = decompress_bounded(&compressed)?;
        TransportInstruction::decode(&protobuf).ok()
    }
}

fn decompress_bounded(compressed: &[u8]) -> Option<Vec<u8>> {
    let decoder = ZlibDecoder::new(compressed);
    let mut limited = decoder.take((MAX_INSTRUCTION_BYTES as u64) + 1);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    (out.len() <= MAX_INSTRUCTION_BYTES).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_receive_returns_ssp_state_numbers_without_changing_wire_protocol() {
        use moshcatty::pb::UserInstruction;

        let key = [0x42u8; 16];
        let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
        let mut client = Transport::new_client(Ocb::new(&key).unwrap());
        let payload =
            UserInstruction::encode_message(&[UserInstruction::keystroke(b"echo test\r".to_vec())]);
        let expected_new = client.set_pending(payload.clone());
        client.force_next_send();

        let mut accepted = None;
        for datagram in client.tick() {
            let outcome = server.receive(&datagram).unwrap();
            if outcome.state.is_some() {
                accepted = outcome.state;
            }
        }

        let accepted = accepted.expect("client state should complete");
        assert_eq!(accepted.old_num, 0);
        assert_eq!(accepted.new_num, expected_new);
        assert_eq!(accepted.diff, payload);
    }

    #[test]
    fn received_state_is_explicit_server_api() {
        let state = ReceivedState {
            old_num: 7,
            new_num: 11,
            ack_num: 5,
            throwaway_num: 4,
            diff: b"user-stream-diff".to_vec(),
        };
        assert_eq!(state.old_num, 7);
        assert_eq!(state.new_num, 11);
        assert_eq!(state.diff, b"user-stream-diff");
    }

    #[test]
    fn terminal_query_extension_is_ignored_by_stock_host_decoder() {
        let encoded = encode_host_message(
            &[],
            &[TerminalQuery {
                id: 7,
                bytes: b"\x1b]10;?\x07".to_vec(),
            }],
        );

        assert_eq!(
            encoded,
            vec![
                0x0A, 0x0E, 0xA2, 0x01, 0x0B, 0x08, 0x07, 0x12, 0x07, 0x1B, 0x5D, 0x31, 0x30, 0x3B,
                0x3F, 0x07,
            ]
        );
        let stock = HostInstruction::decode_message(&encoded).unwrap();
        assert_eq!(stock.len(), 1);
        assert!(stock[0].hoststring.is_empty());
        assert_eq!(stock[0].width, 0);
        assert_eq!(stock[0].height, 0);
        assert_eq!(stock[0].echo_ack_num, -1);
    }
}
