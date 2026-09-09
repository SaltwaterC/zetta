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
    Resize { cols: u16, rows: u16 },
    TerminalResponse(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedInput {
    pub frame: u64,
    pub events: Vec<UserEvent>,
    /// The keep-alive interval this state announced, in milliseconds, if
    /// it carried one.  It contributes no input; see `../../PROTOCOL.md`
    /// for what the server owes it.
    pub keep_alive_ms: Option<u32>,
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
            });
        }

        // Read before decoding, so every return below carries it. A
        // keep-alive is deliberately invisible to `decode_events`: the
        // prefix arithmetic here counts decoded events, and a stock Mosh
        // server decodes none from it either.
        let keep_alive_ms = keep_alive_interval(&state.diff);

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
const WIRE_VARINT: u64 = 0;
const WIRE_FIXED64: u64 = 1;
const WIRE_BYTES: u64 = 2;
const WIRE_FIXED32: u64 = 5;

/// The keep-alive interval a UserStream diff announces, if it carries a
/// keep-alive at all.
///
/// This remains a small second pass over the same bytes. It skips
/// length-delimited fields by their length, so it is linear in the number of
/// protobuf fields and not in the size of a paste. The strict event decoder
/// below handles the fields moshcatty does not know while preserving its
/// keystroke/resize precedence.
///
/// A byte it cannot read is reported as "no keep-alive" rather than as an
/// error. `decode_events` runs over the same diff and is what rejects a
/// malformed UserMessage properly.
fn keep_alive_interval(diff: &[u8]) -> Option<u32> {
    let mut rest = diff;
    let mut found = None;
    while let Some((field, wire)) = read_tag(&mut rest) {
        if (field, wire) == (INSTRUCTION_FIELD, WIRE_BYTES) {
            let instruction = read_bytes(&mut rest)?;
            // Keep walking rather than returning: a diff can carry several
            // keep-alives, and the newest one is the interval now in force.
            found = instruction_keep_alive(instruction).or(found);
            continue;
        }
        if !skip_field(&mut rest, wire) {
            return found;
        }
    }
    found
}

fn instruction_keep_alive(mut rest: &[u8]) -> Option<u32> {
    while let Some((field, wire)) = read_tag(&mut rest) {
        if (field, wire) == (KEEP_ALIVE_FIELD, WIRE_VARINT) {
            return u32::try_from(read_varint(&mut rest)?).ok();
        }
        if !skip_field(&mut rest, wire) {
            return None;
        }
    }
    None
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
    }

    Ok(events)
}

#[derive(Default)]
struct DecodedInstruction {
    keys: Vec<u8>,
    width: i32,
    height: i32,
    terminal_response: Option<Vec<u8>>,
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
            _ => skip_field_strict(&mut rest, wire)?,
        }
    }
    Ok(decoded)
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
mod tests {
    use super::*;

    fn message(instructions: &[UserInstruction]) -> Vec<u8> {
        UserInstruction::encode_message(instructions)
    }

    fn state(old: u64, new: u64, throwaway: u64, diff: Vec<u8>) -> ReceivedState {
        ReceivedState {
            old_num: old,
            new_num: new,
            ack_num: 0,
            throwaway_num: throwaway,
            diff,
        }
    }

    #[test]
    fn cumulative_retransmission_does_not_replay_keys() {
        let mut tracker = UserStreamTracker::new();

        let one = state(
            0,
            1,
            0,
            message(&[UserInstruction::keystroke(b"l".to_vec())]),
        );
        assert_eq!(
            tracker.accept(&one).unwrap().events,
            vec![UserEvent::Byte(b'l')]
        );

        // A newer parallel state is based on state 0 and contains the complete
        // cumulative stream "ls". Only 's' is new to the PTY.
        let two = state(
            0,
            2,
            0,
            message(&[UserInstruction::keystroke(b"ls".to_vec())]),
        );
        assert_eq!(
            tracker.accept(&two).unwrap().events,
            vec![UserEvent::Byte(b's')]
        );
    }

    #[test]
    fn out_of_order_older_state_is_retained_but_not_executed() {
        let mut tracker = UserStreamTracker::new();
        let newer = state(
            0,
            3,
            0,
            message(&[UserInstruction::keystroke(b"abc".to_vec())]),
        );
        assert_eq!(tracker.accept(&newer).unwrap().events.len(), 3);

        let older = state(
            0,
            2,
            0,
            message(&[UserInstruction::keystroke(b"ab".to_vec())]),
        );
        assert!(tracker.accept(&older).unwrap().events.is_empty());
    }

