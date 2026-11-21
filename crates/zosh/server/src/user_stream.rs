//! Reconstruction of the client-side Mosh UserStream.
//!
//! A Mosh SSP diff is relative to `old_num`; it must never be treated as fresh
//! PTY input just because the UDP datagram is new.  This module tracks the
//! accepted state graph and emits only events beyond the input prefix already
//! committed to the PTY.

use crate::protocol::ReceivedState;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;

#[cfg(test)]
use moshcatty::pb::UserInstruction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserEvent {
    Byte(u8),
    Resize {
        cols: u16,
        rows: u16,
    },
    TerminalResponse(Vec<u8>),
    AgentHello {
        version: u32,
    },
    AgentResponse {
        connection_id: u64,
        request_id: u64,
        frame: Vec<u8>,
        closed: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponse {
    pub connection_id: u64,
    pub request_id: u64,
    pub frame: Vec<u8>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedInput {
    pub frame: u64,
    pub events: Vec<UserEvent>,
    /// The keep-alive interval this state announced, in milliseconds, if
    /// it carried one.  It contributes no input; see `../../PROTOCOL.md`
    /// for what the server owes it.
    pub keep_alive_ms: Option<u32>,
    /// How much scrolled-off history this state asked to have carried, in
    /// KiB, if it asked at all.  Like the keep-alive it contributes no
    /// input, so the prefix arithmetic above never sees it.
    pub scrollback_kib: Option<u32>,
    pub clipboard_version: Option<u32>,
}

#[derive(Debug, Clone)]
struct StreamMeta {
    /// Number of UserEvents in the complete logical UserStream at this state.
    total_events: u64,
    /// Events in this state that are after `committed_events`.  Keeping only the
    /// uncommitted suffix bounds memory even during very long sessions.
    pending: Vec<UserEvent>,
}

pub struct UserStreamTracker {
    states: BTreeMap<u64, StreamMeta>,
    latest_state: u64,
    committed_events: u64,
}

impl UserStreamTracker {
    pub fn new() -> Self {
        let mut states = BTreeMap::new();
        states.insert(
            0,
            StreamMeta {
                total_events: 0,
                pending: Vec::new(),
            },
        );
        Self {
            states,
            latest_state: 0,
            committed_events: 0,
        }
    }

    /// Accept one complete SSP state and return only input events that have not
    /// previously been committed to the PTY.  Out-of-order/parallel states are
    /// retained as bases but do not replay their already-committed prefixes.
    pub fn accept(&mut self, state: &ReceivedState) -> Result<AcceptedInput> {
        if state.new_num == u64::MAX {
            return Ok(AcceptedInput {
                frame: state.new_num,
                events: Vec::new(),
                keep_alive_ms: None,
                scrollback_kib: None,
                clipboard_version: None,
            });
        }

        // Read before decoding, so every return below carries it. A
        // keep-alive is deliberately invisible to `decode_events`: the
        // prefix arithmetic here counts decoded events, and a stock Mosh
        // server decodes none from it either.
        let Announcements {
            keep_alive_ms,
            scrollback_kib,
            clipboard_version,
        } = announcements(&state.diff);

        let base = self.states.get(&state.old_num).cloned().ok_or_else(|| {
            anyhow!(
                "Mosh UserStream references unknown base state {}",
                state.old_num
            )
        })?;
        let delta = decode_events(&state.diff).context("decoding Mosh UserStream diff")?;
        let delta_len = u64::try_from(delta.len()).context("UserStream event count overflow")?;
        let total_events = base
            .total_events
            .checked_add(delta_len)
            .ok_or_else(|| anyhow!("Mosh UserStream event count overflow"))?;

        let pending = if base.total_events >= self.committed_events {
            let expected = base.total_events - self.committed_events;
            if u64::try_from(base.pending.len()).ok() != Some(expected) {
                bail!("Mosh UserStream state cache lost its uncommitted prefix");
            }
            let mut pending = base.pending.clone();
            pending.extend(delta);
            pending
        } else {
            // The base is behind the PTY's committed prefix.  A cumulative Mosh
            // diff can legitimately contain events that we have already acted
            // on; skip exactly that prefix and retain only genuinely new input.
            let already_committed = self.committed_events - base.total_events;
            if already_committed >= delta_len {
                Vec::new()
            } else {
                delta
                    .into_iter()
                    .skip(
                        usize::try_from(already_committed)
                            .context("UserStream prefix too large")?,
                    )
                    .collect()
            }
        };

        self.states.insert(
            state.new_num,
            StreamMeta {
                total_events,
                pending: pending.clone(),
            },
        );

        // Honor the remote throwaway watermark after constructing this state;
        // old_num is allowed to be exactly the state the peer is now retiring.
        self.states
            .retain(|num, _| *num >= state.throwaway_num || *num == state.new_num);

        if state.new_num <= self.latest_state {
            return Ok(AcceptedInput {
                frame: state.new_num,
                events: Vec::new(),
                keep_alive_ms,
                scrollback_kib,
                clipboard_version,
            });
        }

        if total_events < self.committed_events {
            bail!(
                "newer Mosh UserStream state {} regressed from {} committed events to {}",
                state.new_num,
                self.committed_events,
                total_events
            );
        }

        let old_committed = self.committed_events;
        self.committed_events = total_events;
        self.latest_state = state.new_num;
        let newly_committed = self.committed_events - old_committed;

        // Every cached branch is expressed relative to the old committed prefix.
        // Advance that common floor now that these events have been sent to the
        // PTY. States behind the floor retain only their count metadata.
        for meta in self.states.values_mut() {
            if meta.total_events <= self.committed_events {
                meta.pending.clear();
                continue;
            }
            let drain = usize::try_from(newly_committed)
                .unwrap_or(usize::MAX)
                .min(meta.pending.len());
            meta.pending.drain(..drain);
        }

        Ok(AcceptedInput {
            frame: state.new_num,
            events: pending,
            keep_alive_ms,
            scrollback_kib,
            clipboard_version,
        })
    }
}

/// `UserMessage.instruction`, the repeated field every user instruction
/// arrives in.
const INSTRUCTION_FIELD: u64 = 1;
/// Zosh's keep-alive on `ClientBuffers.Instruction`; see `PROTOCOL.md`.
const KEEP_ALIVE_FIELD: u64 = 20;
/// Zosh's terminal response on `ClientBuffers.Instruction`.
const TERMINAL_RESPONSE_FIELD: u64 = 21;
/// Zosh's scrollback request on `ClientBuffers.Instruction`; see `PROTOCOL.md`.
const SCROLLBACK_FIELD: u64 = 22;
/// Zosh's SSH-agent extension on `ClientBuffers.Instruction`.
const AGENT_FIELD: u64 = 23;
const CLIPBOARD_FIELD: u64 = 24;
const MAX_AGENT_FRAME: usize = 256 * 1024;
const WIRE_VARINT: u64 = 0;
const WIRE_FIXED64: u64 = 1;
const WIRE_BYTES: u64 = 2;
const WIRE_FIXED32: u64 = 5;

/// What a UserStream diff announces about the session rather than about the
/// user: the two zosh extensions that carry a setting and no input.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Announcements {
    keep_alive_ms: Option<u32>,
    scrollback_kib: Option<u32>,
    clipboard_version: Option<u32>,
}

/// The settings a UserStream diff announces, if it announces any.
///
/// This remains a small second pass over the same bytes. It skips
/// length-delimited fields by their length, so it is linear in the number of
/// protobuf fields and not in the size of a paste. The strict event decoder
/// below handles the fields moshcatty does not know while preserving its
/// keystroke/resize precedence.
///
/// Both are deliberately invisible to `decode_events`: the prefix arithmetic
/// there counts decoded events, and a stock Mosh server decodes neither.
///
/// A byte it cannot read is reported as "nothing announced" rather than as an
/// error. `decode_events` runs over the same diff and is what rejects a
/// malformed UserMessage properly.
fn announcements(diff: &[u8]) -> Announcements {
    let mut rest = diff;
    let mut found = Announcements::default();
    while let Some((field, wire)) = read_tag(&mut rest) {
        if (field, wire) == (INSTRUCTION_FIELD, WIRE_BYTES) {
            let Some(instruction) = read_bytes(&mut rest) else {
                return found;
            };
            // Keep walking rather than returning: a diff can carry several,
            // and the newest of each is the one now in force.
            let announced = instruction_announcements(instruction);
            found.keep_alive_ms = announced.keep_alive_ms.or(found.keep_alive_ms);
            found.scrollback_kib = announced.scrollback_kib.or(found.scrollback_kib);
            found.clipboard_version = announced.clipboard_version.or(found.clipboard_version);
            continue;
        }
        if !skip_field(&mut rest, wire) {
            return found;
        }
    }
    found
}

fn instruction_announcements(mut rest: &[u8]) -> Announcements {
    let mut found = Announcements::default();
    while let Some((field, wire)) = read_tag(&mut rest) {
        match (field, wire) {
            (KEEP_ALIVE_FIELD, WIRE_VARINT) => {
                found.keep_alive_ms = read_varint(&mut rest).and_then(|v| u32::try_from(v).ok());
            }
            (SCROLLBACK_FIELD, WIRE_VARINT) => {
                found.scrollback_kib = read_varint(&mut rest).and_then(|v| u32::try_from(v).ok());
            }
            (CLIPBOARD_FIELD, WIRE_VARINT) => {
                found.clipboard_version =
                    read_varint(&mut rest).and_then(|v| u32::try_from(v).ok());
            }
            _ => {
                if !skip_field(&mut rest, wire) {
                    return found;
                }
            }
        }
    }
    found
}

fn read_varint(rest: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for (index, byte) in rest.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            *rest = &rest[index + 1..];
            return Some(value);
        }
    }
    None
}

