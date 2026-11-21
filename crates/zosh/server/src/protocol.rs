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
#[path = "tests/protocol.rs"]
mod tests;