    /// A keep-alive as the client encodes it: `UserMessage.instruction`
    /// holding `Instruction` field 20, carrying the interval.  Written
    /// out by hand because the point is to track the wire, not
    /// `moshcatty`'s encoder, which has no idea the field exists.
    ///
    /// The interval is a real varint, so anything from 128 up spans two
    /// bytes: writing it as one is what an earlier version of this
    /// helper did, and it silently produced a truncated field.
    fn keep_alive(interval_ms: u32) -> Vec<u8> {
        let mut varint = Vec::new();
        let mut value = interval_ms;
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            varint.push(if value == 0 { byte } else { byte | 0x80 });
            if value == 0 {
                break;
            }
        }
        let mut instruction = vec![0xA0, 0x01];
        instruction.extend_from_slice(&varint);
        let mut message = vec![0x0A, instruction.len() as u8];
        message.extend_from_slice(&instruction);
        message
    }

    #[test]
    fn a_keep_alive_is_answered_without_reaching_the_pty() {
        let mut tracker = UserStreamTracker::new();
        let accepted = tracker.accept(&state(0, 1, 0, keep_alive(100))).unwrap();
        assert_eq!(accepted.keep_alive_ms, Some(100));
        assert!(accepted.events.is_empty(), "a keep-alive is not input");
        assert_eq!(
            tracker.committed_events, 0,
            "a keep-alive must not move the committed prefix, or every \
             later cumulative diff is skipped by one event"
        );

        // And the input either side of it still lands exactly once.
        let typed = tracker
            .accept(&state(
                1,
                2,
                0,
                message(&[UserInstruction::keystroke(b"ls".to_vec())]),
            ))
            .unwrap();
        assert_eq!(
            typed.events,
            vec![UserEvent::Byte(b'l'), UserEvent::Byte(b's')]
        );
        assert_eq!(typed.keep_alive_ms, None);
    }

    #[test]
    fn a_keep_alive_beside_typing_yields_only_the_typing() {
        let mut tracker = UserStreamTracker::new();
        // One diff carrying a keystroke instruction and a keep-alive,
        // which is what a keep-alive minted in the same tick as input
        // looks like on the wire.
        let mut diff = message(&[UserInstruction::keystroke(b"a".to_vec())]);
        diff.extend_from_slice(&keep_alive(100));
        let accepted = tracker.accept(&state(0, 1, 0, diff)).unwrap();
        assert_eq!(accepted.keep_alive_ms, Some(100));
        assert_eq!(accepted.events, vec![UserEvent::Byte(b'a')]);
        assert_eq!(tracker.committed_events, 1);
    }

    #[test]
    fn an_ordinary_diff_is_not_mistaken_for_a_keep_alive() {
        assert_eq!(keep_alive_interval(&[]), None);
        assert_eq!(
            keep_alive_interval(&message(&[UserInstruction::keystroke(b"ls -l\r".to_vec())])),
            None
        );
        assert_eq!(
            keep_alive_interval(&message(&[UserInstruction::resize(120, 40)])),
            None
        );
        // Some other extension nobody here knows: field 21, varint.
        assert_eq!(keep_alive_interval(&[0x0A, 0x03, 0xA8, 0x01, 0x01]), None);
        // Truncated rather than absent. The scan gives up; `decode_events`
        // is what reports the diff as malformed.
        assert_eq!(keep_alive_interval(&[0x0A, 0x03, 0xA0]), None);
        assert_eq!(keep_alive_interval(&[0x0A, 0x7F]), None);
    }

    #[test]
    fn a_multi_byte_interval_and_the_newest_of_several_are_read() {
        // 500 ms needs a two-byte varint, and the helper must agree with
        // the bytes PROTOCOL.md documents.
        assert_eq!(keep_alive(500), vec![0x0A, 0x04, 0xA0, 0x01, 0xF4, 0x03]);
        assert_eq!(
            keep_alive_interval(&[0x0A, 0x04, 0xA0, 0x01, 0xF4, 0x03]),
            Some(500)
        );
        // A cumulative diff can carry several. The last is the interval
        // now in force, so it is the one that wins.
        let mut diff = keep_alive(100);
        diff.extend_from_slice(&keep_alive(250));
        assert_eq!(keep_alive_interval(&diff), Some(250));
    }

    #[test]
    fn a_keep_alive_survives_a_long_keystroke_field_in_front_of_it() {
        // The walk must skip a length-delimited field by its length
        // rather than scanning through it, or a paste containing the
        // bytes 0xA0 0x01 would read as a keep-alive.
        let mut diff = message(&[UserInstruction::keystroke(vec![0xA0, 0x01, 0x09])]);
        assert_eq!(keep_alive_interval(&diff), None);
        diff.extend_from_slice(&keep_alive(100));
        assert_eq!(keep_alive_interval(&diff), Some(100));
    }

    fn terminal_response(bytes: &[u8]) -> Vec<u8> {
        let mut instruction = vec![0xAA, 0x01, bytes.len() as u8];
        instruction.extend_from_slice(bytes);
        let mut message = vec![0x0A, instruction.len() as u8];
        message.extend_from_slice(&instruction);
        message
    }

    #[test]
    fn stock_user_decoder_skips_terminal_response_extension() {
        let decoded = UserInstruction::decode_message(&terminal_response(b"response")).unwrap();
        assert_eq!(decoded, vec![UserInstruction::default()]);
    }

    #[test]
    fn terminal_responses_are_counted_and_replayed_in_stream_order() {
        let mut tracker = UserStreamTracker::new();
        let first = state(
            0,
            1,
            0,
            message(&[UserInstruction::keystroke(b"a".to_vec())]),
        );
        assert_eq!(
            tracker.accept(&first).unwrap().events,
            vec![UserEvent::Byte(b'a')]
        );

        // This newer state is cumulative from state 0. The already committed
        // key is skipped, while the response remains the next event.
        let mut diff = message(&[UserInstruction::keystroke(b"a".to_vec())]);
        diff.extend_from_slice(&terminal_response(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07"));
        let accepted = tracker.accept(&state(0, 2, 0, diff.clone())).unwrap();
        assert_eq!(
            accepted.events,
            vec![UserEvent::TerminalResponse(
                b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()
            )]
        );
        assert_eq!(tracker.committed_events, 2);

        let replay = tracker.accept(&state(0, 2, 0, diff)).unwrap();
        assert!(replay.events.is_empty());
    }

    #[test]
    fn resize_is_a_userstream_event() {
        let mut tracker = UserStreamTracker::new();
        let resize = state(0, 1, 0, message(&[UserInstruction::resize(120, 40)]));
        assert_eq!(
            tracker.accept(&resize).unwrap().events,
            vec![UserEvent::Resize {
                cols: 120,
                rows: 40
            }]
        );
    }
}
