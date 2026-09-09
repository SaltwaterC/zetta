//! State sync: what each side's diffs actually contain
//! (`user.cc`, `completeterminal.cc`, `userinput.proto`,
//! `hostinput.proto`).
//!
//! The two directions are not symmetric, and that asymmetry is the
//! whole design of mosh:
//!
//! - **Client to server** is a [`UserStream`]: an append-only log of
//!   keystrokes and resizes. A diff is literally the suffix the peer
//!   has not seen yet, which is why it can be recomputed from any
//!   older state at no cost.
//! - **Server to client** is a diff of the TERMINAL, already rendered
//!   as ECMA-48 escape bytes by the server's own emulator, plus
//!   resizes and echo acknowledgements. The client never receives raw
//!   host output and never diffs terminal states itself: it feeds
//!   `hoststring` into its emulator exactly as it would feed a PTY.
//!
//! The protobufs are declared with prost derives rather than generated
//! from the `.proto` files. mosh declares its per-instruction fields
//! as proto2 *extensions*, which are ordinary wire fields with the
//! same numbers, so a plain message with those tags is byte-identical
//! on the wire and needs no `protoc`.

// These types mirror mosh's `.proto` files field for field, and the
// tag on each field is its documentation: `Keystroke.keys` is field 4
// because `userinput.proto` says so, and prose restating the name would
// be noise on top of the mapping the module docs above already give.
#![allow(missing_docs)]

use prost::Message as _;

use crate::error::{MoshError, Result};

// ---------------------------------------------------------------- //
// client -> server: userinput.proto (package ClientBuffers)
// ---------------------------------------------------------------- //

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Keystroke {
    #[prost(bytes = "vec", optional, tag = "4")]
    pub keys: Option<Vec<u8>>,
}

/// Also the host direction's resize; mosh declares one per package
/// with the same field numbers.
#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct ResizeMessage {
    #[prost(int32, optional, tag = "5")]
    pub width: Option<i32>,
    #[prost(int32, optional, tag = "6")]
    pub height: Option<i32>,
}

/// One user instruction. In the `.proto` these fields are extensions of
/// an empty message; on the wire they are just fields 2, 3, 20 and 21.
///
/// Field 20 is not mosh's. It is Zetta's keep-alive extension, and it
/// is deliberately a bare varint rather than a nested message so the
/// whole instruction costs five bytes inside a `UserMessage`. mosh
/// declares `Instruction` as `extensions 2 to max`, so an
/// implementation that does not know the number parses it into its
/// unknown-field set and its `if keystroke / else if resize` chain
/// ignores it. `../../zosh/PROTOCOL.md` is the specification.
///
/// Field 21 carries a response to a terminal query forwarded by a zosh
/// server. It is kept as a separate instruction so stock Mosh continues to
/// ignore it and so the server can write responses to its PTY in order.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct UserInstruction {
    #[prost(message, optional, tag = "2")]
    pub keystroke: Option<Keystroke>,
    #[prost(message, optional, tag = "3")]
    pub resize: Option<ResizeMessage>,
    /// The keep-alive interval this client is holding the session to,
    /// in milliseconds. Doubles as the marker: a server that recognises
    /// it may keep the session alive from its own side on the same
    /// interval, which a client-driven keep-alive alone cannot do.
    #[prost(uint32, optional, tag = "20")]
    pub zosh_keepalive_ms: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "21")]
    pub terminal_response: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct UserMessage {
    #[prost(message, repeated, tag = "1")]
    pub instruction: Vec<UserInstruction>,
}

impl UserMessage {
    /// Every keystroke byte in a serialized user diff, concatenated.
    /// Resizes are skipped, so this answers "what did the user type",
    /// which is what a caller checking a diff usually means.
    pub fn keystrokes_of(diff: &[u8]) -> Result<Vec<u8>> {
        let msg = Self::decode(diff).map_err(|_| MoshError::BadInstruction)?;
        Ok(msg
            .instruction
            .into_iter()
            .filter_map(|i| i.keystroke)
            .filter_map(|k| k.keys)
            .flatten()
            .collect())
    }
}

