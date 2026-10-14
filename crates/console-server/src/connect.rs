// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! The client's CONSOLE_SERVER_MSG, sent with every connection: what
//! OpenConsole's ConsoleInitializeConnectInfo takes from it.

/// sizeof(CONSOLE_SERVER_MSG).
pub const SIZE: usize = 1340;
const PROCESS_GROUP: usize = 28;
const CONSOLE_APP: usize = 32;
const TITLE_LENGTH: usize = 34;
const TITLE: usize = 36;
const TITLE_CAPACITY: usize = 261;
const APPLICATION_LENGTH: usize = 558;
const APPLICATION: usize = 560;
const APPLICATION_CAPACITY: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Connect {
    pub process_group: u32,
    /// False for AttachConsole, which must not be told it is a console app.
    pub console_app: bool,
    pub title: String,
    /// The executable name, which aliases and command history are kept by.
    pub application: String,
}

/// Parses as much of the message as the client sent; fields past the end
/// read as empty, as ReadMessageInput leaves them zeroed.
pub fn parse(message: &[u8]) -> Connect {
    let mut bytes = [0u8; SIZE];
    let length = message.len().min(SIZE);
    bytes[..length].copy_from_slice(&message[..length]);
    let text = |offset: usize, length_offset: usize, capacity: usize| {
        let units = (usize::from(u16::from_le_bytes([
            bytes[length_offset],
            bytes[length_offset + 1],
        ])) / 2)
            .min(capacity);
        let wide: Vec<u16> = bytes[offset..offset + units * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&wide)
            .trim_end_matches('\0')
            .to_owned()
    };
    Connect {
        process_group: u32::from_le_bytes(
            bytes[PROCESS_GROUP..PROCESS_GROUP + 4].try_into().unwrap(),
        ),
        console_app: bytes[CONSOLE_APP] != 0,
        title: text(TITLE, TITLE_LENGTH, TITLE_CAPACITY),
        application: text(APPLICATION, APPLICATION_LENGTH, APPLICATION_CAPACITY),
    }
}

#[cfg(test)]
#[path = "tests/connect.rs"]
mod tests;
