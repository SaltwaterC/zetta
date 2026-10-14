// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! ConDrv wire layouts, derived from Microsoft's MIT-licensed condrv.h.
//! Tokens are opaque integers owned by this server, never dereferenced pointers.
use std::ffi::c_void;

pub const READ_IO: u32 = 0x500006;
pub const READ_INPUT: u32 = 0x50000f;
pub const WRITE_OUTPUT: u32 = 0x500013;
pub const COMPLETE_IO: u32 = 0x50000b;
pub const SET_SERVER_INFORMATION: u32 = 0x50001f;
/// Console API numbers: the ApiSorter.cpp layer in the top byte and the
/// routine's index in that layer's table below it.
pub mod api {
    pub const GET_CP: u32 = 0x0100_0000;
    pub const GET_MODE: u32 = 0x0100_0001;
    pub const SET_MODE: u32 = 0x0100_0002;
    pub const GET_NUMBER_OF_INPUT_EVENTS: u32 = 0x0100_0003;
    pub const GET_CONSOLE_INPUT: u32 = 0x0100_0004;
    pub const READ_CONSOLE: u32 = 0x0100_0005;
    pub const WRITE_CONSOLE: u32 = 0x0100_0006;
    pub const GET_LANG_ID: u32 = 0x0100_0008;
    pub const FILL_CONSOLE_OUTPUT: u32 = 0x0200_0000;
    pub const GENERATE_CTRL_EVENT: u32 = 0x0200_0001;
    pub const SET_ACTIVE_SCREEN_BUFFER: u32 = 0x0200_0002;
    pub const FLUSH_INPUT_BUFFER: u32 = 0x0200_0003;
    pub const SET_CP: u32 = 0x0200_0004;
    pub const GET_CURSOR_INFO: u32 = 0x0200_0005;
    pub const SET_CURSOR_INFO: u32 = 0x0200_0006;
    pub const GET_SCREEN_BUFFER_INFO: u32 = 0x0200_0007;
    pub const SET_SCREEN_BUFFER_INFO: u32 = 0x0200_0008;
    pub const SET_SCREEN_BUFFER_SIZE: u32 = 0x0200_0009;
    pub const SET_CURSOR_POSITION: u32 = 0x0200_000a;
    pub const GET_LARGEST_WINDOW_SIZE: u32 = 0x0200_000b;
    pub const SCROLL_SCREEN_BUFFER: u32 = 0x0200_000c;
    pub const SET_TEXT_ATTRIBUTE: u32 = 0x0200_000d;
    pub const SET_WINDOW_INFO: u32 = 0x0200_000e;
    pub const READ_OUTPUT_STRING: u32 = 0x0200_000f;
    pub const WRITE_CONSOLE_INPUT: u32 = 0x0200_0010;
    pub const WRITE_CONSOLE_OUTPUT: u32 = 0x0200_0011;
    pub const WRITE_OUTPUT_STRING: u32 = 0x0200_0012;
    pub const READ_CONSOLE_OUTPUT: u32 = 0x0200_0013;
    pub const GET_TITLE: u32 = 0x0200_0014;
    pub const SET_TITLE: u32 = 0x0200_0015;
    pub const GET_MOUSE_INFO: u32 = 0x0300_0001;
    pub const GET_FONT_SIZE: u32 = 0x0300_0003;
    pub const GET_CURRENT_FONT: u32 = 0x0300_0004;
    pub const SET_DISPLAY_MODE: u32 = 0x0300_000d;
    pub const GET_DISPLAY_MODE: u32 = 0x0300_0011;
    pub const ADD_ALIAS: u32 = 0x0300_0012;
    pub const GET_ALIAS: u32 = 0x0300_0013;
    pub const GET_ALIASES_LENGTH: u32 = 0x0300_0014;
    pub const GET_ALIAS_EXES_LENGTH: u32 = 0x0300_0015;
    pub const GET_ALIASES: u32 = 0x0300_0016;
    pub const GET_ALIAS_EXES: u32 = 0x0300_0017;
    pub const EXPUNGE_COMMAND_HISTORY: u32 = 0x0300_0018;
    pub const SET_NUMBER_OF_COMMANDS: u32 = 0x0300_0019;
    pub const GET_COMMAND_HISTORY_LENGTH: u32 = 0x0300_001a;
    pub const GET_COMMAND_HISTORY: u32 = 0x0300_001b;
    pub const GET_CONSOLE_WINDOW: u32 = 0x0300_001f;
    pub const GET_SELECTION_INFO: u32 = 0x0300_0028;
    pub const GET_PROCESS_LIST: u32 = 0x0300_0029;
    pub const GET_HISTORY: u32 = 0x0300_002a;
    pub const SET_HISTORY: u32 = 0x0300_002b;
    pub const SET_CURRENT_FONT: u32 = 0x0300_002c;
}
pub const INVALID_PARAMETER: i32 = 0xc000000du32 as i32;
pub const INVALID_HANDLE: i32 = 0xc0000008u32 as i32;
pub const NOT_SUPPORTED: i32 = 0xc00000bbu32 as i32;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Identifier {
    pub low: u32,
    pub high: i32,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Descriptor {
    pub id: Identifier,
    pub process: usize,
    pub object: usize,
    pub function: u32,
    pub input_size: u32,
    pub output_size: u32,
    pub reserved: u32,
}
#[repr(C)]
pub struct Packet {
    pub descriptor: Descriptor,
    pub payload: [u8; 4096],
}
impl Default for Packet {
    fn default() -> Self {
        Self {
            descriptor: Descriptor::default(),
            payload: [0; 4096],
        }
    }
}
#[repr(C)]
#[derive(Default)]
pub struct Buffer {
    pub data: *mut c_void,
    pub size: u32,
    pub offset: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct IoStatus {
    pub status: usize,
    pub information: usize,
}
#[repr(C)]
#[derive(Default)]
pub struct Completion {
    pub id: Identifier,
    pub status: IoStatus,
    pub write: Buffer,
}
impl Completion {
    pub fn new(id: Identifier, status: i32, information: usize, bytes: &mut [u8]) -> Self {
        Self {
            id,
            status: IoStatus {
                status: status as u32 as usize,
                information,
            },
            write: Buffer {
                data: bytes.as_mut_ptr().cast(),
                size: bytes.len() as u32,
                offset: 0,
            },
        }
    }
}
#[repr(C)]
pub struct Operation {
    pub id: Identifier,
    pub buffer: Buffer,
}
pub fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
pub fn set_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
pub fn set_i16(bytes: &mut [u8], offset: usize, value: i16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
pub fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
pub fn i16_at(bytes: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
pub fn set_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
#[cfg(test)]
#[path = "tests/protocol.rs"]
mod tests;
