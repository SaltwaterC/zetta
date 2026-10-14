//! Console code pages. ANSI APIs and raw byte writes are in the console's
//! code page, and the embedder's parser only understands UTF-8, so non-UTF-8
//! text is converted here. A character split across two writes is carried
//! over, as OpenConsole carries a DBCS lead byte or a partial UTF-8 sequence.
use windows_sys::Win32::Globalization::{
    CPINFO, GetCPInfo, GetOEMCP, IsDBCSLeadByteEx, IsValidCodePage, MB_USEGLYPHCHARS,
    MultiByteToWideChar, WideCharToMultiByte,
};

pub const UTF8: u32 = 65001;

/// The code page a new console starts with: ConPTY inherits the OEM one.
pub fn default() -> u32 {
    unsafe { GetOEMCP() }
}

pub fn is_valid(code_page: u32) -> bool {
    code_page == UTF8 || unsafe { IsValidCodePage(code_page) } != 0
}

/// Whether bytes below 0x80 decode to themselves, so pure ASCII can skip
/// conversion. False for EBCDIC and similar pages.
pub fn ascii_compatible(code_page: u32) -> bool {
    if code_page == UTF8 {
        return true;
    }
    let bytes: Vec<u8> = (0..0x80).collect();
    to_utf16(code_page, &bytes)
        == bytes
            .iter()
            .map(|&byte| u16::from(byte))
            .collect::<Vec<_>>()
}

fn max_char_size(code_page: u32) -> u32 {
    let mut info: CPINFO = unsafe { std::mem::zeroed() };
    if unsafe { GetCPInfo(code_page, &mut info) } == 0 {
        return 1;
    }
    info.MaxCharSize
}

/// The OEM glyph a control byte is shown as when written with processed
/// output and VT processing off; None where the code page has none.
pub fn glyph(code_page: u32, byte: u8) -> Option<char> {
    let mut wide = 0u16;
    let converted =
        unsafe { MultiByteToWideChar(code_page, MB_USEGLYPHCHARS, &byte, 1, &mut wide, 1) };
    (converted == 1 && wide != 0)
        .then(|| char::from_u32(u32::from(wide)))
        .flatten()
}

/// Bytes in `code_page` as UTF-16. Callers pass only complete characters.
pub fn to_utf16(code_page: u32, bytes: &[u8]) -> Vec<u16> {
    if bytes.is_empty() {
        return Vec::new();
    }
    if code_page == UTF8 {
        return String::from_utf8_lossy(bytes).encode_utf16().collect();
    }
    let length = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    let needed = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
        )
    };
    let mut wide = vec![0u16; needed.max(0) as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            length,
            wide.as_mut_ptr(),
            needed,
        )
    };
    wide.truncate(written.max(0) as usize);
    wide
}

/// UTF-16 as bytes in `code_page`, for ANSI reads and titles.
pub fn from_utf16(code_page: u32, text: &[u16]) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    if code_page == UTF8 {
        return String::from_utf16_lossy(text).into_bytes();
    }
    let length = i32::try_from(text.len()).unwrap_or(i32::MAX);
    let needed = unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            text.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    let mut bytes = vec![0u8; needed.max(0) as usize];
    let written = unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            text.as_ptr(),
            length,
            bytes.as_mut_ptr(),
            needed,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    bytes.truncate(written.max(0) as usize);
    bytes
}

/// Converts a byte stream in the output code page, holding back a trailing
/// partial character until the next write completes it.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    code_page: u32,
    double_byte: bool,
}
impl Decoder {
    /// No partial character is waiting for the next write.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }
    pub fn decode(&mut self, code_page: u32, bytes: &[u8]) -> Vec<u16> {
        if code_page != self.code_page {
            // A code page change abandons a character the old one began.
            self.pending.clear();
            self.code_page = code_page;
            self.double_byte = code_page != UTF8 && max_char_size(code_page) > 1;
        }
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let complete = if code_page == UTF8 {
            utf8_complete(&data)
        } else if self.double_byte {
            dbcs_complete(code_page, &data)
        } else {
            data.len()
        };
        self.pending = data.split_off(complete);
        to_utf16(code_page, &data)
    }
}

/// Length of `data` without a trailing UTF-8 sequence that more bytes could
/// still complete; invalid bytes count as complete (they decode to U+FFFD).
fn utf8_complete(data: &[u8]) -> usize {
    match std::str::from_utf8(data) {
        Ok(_) => data.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => {
            // An invalid sequence earlier on; only an incomplete tail is held.
            let tail = data.len().saturating_sub(3);
            (tail..data.len())
                .find(|&start| {
                    std::str::from_utf8(&data[start..])
                        .err()
                        .is_some_and(|error| {
                            error.valid_up_to() == 0 && error.error_len().is_none()
                        })
                })
                .unwrap_or(data.len())
        }
    }
}

/// Length of `data` without a trailing DBCS lead byte. Lead and trail byte
/// ranges overlap, so the scan has to start from a character boundary.
fn dbcs_complete(code_page: u32, data: &[u8]) -> usize {
    let mut index = 0;
    while index < data.len() {
        if unsafe { IsDBCSLeadByteEx(code_page, data[index]) } != 0 {
            if index + 1 == data.len() {
                return index;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    data.len()
}

#[cfg(test)]
#[path = "tests/codepage.rs"]
mod tests;
