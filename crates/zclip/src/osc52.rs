//! Recognizes standard OSC 52 clipboard writes for the Zosh relay.
//!
//! Writes need no response ticket. Reads stay outside this relay; the displaying
//! terminal remains responsible for decoding the payload and applying policy.

/// Bound the relay's scanner while accommodating large application selections.
pub const MAX_SEQUENCE_BYTES: usize = 256 * 1024;

pub fn is_write(sequence: &[u8]) -> bool {
    if sequence.len() > MAX_SEQUENCE_BYTES {
        return false;
    }
    let Some(payload) = sequence.strip_prefix(b"\x1b]52;") else {
        return false;
    };
    let Some(payload) = payload
        .strip_suffix(b"\x07")
        .or_else(|| payload.strip_suffix(b"\x1b\\"))
    else {
        return false;
    };
    let Some(separator) = payload.iter().position(|&byte| byte == b';') else {
        return false;
    };
    payload[..separator]
        .iter()
        .all(|byte| matches!(byte, b'c' | b'p' | b'q' | b's' | b'0'..=b'7'))
        && payload[separator + 1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

#[cfg(test)]
#[path = "tests/osc52.rs"]
mod tests;