fn read_tag(rest: &mut &[u8]) -> Option<(u64, u64)> {
    let tag = read_varint(rest)?;
    Some((tag >> 3, tag & 0x7))
}

fn read_bytes<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let length = usize::try_from(read_varint(rest)?).ok()?;
    let (head, tail) = rest.split_at_checked(length)?;
    *rest = tail;
    Some(head)
}

fn skip_field(rest: &mut &[u8], wire: u64) -> bool {
    match wire {
        WIRE_VARINT => read_varint(rest).is_some(),
        WIRE_FIXED64 => advance(rest, 8),
        WIRE_BYTES => read_bytes(rest).is_some(),
        WIRE_FIXED32 => advance(rest, 4),
        // Wire types 3 and 4 are proto2 groups, which neither Mosh proto
        // uses, and the rest are not wire types at all.  Stop rather than
        // guess at a length.
        _ => false,
    }
}

fn advance(rest: &mut &[u8], count: usize) -> bool {
    match rest.split_at_checked(count) {
        Some((_, tail)) => {
            *rest = tail;
            true
        }
        None => false,
    }
}

fn decode_events(diff: &[u8]) -> Result<Vec<UserEvent>> {
    if diff.is_empty() {
        return Ok(Vec::new());
    }

    let mut events = Vec::new();

    let mut rest = diff;
    while !rest.is_empty() {
        let (field, wire) = read_tag_strict(&mut rest)?;
        if (field, wire) != (INSTRUCTION_FIELD, WIRE_BYTES) {
            skip_field_strict(&mut rest, wire)?;
            continue;
        }

        let instruction = read_bytes_strict(&mut rest)?;
        let decoded = decode_instruction(instruction)?;
        // Stock UserStream::apply_string uses `if keystroke ... else if
        // resize`. Preserve that precedence for a malformed instruction
        // carrying both, then apply the zosh response extension as its own
        // event so an unusual combined instruction remains ordered.
        if !decoded.keys.is_empty() {
            events.extend(decoded.keys.into_iter().map(UserEvent::Byte));
        } else if decoded.width > 0 && decoded.height > 0 {
            let cols = u16::try_from(decoded.width)
                .map_err(|_| anyhow!("invalid terminal width {}", decoded.width))?;
            let rows = u16::try_from(decoded.height)
                .map_err(|_| anyhow!("invalid terminal height {}", decoded.height))?;
            events.push(UserEvent::Resize { cols, rows });
        }
        if let Some(response) = decoded.terminal_response
            && !response.is_empty()
        {
            events.push(UserEvent::TerminalResponse(response));
        }
        if let Some(version) = decoded.agent_hello {
            events.push(UserEvent::AgentHello { version });
        }
        if let Some(response) = decoded.agent_response {
            events.push(UserEvent::AgentResponse {
                connection_id: response.connection_id,
                request_id: response.request_id,
                frame: response.frame,
                closed: response.closed,
            });
        }
    }

    Ok(events)
}