/// One thing the user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserEvent {
    /// A single byte of input. mosh keeps input byte-by-byte and
    /// coalesces only when serializing, so a diff can start anywhere.
    Byte(u8),
    Resize {
        width: i32,
        height: i32,
    },
    /// Zetta's keep-alive: an event the user did not cause and no
    /// server acts on. It exists to make the diff NON-EMPTY, because
    /// that is what every mosh server keys its prompt acknowledgement
    /// off (`!inst.diff().empty()` sets the data-ack), so a peer answers
    /// within `ACK_DELAY` instead of at the three-second heartbeat.
    ///
    /// It carries the interval, in milliseconds, that the client is
    /// holding the session to. A server that understands the field can
    /// then hold up its own half on the same interval instead of only
    /// ever replying; see `zosh`'s `PROTOCOL.md`.
    KeepAlive(u32),
    /// A response returned by the local terminal to a query the server
    /// forwarded. It is not keyboard input, but it is part of the cumulative
    /// UserStream so retransmitted states cannot write it twice to the PTY.
    TerminalResponse(Vec<u8>),
}

/// The client's state: everything the user has done, in order.
///
/// Diffs are pure suffixes, so a state is "older" than another exactly
/// when it is a prefix of it. That is what lets the sender recompute a
/// diff against any state the receiver might still be holding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserStream {
    events: Vec<UserEvent>,
}

impl UserStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_byte(&mut self, byte: u8) {
        self.events.push(UserEvent::Byte(byte));
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.events
            .extend(bytes.iter().copied().map(UserEvent::Byte));
    }

    pub fn push_resize(&mut self, width: i32, height: i32) {
        self.events.push(UserEvent::Resize { width, height });
    }

    /// Append a keep-alive announcing `interval_ms`. See
    /// [`UserEvent::KeepAlive`].
    pub fn push_keep_alive(&mut self, interval_ms: u32) {
        self.events.push(UserEvent::KeepAlive(interval_ms));
    }

    /// Append bytes returned by the local terminal for a forwarded query.
    pub fn push_terminal_response(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.events
                .push(UserEvent::TerminalResponse(bytes.to_vec()));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn events(&self) -> &[UserEvent] {
        &self.events
    }

    /// The diff that takes `existing` to `self`: the events `existing`
    /// does not have yet, serialized.
    ///
    /// Contiguous bytes are coalesced into one `Keystroke`, which is
    /// what keeps a burst of typing to a single instruction; a resize
    /// breaks the run because it is its own instruction.
    ///
    /// Returns `None` when `existing` is not a prefix of `self`. mosh
    /// asserts here, because its sender only ever diffs against states
    /// it sent, and a violation means the sender lost track of what the
    /// receiver holds.
    pub fn diff_from(&self, existing: &UserStream) -> Option<Vec<u8>> {
        if !self.events.starts_with(&existing.events) {
            return None;
        }
        let mut msg = UserMessage::default();
        for event in &self.events[existing.events.len()..] {
            match event {
                UserEvent::Byte(b) => {
                    // Append to the run in progress, if the last
                    // instruction is one.
                    match msg
                        .instruction
                        .last_mut()
                        .and_then(|i| i.keystroke.as_mut())
                        .and_then(|k| k.keys.as_mut())
                    {
                        Some(keys) => keys.push(*b),
                        None => msg.instruction.push(UserInstruction {
                            keystroke: Some(Keystroke {
                                keys: Some(vec![*b]),
                            }),
                            resize: None,
                            zosh_keepalive_ms: None,
                            terminal_response: None,
                        }),
                    }
                }
                UserEvent::Resize { width, height } => {
                    msg.instruction.push(UserInstruction {
                        keystroke: None,
                        resize: Some(ResizeMessage {
                            width: Some(*width),
                            height: Some(*height),
                        }),
                        zosh_keepalive_ms: None,
                        terminal_response: None,
                    });
                }
                // Its own instruction, like a resize: a keep-alive
                // breaks the keystroke run rather than joining it, so a
                // peer that ignores field 20 sees an instruction with
                // nothing in it and the run either side stays intact.
                UserEvent::KeepAlive(interval_ms) => {
                    msg.instruction.push(UserInstruction {
                        keystroke: None,
                        resize: None,
                        zosh_keepalive_ms: Some(*interval_ms),
                        terminal_response: None,
                    });
                }
                UserEvent::TerminalResponse(bytes) => {
                    msg.instruction.push(UserInstruction {
                        keystroke: None,
                        resize: None,
                        zosh_keepalive_ms: None,
                        terminal_response: Some(bytes.clone()),
                    });
                }
            }
        }
        Some(msg.encode_to_vec())
    }

    /// The diff from nothing: what a fresh peer needs to catch up.
    pub fn init_diff(&self) -> Vec<u8> {
        self.diff_from(&UserStream::new()).unwrap_or_default()
    }

    /// Apply a diff produced by [`Self::diff_from`], appending its
    /// events. This is the server's job in a real session; the client
    /// runs it only to verify its own encoding.
    pub fn apply_string(&mut self, diff: &[u8]) -> Result<()> {
        let msg = UserMessage::decode(diff).map_err(|_| MoshError::BadInstruction)?;
        for inst in msg.instruction {
            if let Some(k) = inst.keystroke
                && let Some(keys) = k.keys
            {
                self.push_bytes(&keys);
            }
            if let Some(r) = inst.resize
                && let (Some(width), Some(height)) = (r.width, r.height)
            {
                self.push_resize(width, height);
            }
            if let Some(interval_ms) = inst.zosh_keepalive_ms {
                self.push_keep_alive(interval_ms);
            }
            if let Some(response) = inst.terminal_response {
                self.push_terminal_response(&response);
            }
        }
        Ok(())
    }

    /// Drop the prefix the receiver has confirmed, so a long session
    /// does not keep every keystroke it ever sent
    /// (`UserStream::subtract`). `false` when `prefix` is not one.
    pub fn subtract(&mut self, prefix: &UserStream) -> bool {
        if !self.events.starts_with(&prefix.events) {
            return false;
        }
        self.events.drain(..prefix.events.len());
        true
    }
}

