//! Device status reports and protocol state which is independent of the grid.
//!
//! Replies retain their wire encoding, including eight-bit C1 controls. Query
//! handling never rewrites screen content or pretends to support a capability.

use super::*;

mod erase;
mod rectangles;
mod sequences;
mod sgr_stack;
mod status;
mod vt52;

#[derive(Clone)]
pub(super) struct State {
    pub c1: bool,
    sgr_stack: Vec<sgr_stack::SavedSgr>,
    pub alternate_resized: bool,
    pub tab_wrap: bool,
    pub grapheme_width: bool,
    pub saved_modes: std::collections::BTreeMap<u16, bool>,
    pub level: u8,
    pub checksum_flags: u8,
    pub attribute_extent: u8,
    pub iso_guarded: bool,
    pub erase_guarded: bool,
    pub single_shift: Option<CharsetIndex>,
    /// DECNRCM: national replacement sets may be designated.
    pub nrcs: bool,
    /// DECUDK locked the user-defined keys against redefinition.
    pub udk_locked: bool,
    /// Mode 2048: every completed resize is reported in band.
    pub in_band_resize: bool,
    /// OSC 99 notifications still waiting for their final chunk.
    pub notifications: notification::Assembler,
}

impl Default for State {
    fn default() -> Self {
        Self {
            saved_modes: Default::default(),
            c1: false,
            sgr_stack: Vec::new(),
            alternate_resized: false,
            tab_wrap: false,
            grapheme_width: true,
            level: 64,
            checksum_flags: 0,
            attribute_extent: 0,
            iso_guarded: false,
            erase_guarded: false,
            single_shift: None,
            nrcs: false,
            udk_locked: false,
            in_band_resize: false,
            notifications: Default::default(),
        }
    }
}

impl<T: EventListener> Term<T> {
    pub(super) fn write_reply(&self, reply: String) {
        if !self.protocol.c1 {
            self.event_proxy.send_event(Event::PtyWrite(reply));
            return;
        }
        let mut output = Vec::with_capacity(reply.len());
        let mut bytes = reply.bytes();
        while let Some(byte) = bytes.next() {
            if byte == 0x1b {
                match bytes.next() {
                    Some(next @ (b'P' | b'[' | b']' | b'\\')) => output.push(next + 0x40),
                    Some(next) => output.extend_from_slice(&[byte, next]),
                    None => output.push(byte),
                }
            } else {
                output.push(byte);
            }
        }
        self.event_proxy.send_event(Event::PtyWriteRaw(output));
    }

    /// Mode 2048 report: `CSI 48 ; rows ; columns ; height ; width t`. Pixel
    /// sizes belong to the frontend, which formats the reply at its geometry.
    pub(super) fn report_in_band_size(&self) {
        if !self.protocol.in_band_resize {
            return;
        }
        self.event_proxy.send_event(Event::TextAreaSizeRequest(Arc::new(|size| {
            let height = u32::from(size.num_lines) * u32::from(size.cell_height);
            let width = u32::from(size.num_cols) * u32::from(size.cell_width);
            format!("\x1b[48;{};{};{height};{width}t", size.num_lines, size.num_cols)
        })));
    }

    pub(super) fn apply_soft_reset(&mut self) {
        self.mode &= TermMode::ALT_SCREEN | TermMode::VI;
        self.mode.insert(TermMode::default());
        self.restore_output_mode();
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);
        self.horizontal_margin = None;
        self.grid.cursor.template = Cell::default();
        self.grid.cursor.charsets = Default::default();
        self.grid.cursor.input_needs_wrap = false;
        self.grid.saved_cursor = Default::default();
        self.inactive_grid.saved_cursor = Default::default();
        self.active_charset = Default::default();
        self.cursor_style = None;
        self.protocol.attribute_extent = 0;
        self.protocol.checksum_flags = 0;
        self.protocol.sgr_stack.clear();
        self.protocol.single_shift = None;
        self.protocol.tab_wrap = false;
        self.protocol.grapheme_width = true;
        self.protocol.iso_guarded = false;
        self.protocol.erase_guarded = false;
        self.mark_fully_damaged();
    }
}

#[cfg(test)]
#[path = "../../tests/term/protocol.rs"]
mod tests;