#[derive(Default)]
struct DecodedInstruction {
    keys: Vec<u8>,
    width: i32,
    height: i32,
    terminal_response: Option<Vec<u8>>,
    agent_hello: Option<u32>,
    agent_response: Option<AgentResponse>,
}

fn decode_instruction(mut rest: &[u8]) -> Result<DecodedInstruction> {
    let mut decoded = DecodedInstruction::default();
    while !rest.is_empty() {
        let (field, wire) = read_tag_strict(&mut rest)?;
        match (field, wire) {
            (2, WIRE_BYTES) => decode_keystroke(&mut rest, &mut decoded.keys)?,
            (3, WIRE_BYTES) => decode_resize(&mut rest, &mut decoded.width, &mut decoded.height)?,
            (TERMINAL_RESPONSE_FIELD, WIRE_BYTES) => {
                decoded.terminal_response = Some(read_bytes_strict(&mut rest)?.to_vec());
            }
            (AGENT_FIELD, WIRE_BYTES) => {
                let agent = decode_agent(read_bytes_strict(&mut rest)?)?;
                decoded.agent_hello = agent.hello;
                decoded.agent_response = agent.response;
            }
            _ => skip_field_strict(&mut rest, wire)?,
        }
    }
    Ok(decoded)
}

#[derive(Default)]
struct DecodedAgent {
    hello: Option<u32>,
    response: Option<AgentResponse>,
}