// ---------------------------------------------------------------- //
// server -> client: hostinput.proto (package HostBuffers)
// ---------------------------------------------------------------- //

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HostBytes {
    /// ECMA-48 escape output, already rendered by the SERVER's
    /// terminal emulator as the difference between two framebuffers.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub hoststring: Option<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct EchoAck {
    /// The latest client state whose keystrokes the server has echoed.
    /// A client retires predictive local echo with this; one that does
    /// not predict may ignore it.
    #[prost(uint64, optional, tag = "8")]
    pub echo_ack_num: Option<u64>,
}

/// A terminal query forwarded by a zosh server. The query ID is local to the
/// server session and lets the client suppress the same query when a
/// cumulative host state is retransmitted.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TerminalQuery {
    #[prost(uint64, optional, tag = "1")]
    pub id: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub bytes: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HostInstruction {
    #[prost(message, optional, tag = "2")]
    pub hostbytes: Option<HostBytes>,
    #[prost(message, optional, tag = "3")]
    pub resize: Option<ResizeMessage>,
    #[prost(message, optional, tag = "7")]
    pub echoack: Option<EchoAck>,
    #[prost(message, optional, tag = "20")]
    pub terminal_query: Option<TerminalQuery>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct HostMessage {
    #[prost(message, repeated, tag = "1")]
    pub instruction: Vec<HostInstruction>,
}

/// What one host diff asks the client to do, flattened out of the
/// protobuf in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Feed these bytes to the terminal emulator.
    Bytes(Vec<u8>),
    Resize {
        width: i32,
        height: i32,
    },
    EchoAck(u64),
    TerminalQuery {
        id: u64,
        bytes: Vec<u8>,
    },
}

