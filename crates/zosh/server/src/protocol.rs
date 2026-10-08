//! Server-facing Mosh protocol boundary.
//!
//! The wire protocol remains stock Mosh protocol v2.  The important distinction
//! for a server is that an inbound packet is *not* a byte-stream fragment: once
//! SSP accepts a complete remote state we need the state relationship
//! (`old_num -> new_num`) as well as its UserStream diff.  Exposing that here
//! prevents the PTY layer from replaying a cumulative/retransmitted UserStream.

use crate::terminal_state::TerminalQuery;
use anyhow::Result;
use moshcatty::Ocb;
use moshcatty::pb::HostInstruction;
use moshcatty::transport::Transport;
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
    /// True when the underlying Mosh transport authenticated this datagram
    /// and it was not a replay the transport still remembers.
    pub authenticated: bool,
    /// True when it was also newer than every datagram authenticated before
    /// it. This, and not `authenticated`, is what may move the roaming peer
    /// address: a captured datagram replayed from elsewhere authenticates
    /// too. It is independent of whether the datagram completed an SSP
    /// state, so in-order fragments still roam, as in stock Mosh.
    pub in_order: bool,
    /// Present only when SSP accepted a complete, non-duplicate remote state.
    pub state: Option<ReceivedState>,
}

/// One cumulative Zosh agent record to append to a host state. The record ID
/// is what lets the client suppress a retransmitted request or close notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentHostRecord {
    Ready {
        supported: bool,
        error: Option<String>,
    },
    Request {
        id: u64,
        connection_id: u64,
        frame: Vec<u8>,
    },
    Close {
        id: u64,
        connection_id: u64,
        error: Option<String>,
    },
}

/// Encode the standard host instructions together with zosh's optional
/// terminal-query extension. MoshCatty's host protobuf intentionally only
/// knows stock fields, so extension instructions are appended as ordinary
/// repeated `HostMessage.instruction` fields. Stock clients skip field 20.
#[cfg(test)]
pub(crate) fn encode_host_message(
    instructions: &[HostInstruction],
    queries: &[TerminalQuery],
) -> Vec<u8> {
    encode_host_message_with_agent(instructions, queries, &[])
}

pub(crate) fn encode_host_message_with_agent(
    instructions: &[HostInstruction],
    queries: &[TerminalQuery],
    agent_records: &[AgentHostRecord],
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
    for record in agent_records {
        let mut agent = Vec::new();
        match record {
            AgentHostRecord::Ready { supported, error } => {
                let mut ready = Vec::new();
                append_tag_varint(&mut ready, 1, u64::from(*supported));
                if let Some(error) = error {
                    append_tag_bytes(&mut ready, 2, error.as_bytes());
                }
                append_tag_bytes(&mut agent, 1, &ready);
            }
            AgentHostRecord::Request {
                id,
                connection_id,
                frame,
            } => {
                let mut request = Vec::new();
                append_tag_varint(&mut request, 1, *id);
                append_tag_varint(&mut request, 2, *connection_id);
                append_tag_bytes(&mut request, 3, frame);
                append_tag_bytes(&mut agent, 2, &request);
            }
            AgentHostRecord::Close {
                id,
                connection_id,
                error,
            } => {
                let mut close = Vec::new();
                append_tag_varint(&mut close, 1, *id);
                append_tag_varint(&mut close, 2, *connection_id);
                if let Some(error) = error {
                    append_tag_bytes(&mut close, 3, error.as_bytes());
                }
                append_tag_bytes(&mut agent, 3, &close);
            }
        }
        let mut instruction = Vec::new();
        append_tag_bytes(&mut instruction, 21, &agent);
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
}

impl ServerTransport {
    pub fn new(ocb: Ocb) -> Self {
        Self {
            inner: Transport::new_server(ocb),
        }
    }

    pub fn receive(&mut self, packet: &[u8]) -> Result<ReceiveOutcome> {
        // The canonical SSP receiver owns replay, old-state, ACK and quench
        // semantics, and hands back the numbering a server needs alongside
        // the diff. Each datagram is opened and decompressed once.
        let received = self.inner.receive(packet);
        let state = received.state.map(|state| ReceivedState {
            old_num: state.old_num,
            new_num: state.new_num,
            ack_num: state.ack_num,
            throwaway_num: state.throwaway_num,
            diff: state.diff,
        });
        Ok(ReceiveOutcome {
            authenticated: received.authenticated,
            in_order: received.in_order,
            state,
        })
    }

    #[cfg(test)]
    pub fn set_pending(&mut self, diff: Vec<u8>) -> u64 {
        self.inner.set_pending(diff)
    }

    /// The state a new frame should be diffed from: the newest one the peer
    /// has probably received, or `None` for the acknowledged one — always so
    /// once the newest queued state's base has aged past assuming.
    pub fn frame_base(&self) -> Option<u64> {
        if self.inner.prospective_chain_expired() {
            return None;
        }
        self.inner
            .prospective_base_num()
            .filter(|state| *state != self.inner.acked_by_remote())
    }

    /// Whether the newest queued state rests on a base the peer may never
    /// have had, so the next frame has to be built from the acknowledged one.
    pub fn frame_base_expired(&self) -> bool {
        self.inner.prospective_chain_expired()
    }

    /// Queue a frame diffed from `base` (`None`: the acknowledged state), and
    /// only from it; `None` when that base can no longer be named.
    pub fn set_pending_on(&mut self, base: Option<u64>, diff: Vec<u8>) -> Option<u64> {
        let base = base.unwrap_or_else(|| self.inner.acked_by_remote());
        self.inner.set_pending_on(base, diff)
    }

    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        self.inner.tick()
    }

    /// The pace frames may go out at; see `Transport::send_interval`.
    pub fn send_interval(&self) -> std::time::Duration {
        self.inner.send_interval()
    }

    /// When `tick` next has anything to do; see `Transport::next_deadline`.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.inner.next_deadline()
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

    #[test]
    fn agent_host_extension_is_ignored_by_stock_host_decoder() {
        let encoded = encode_host_message_with_agent(
            &[],
            &[],
            &[
                AgentHostRecord::Ready {
                    supported: true,
                    error: None,
                },
                AgentHostRecord::Request {
                    id: 4,
                    connection_id: 8,
                    frame: vec![0, 0, 0, 1, 6],
                },
            ],
        );

        let stock = HostInstruction::decode_message(&encoded).unwrap();
        assert_eq!(stock.len(), 2);
        assert!(stock.iter().all(|instruction| {
            instruction.hoststring.is_empty()
                && instruction.width == 0
                && instruction.height == 0
                && instruction.echo_ack_num == -1
        }));
        assert!(encoded.windows(2).any(|tag| tag == [0xAA, 0x01]));
    }
}