fn decode_agent(mut rest: &[u8]) -> Result<DecodedAgent> {
    let mut decoded = DecodedAgent::default();
    while !rest.is_empty() {
        let (field, wire) = read_tag_strict(&mut rest)?;
        match (field, wire) {
            (1, WIRE_BYTES) => {
                let mut hello = read_bytes_strict(&mut rest)?;
                let mut version = None;
                while !hello.is_empty() {
                    let (nested_field, nested_wire) = read_tag_strict(&mut hello)?;
                    if (nested_field, nested_wire) == (1, WIRE_VARINT) {
                        version = Some(u32::try_from(read_varint_strict(&mut hello)?)?);
                    } else {
                        skip_field_strict(&mut hello, nested_wire)?;
                    }
                }
                decoded.hello = version;
            }
            (2, WIRE_BYTES) => {
                decoded.response = Some(decode_agent_response(read_bytes_strict(&mut rest)?)?);
            }
            _ => skip_field_strict(&mut rest, wire)?,
        }
    }
    Ok(decoded)
}

fn decode_agent_response(mut rest: &[u8]) -> Result<AgentResponse> {
    let mut connection_id = None;
    let mut request_id = None;
    let mut frame = Vec::new();
    let mut closed = false;
    while !rest.is_empty() {
        let (field, wire) = read_tag_strict(&mut rest)?;
        match (field, wire) {
            (1, WIRE_VARINT) => connection_id = Some(read_varint_strict(&mut rest)?),
            (2, WIRE_VARINT) => request_id = Some(read_varint_strict(&mut rest)?),
            (3, WIRE_BYTES) => {
                let bytes = read_bytes_strict(&mut rest)?;
                if bytes.len() > MAX_AGENT_FRAME {
                    bail!("SSH-agent response exceeds {MAX_AGENT_FRAME} bytes");
                }
                frame = bytes.to_vec();
            }
            (4, WIRE_VARINT) => closed = read_varint_strict(&mut rest)? != 0,
            _ => skip_field_strict(&mut rest, wire)?,
        }
    }
    Ok(AgentResponse {
        connection_id: connection_id
            .ok_or_else(|| anyhow!("agent response lacks connection ID"))?,
        request_id: request_id.ok_or_else(|| anyhow!("agent response lacks request ID"))?,
        frame,
        closed,
    })
}