/// Decode a host diff into the events it carries.
pub fn parse_host_diff(diff: &[u8]) -> Result<Vec<HostEvent>> {
    let msg = HostMessage::decode(diff).map_err(|_| MoshError::BadInstruction)?;
    let mut out = Vec::new();
    for inst in msg.instruction {
        if let Some(e) = inst.echoack
            && let Some(num) = e.echo_ack_num
        {
            out.push(HostEvent::EchoAck(num));
        }
        if let Some(r) = inst.resize
            && let (Some(width), Some(height)) = (r.width, r.height)
        {
            out.push(HostEvent::Resize { width, height });
        }
        if let Some(h) = inst.hostbytes
            && let Some(bytes) = h.hoststring
            && !bytes.is_empty()
        {
            out.push(HostEvent::Bytes(bytes));
        }
        if let Some(query) = inst.terminal_query
            && let (Some(id), Some(bytes)) = (query.id, query.bytes)
            && !bytes.is_empty()
        {
            out.push(HostEvent::TerminalQuery { id, bytes });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_carries_only_the_suffix() {
        let mut old = UserStream::new();
        old.push_bytes(b"ls");
        let mut new = old.clone();
        new.push_bytes(b" -l\r");

        let diff = new.diff_from(&old).expect("old is a prefix");
        let mut rebuilt = old.clone();
        rebuilt.apply_string(&diff).unwrap();
        assert_eq!(rebuilt, new);
    }

    #[test]
    fn contiguous_bytes_coalesce_into_one_keystroke() {
        let mut s = UserStream::new();
        s.push_bytes(b"echo hi");
        let diff = s.init_diff();
        let msg = UserMessage::decode(diff.as_slice()).unwrap();
        assert_eq!(
            msg.instruction.len(),
            1,
            "one run of typing, one instruction"
        );
        assert_eq!(
            msg.instruction[0]
                .keystroke
                .as_ref()
                .unwrap()
                .keys
                .as_deref(),
            Some(&b"echo hi"[..])
        );
    }

    #[test]
    fn a_resize_breaks_the_run() {
        let mut s = UserStream::new();
        s.push_bytes(b"ab");
        s.push_resize(80, 24);
        s.push_bytes(b"cd");
        let msg = UserMessage::decode(s.init_diff().as_slice()).unwrap();
        assert_eq!(msg.instruction.len(), 3);
        assert!(msg.instruction[0].keystroke.is_some());
        assert_eq!(
            msg.instruction[1].resize,
            Some(ResizeMessage {
                width: Some(80),
                height: Some(24)
            })
        );
        assert!(msg.instruction[2].keystroke.is_some());
    }

    #[test]
    fn a_keep_alive_is_field_twenty_carrying_its_interval() {
        let mut s = UserStream::new();
        s.push_keep_alive(100);
        // The specification in zosh's PROTOCOL.md, byte for byte:
        //   0x0A 0x03        UserMessage.instruction, length 3
        //   0xA0 0x01 0x64   Instruction field 20, varint 100
        assert_eq!(s.init_diff(), vec![0x0A, 0x03, 0xA0, 0x01, 0x64]);
        // 500 needs a two-byte varint, so the instruction is one longer.
        let mut s = UserStream::new();
        s.push_keep_alive(500);
        assert_eq!(s.init_diff(), vec![0x0A, 0x04, 0xA0, 0x01, 0xF4, 0x03]);
    }

    #[test]
    fn a_keep_alive_breaks_the_run_and_is_not_typing() {
        let mut s = UserStream::new();
        s.push_bytes(b"ab");
        s.push_keep_alive(500);
        s.push_bytes(b"cd");
        let diff = s.init_diff();
        let msg = UserMessage::decode(diff.as_slice()).unwrap();
        assert_eq!(msg.instruction.len(), 3);
        assert_eq!(msg.instruction[1].zosh_keepalive_ms, Some(500));
        assert!(msg.instruction[1].keystroke.is_none());
        assert!(msg.instruction[1].resize.is_none());
        // A peer that ignores field 20 sees an instruction with nothing
        // in it; the typing either side of it is unaffected.
        assert_eq!(UserMessage::keystrokes_of(&diff).unwrap(), b"abcd".to_vec());
    }

    #[test]
    fn a_keep_alive_round_trips_like_any_other_event() {
        let mut old = UserStream::new();
        old.push_bytes(b"ls");
        let mut new = old.clone();
        new.push_keep_alive(250);
        new.push_bytes(b"\r");

        let diff = new.diff_from(&old).expect("old is a prefix");
        let mut rebuilt = old.clone();
        rebuilt.apply_string(&diff).unwrap();
        assert_eq!(rebuilt, new);
        assert_eq!(rebuilt.events()[2], UserEvent::KeepAlive(250));
    }

    #[test]
    fn a_terminal_response_is_a_standalone_field_twenty_one_instruction() {
        let mut stream = UserStream::new();
        stream.push_bytes(b"a");
        stream.push_terminal_response(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07");
        stream.push_bytes(b"b");

        let diff = stream.init_diff();
        let msg = UserMessage::decode(diff.as_slice()).unwrap();
        assert_eq!(msg.instruction.len(), 3);
        assert_eq!(
            msg.instruction[1].terminal_response.as_deref(),
            Some(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".as_slice())
        );
        assert!(msg.instruction[1].keystroke.is_none());

        let mut rebuilt = UserStream::new();
        rebuilt.apply_string(&diff).unwrap();
        assert_eq!(rebuilt, stream);
    }

    #[test]
    fn a_state_that_is_not_a_prefix_has_no_diff() {
        let mut a = UserStream::new();
        a.push_bytes(b"abc");
        let mut b = UserStream::new();
        b.push_bytes(b"xyz");
        assert!(b.diff_from(&a).is_none());
    }

    #[test]
    fn subtract_drops_the_confirmed_prefix() {
        let mut sent = UserStream::new();
        sent.push_bytes(b"hello");
        let mut acked = UserStream::new();
        acked.push_bytes(b"hel");

        assert!(sent.subtract(&acked));
        assert_eq!(sent.len(), 2);
        // What remains still diffs correctly against nothing.
        let msg = UserMessage::decode(sent.init_diff().as_slice()).unwrap();
        assert_eq!(
            msg.instruction[0]
                .keystroke
                .as_ref()
                .unwrap()
                .keys
                .as_deref(),
            Some(&b"lo"[..])
        );
    }

    #[test]
    fn subtract_refuses_a_state_that_is_not_a_prefix() {
        let mut sent = UserStream::new();
        sent.push_bytes(b"hello");
        let mut other = UserStream::new();
        other.push_bytes(b"world");
        assert!(!sent.subtract(&other));
        assert_eq!(sent.len(), 5, "a refused subtract changes nothing");
    }

    #[test]
    fn host_diffs_decode_in_arrival_order() {
        let msg = HostMessage {
            instruction: vec![
                HostInstruction {
                    hostbytes: None,
                    resize: None,
                    echoack: Some(EchoAck {
                        echo_ack_num: Some(7),
                    }),
                    terminal_query: None,
                },
                HostInstruction {
                    hostbytes: Some(HostBytes {
                        hoststring: Some(b"\x1b[2Jhello".to_vec()),
                    }),
                    resize: Some(ResizeMessage {
                        width: Some(100),
                        height: Some(30),
                    }),
                    echoack: None,
                    terminal_query: None,
                },
            ],
        };
        let events = parse_host_diff(&msg.encode_to_vec()).unwrap();
        assert_eq!(
            events,
            vec![
                HostEvent::EchoAck(7),
                // Within one instruction the resize is applied before
                // the bytes it reflows.
                HostEvent::Resize {
                    width: 100,
                    height: 30
                },
                HostEvent::Bytes(b"\x1b[2Jhello".to_vec()),
            ]
        );
    }

    #[test]
    fn an_empty_hoststring_is_not_an_event() {
        let msg = HostMessage {
            instruction: vec![HostInstruction {
                hostbytes: Some(HostBytes {
                    hoststring: Some(Vec::new()),
                }),
                resize: None,
                echoack: None,
                terminal_query: None,
            }],
        };
        assert!(parse_host_diff(&msg.encode_to_vec()).unwrap().is_empty());
    }

    #[test]
    fn a_terminal_query_decodes_with_its_id_and_bytes() {
        let msg = HostMessage {
            instruction: vec![HostInstruction {
                hostbytes: None,
                resize: None,
                echoack: None,
                terminal_query: Some(TerminalQuery {
                    id: Some(42),
                    bytes: Some(b"\x1b]11;?\x1b\\".to_vec()),
                }),
            }],
        };

        assert_eq!(
            parse_host_diff(&msg.encode_to_vec()).unwrap(),
            vec![HostEvent::TerminalQuery {
                id: 42,
                bytes: b"\x1b]11;?\x1b\\".to_vec(),
            }]
        );
    }
}