fn decode_keystroke(rest: &mut &[u8], keys: &mut Vec<u8>) -> Result<()> {
    let mut nested = read_bytes_strict(rest)?;
    while !nested.is_empty() {
        let (field, wire) = read_tag_strict(&mut nested)?;
        if (field, wire) == (4, WIRE_BYTES) {
            keys.extend_from_slice(read_bytes_strict(&mut nested)?);
        } else {
            skip_field_strict(&mut nested, wire)?;
        }
    }
    Ok(())
}

fn decode_resize(rest: &mut &[u8], width: &mut i32, height: &mut i32) -> Result<()> {
    let mut nested = read_bytes_strict(rest)?;
    while !nested.is_empty() {
        let (field, wire) = read_tag_strict(&mut nested)?;
        match (field, wire) {
            (5, WIRE_VARINT) => *width = read_varint_strict(&mut nested)? as i32,
            (6, WIRE_VARINT) => *height = read_varint_strict(&mut nested)? as i32,
            _ => skip_field_strict(&mut nested, wire)?,
        }
    }
    Ok(())
}

fn read_varint_strict(rest: &mut &[u8]) -> Result<u64> {
    let mut value = 0_u64;
    for (index, &byte) in rest.iter().enumerate().take(10) {
        let bits = u64::from(byte & 0x7f);
        if index == 9 && bits > 1 {
            bail!("protobuf varint overflows u64");
        }
        value |= bits << (index * 7);
        if byte & 0x80 == 0 {
            *rest = &rest[index + 1..];
            return Ok(value);
        }
    }
    bail!("truncated or oversized protobuf varint")
}

fn read_tag_strict(rest: &mut &[u8]) -> Result<(u64, u64)> {
    let tag = read_varint_strict(rest)?;
    Ok((tag >> 3, tag & 0x7))
}

fn read_bytes_strict<'a>(rest: &mut &'a [u8]) -> Result<&'a [u8]> {
    let length = usize::try_from(read_varint_strict(rest)?)
        .context("protobuf length does not fit in usize")?;
    let (head, tail) = rest
        .split_at_checked(length)
        .ok_or_else(|| anyhow!("truncated protobuf bytes field"))?;
    *rest = tail;
    Ok(head)
}

fn skip_field_strict(rest: &mut &[u8], wire: u64) -> Result<()> {
    match wire {
        WIRE_VARINT => {
            read_varint_strict(rest)?;
        }
        WIRE_FIXED64 => advance_strict(rest, 8)?,
        WIRE_BYTES => {
            read_bytes_strict(rest)?;
        }
        WIRE_FIXED32 => advance_strict(rest, 4)?,
        _ => bail!("unsupported protobuf wire type {wire}"),
    }
    Ok(())
}

fn advance_strict(rest: &mut &[u8], count: usize) -> Result<()> {
    let (_, tail) = rest
        .split_at_checked(count)
        .ok_or_else(|| anyhow!("truncated protobuf fixed-width field"))?;
    *rest = tail;
    Ok(())
}

#[cfg(test)]
#[path = "tests/user_stream.rs"]
mod tests;
