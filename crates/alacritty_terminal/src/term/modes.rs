//! Public and private terminal modes and their query responses.
//! Kept separate from ANSI dispatch so mode transitions can share side effects.

use super::*;

impl<T: EventListener> Term<T> {
    #[inline]
    fn swap_screen_without_cursor_save(&mut self) {
        let exiting = self.mode.contains(TermMode::ALT_SCREEN);
        let mut cursor = self.grid.cursor.clone();
        if exiting && self.protocol.alternate_resized {
            // Resizing reflows the hidden primary cursor with its text. Keep
            // that coordinate instead of replacing it with an alternate-grid
            // coordinate that has no corresponding primary text position.
            cursor.point = self.inactive_grid.cursor.point;
            cursor.input_needs_wrap = self.inactive_grid.cursor.input_needs_wrap;
        }
        self.swap_screen_buffers();
        self.grid.cursor = cursor;
        if !exiting {
            self.protocol.alternate_resized = false;
        }
    }

    pub(super) fn set_private_mode_inner(&mut self, mode: PrivateMode) {
        match mode.raw() {
            5 => {
                self.mode.insert(TermMode::REVERSE_VIDEO);
                self.mark_fully_damaged();
                return;
            },
            66 => {
                self.set_keypad_application_mode();
                return;
            },
            2 => {
                self.protocol.level = 61;
                self.protocol.c1 = false;
                return;
            },
            41 | 2027 => {
                if mode.raw() == 41 {
                    self.protocol.tab_wrap = true;
                } else {
                    self.protocol.grapheme_width = true;
                }
                return;
            },
            45 | 1045 => {
                self.mode.insert(if mode.raw() == 45 {
                    TermMode::REVERSE_WRAP
                } else {
                    TermMode::REVERSE_WRAP_ALL
                });
                return;
            },
            47 | 1047 => {
                if !self.mode.contains(TermMode::ALT_SCREEN) {
                    self.swap_screen_without_cursor_save();
                }
                return;
            },
            1048 => {
                self.save_cursor_position();
                return;
            },
            42 => {
                self.protocol.nrcs = true;
                return;
            },
            2048 => {
                self.protocol.in_band_resize = true;
                self.report_in_band_size();
                return;
            },
            1016 => {
                // Mouse encodings are mutually exclusive.
                self.mode.remove(TermMode::SGR_MOUSE | TermMode::UTF8_MOUSE);
                self.mode.insert(TermMode::SGR_PIXEL_MOUSE);
                return;
            },
            raw => {
                if let Some((flag, set)) = input_mode_flag(raw) {
                    self.mode.set(flag, set);
                    return;
                }
            },
        }
        if mode.raw() == 69 {
            self.horizontal_margin = Some(Column(0)..Column(self.columns()));
            return;
        }
        let mode = match mode {
            PrivateMode::Named(mode) => mode,
            PrivateMode::Unknown(9001) => {
                self.mode.insert(TermMode::WIN32_INPUT);
                return;
            },
            PrivateMode::Unknown(mode) => {
                debug!("Ignoring unknown mode {mode} in set_private_mode");
                return;
            },
        };

        trace!("Setting private mode: {mode:?}");
        match mode {
            NamedPrivateMode::UrgencyHints => self.mode.insert(TermMode::URGENCY_HINTS),
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                if !self.mode.contains(TermMode::ALT_SCREEN) {
                    self.swap_alt();
                }
            },
            NamedPrivateMode::ShowCursor => self.mode.insert(TermMode::SHOW_CURSOR),
            NamedPrivateMode::CursorKeys => self.mode.insert(TermMode::APP_CURSOR),
            // Mouse protocols are mutually exclusive.
            NamedPrivateMode::ReportMouseClicks => {
                self.mode.remove(TermMode::MOUSE_MODE);
                self.mode.insert(TermMode::MOUSE_REPORT_CLICK);
                self.event_proxy.send_event(Event::MouseCursorDirty);
            },
            NamedPrivateMode::ReportCellMouseMotion => {
                self.mode.remove(TermMode::MOUSE_MODE);
                self.mode.insert(TermMode::MOUSE_DRAG);
                self.event_proxy.send_event(Event::MouseCursorDirty);
            },
            NamedPrivateMode::ReportAllMouseMotion => {
                self.mode.remove(TermMode::MOUSE_MODE);
                self.mode.insert(TermMode::MOUSE_MOTION);
                self.event_proxy.send_event(Event::MouseCursorDirty);
            },
            NamedPrivateMode::ReportFocusInOut => self.mode.insert(TermMode::FOCUS_IN_OUT),
            NamedPrivateMode::BracketedPaste => self.mode.insert(TermMode::BRACKETED_PASTE),
            // Mouse encodings are mutually exclusive.
            NamedPrivateMode::SgrMouse => {
                self.mode.remove(TermMode::UTF8_MOUSE | TermMode::SGR_PIXEL_MOUSE);
                self.mode.insert(TermMode::SGR_MOUSE);
            },
            NamedPrivateMode::Utf8Mouse => {
                self.mode.remove(TermMode::SGR_MOUSE | TermMode::SGR_PIXEL_MOUSE);
                self.mode.insert(TermMode::UTF8_MOUSE);
            },
            NamedPrivateMode::AlternateScroll => self.mode.insert(TermMode::ALTERNATE_SCROLL),
            NamedPrivateMode::LineWrap => self.mode.insert(TermMode::LINE_WRAP),
            NamedPrivateMode::Origin => {
                self.mode.insert(TermMode::ORIGIN);
                self.goto(0, 0);
            },
            NamedPrivateMode::ColumnMode => self.deccolm(),
            NamedPrivateMode::BlinkingCursor => {
                let style = self.cursor_style.get_or_insert(self.config.default_cursor_style);
                style.blinking = true;
                self.event_proxy.send_event(Event::CursorBlinkingChange);
            },
            NamedPrivateMode::SyncUpdate => (),
        }
    }

    #[inline]
    pub(super) fn unset_private_mode_inner(&mut self, mode: PrivateMode) {
        match mode.raw() {
            5 => {
                self.mode.remove(TermMode::REVERSE_VIDEO);
                self.mark_fully_damaged();
                return;
            },
            66 => {
                self.unset_keypad_application_mode();
                return;
            },
            2 => {
                self.protocol.level = 52;
                self.protocol.c1 = false;
                self.active_charset = CharsetIndex::G0;
                self.grid.cursor.charsets = Default::default();
                self.mode.remove(TermMode::ORIGIN);
                return;
            },
            41 | 2027 => {
                if mode.raw() == 41 {
                    self.protocol.tab_wrap = false;
                } else {
                    self.protocol.grapheme_width = false;
                }
                return;
            },
            45 | 1045 => {
                self.mode.remove(if mode.raw() == 45 {
                    TermMode::REVERSE_WRAP
                } else {
                    TermMode::REVERSE_WRAP_ALL
                });
                return;
            },
            47 | 1047 => {
                if self.mode.contains(TermMode::ALT_SCREEN) {
                    self.swap_screen_without_cursor_save();
                }
                if mode.raw() == 1047 {
                    self.inactive_grid.reset_region(..);
                }
                return;
            },
            1048 => {
                self.restore_cursor_position();
                return;
            },
            42 => {
                self.protocol.nrcs = false;
                return;
            },
            2048 => {
                self.protocol.in_band_resize = false;
                return;
            },
            1016 => {
                self.mode.remove(TermMode::SGR_PIXEL_MOUSE);
                return;
            },
            raw => {
                if let Some((flag, set)) = input_mode_flag(raw) {
                    self.mode.set(flag, !set);
                    return;
                }
            },
        }
        if mode.raw() == 69 {
            self.horizontal_margin = None;
            return;
        }
        let mode = match mode {
            PrivateMode::Named(mode) => mode,
            PrivateMode::Unknown(9001) => {
                self.mode.remove(TermMode::WIN32_INPUT);
                return;
            },
            PrivateMode::Unknown(mode) => {
                debug!("Ignoring unknown mode {mode} in unset_private_mode");
                return;
            },
        };

        trace!("Unsetting private mode: {mode:?}");
        match mode {
            NamedPrivateMode::UrgencyHints => self.mode.remove(TermMode::URGENCY_HINTS),
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                if self.mode.contains(TermMode::ALT_SCREEN) {
                    self.swap_alt();
                    self.mode.set(TermMode::ORIGIN, self.grid.saved_cursor.origin);
                    self.active_charset = self.grid.saved_cursor.active_charset;
                }
                self.inactive_grid.reset_region(..);
            },
            NamedPrivateMode::ShowCursor => self.mode.remove(TermMode::SHOW_CURSOR),
            NamedPrivateMode::CursorKeys => self.mode.remove(TermMode::APP_CURSOR),
            NamedPrivateMode::ReportMouseClicks => {
                self.mode.remove(TermMode::MOUSE_REPORT_CLICK);
                self.event_proxy.send_event(Event::MouseCursorDirty);
            },
            NamedPrivateMode::ReportCellMouseMotion => {
                self.mode.remove(TermMode::MOUSE_DRAG);
                self.event_proxy.send_event(Event::MouseCursorDirty);
            },
            NamedPrivateMode::ReportAllMouseMotion => {
                self.mode.remove(TermMode::MOUSE_MOTION);
                self.event_proxy.send_event(Event::MouseCursorDirty);
            },
            NamedPrivateMode::ReportFocusInOut => self.mode.remove(TermMode::FOCUS_IN_OUT),
            NamedPrivateMode::BracketedPaste => self.mode.remove(TermMode::BRACKETED_PASTE),
            NamedPrivateMode::SgrMouse => self.mode.remove(TermMode::SGR_MOUSE),
            NamedPrivateMode::Utf8Mouse => self.mode.remove(TermMode::UTF8_MOUSE),
            NamedPrivateMode::AlternateScroll => self.mode.remove(TermMode::ALTERNATE_SCROLL),
            NamedPrivateMode::LineWrap => self.mode.remove(TermMode::LINE_WRAP),
            NamedPrivateMode::Origin => {
                self.mode.remove(TermMode::ORIGIN);
                self.goto(0, 0);
            },
            NamedPrivateMode::ColumnMode => self.deccolm(),
            NamedPrivateMode::BlinkingCursor => {
                let style = self.cursor_style.get_or_insert(self.config.default_cursor_style);
                style.blinking = false;
                self.event_proxy.send_event(Event::CursorBlinkingChange);
            },
            NamedPrivateMode::SyncUpdate => (),
        }
    }

    #[inline]
    pub(super) fn report_private_mode_inner(&mut self, mode: PrivateMode) {
        if self.protocol.level < 62 {
            return;
        }
        trace!("Reporting private mode {mode:?}");
        let state = self.private_mode_state(mode);
        self.write_reply(format!("\x1b[?{};{}$y", mode.raw(), state as u8));
    }

    fn private_mode_state(&mut self, mode: PrivateMode) -> ModeState {
        match mode {
            PrivateMode::Named(mode) => match mode {
                NamedPrivateMode::CursorKeys => self.mode.contains(TermMode::APP_CURSOR).into(),
                NamedPrivateMode::Origin => self.mode.contains(TermMode::ORIGIN).into(),
                NamedPrivateMode::LineWrap => self.mode.contains(TermMode::LINE_WRAP).into(),
                NamedPrivateMode::BlinkingCursor => {
                    let style = self.cursor_style.get_or_insert(self.config.default_cursor_style);
                    style.blinking.into()
                },
                NamedPrivateMode::ShowCursor => self.mode.contains(TermMode::SHOW_CURSOR).into(),
                NamedPrivateMode::ReportMouseClicks => {
                    self.mode.contains(TermMode::MOUSE_REPORT_CLICK).into()
                },
                NamedPrivateMode::ReportCellMouseMotion => {
                    self.mode.contains(TermMode::MOUSE_DRAG).into()
                },
                NamedPrivateMode::ReportAllMouseMotion => {
                    self.mode.contains(TermMode::MOUSE_MOTION).into()
                },
                NamedPrivateMode::ReportFocusInOut => {
                    self.mode.contains(TermMode::FOCUS_IN_OUT).into()
                },
                NamedPrivateMode::Utf8Mouse => self.mode.contains(TermMode::UTF8_MOUSE).into(),
                NamedPrivateMode::SgrMouse => self.mode.contains(TermMode::SGR_MOUSE).into(),
                NamedPrivateMode::AlternateScroll => {
                    self.mode.contains(TermMode::ALTERNATE_SCROLL).into()
                },
                NamedPrivateMode::UrgencyHints => {
                    self.mode.contains(TermMode::URGENCY_HINTS).into()
                },
                NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.mode.contains(TermMode::ALT_SCREEN).into()
                },
                NamedPrivateMode::BracketedPaste => {
                    self.mode.contains(TermMode::BRACKETED_PASTE).into()
                },
                NamedPrivateMode::SyncUpdate => ModeState::Reset,
                NamedPrivateMode::ColumnMode => ModeState::NotSupported,
            },
            // DECRQM is only answered outside VT52 mode, so DECANM is set.
            PrivateMode::Unknown(2) => ModeState::Set,
            PrivateMode::Unknown(42) => self.protocol.nrcs.into(),
            PrivateMode::Unknown(2048) => self.protocol.in_band_resize.into(),
            PrivateMode::Unknown(1016) => self.mode.contains(TermMode::SGR_PIXEL_MOUSE).into(),
            PrivateMode::Unknown(raw) if let Some((flag, set)) = input_mode_flag(raw) => {
                (self.mode.contains(flag) == set).into()
            },
            PrivateMode::Unknown(5) => self.mode.contains(TermMode::REVERSE_VIDEO).into(),
            PrivateMode::Unknown(66) => self.mode.contains(TermMode::APP_KEYPAD).into(),
            PrivateMode::Unknown(41) => self.protocol.tab_wrap.into(),
            PrivateMode::Unknown(2027) => self.protocol.grapheme_width.into(),
            PrivateMode::Unknown(45) => self.mode.contains(TermMode::REVERSE_WRAP).into(),
            PrivateMode::Unknown(1045) => self.mode.contains(TermMode::REVERSE_WRAP_ALL).into(),
            PrivateMode::Unknown(69) => self.horizontal_margin.is_some().into(),
            PrivateMode::Unknown(47 | 1047 | 1048) => {
                self.mode.contains(TermMode::ALT_SCREEN).into()
            },
            PrivateMode::Unknown(_) => ModeState::NotSupported,
        }
    }

    pub(super) fn save_private_mode(&mut self, raw: u16) {
        let state = self.private_mode_state(PrivateMode::new(raw));
        match state {
            ModeState::Set => {
                self.protocol.saved_modes.insert(raw, true);
            },
            ModeState::Reset => {
                self.protocol.saved_modes.insert(raw, false);
            },
            ModeState::NotSupported => (),
        }
    }

    pub(super) fn restore_private_mode(&mut self, raw: u16) {
        if let Some(&enabled) = self.protocol.saved_modes.get(&raw) {
            let mode = PrivateMode::new(raw);
            if enabled {
                self.set_private_mode_inner(mode);
            } else {
                self.unset_private_mode_inner(mode);
            }
        }
    }

    #[inline]
    pub(super) fn set_mode_inner(&mut self, mode: ansi::Mode) {
        match mode.raw() {
            2 => return self.mode.insert(TermMode::KEYBOARD_LOCKED),
            6 => return self.protocol.erase_guarded = true,
            12 => return self.mode.remove(TermMode::LOCAL_ECHO),
            _ => (),
        }
        let mode = match mode {
            ansi::Mode::Named(mode) => mode,
            ansi::Mode::Unknown(mode) => {
                debug!("Ignoring unknown mode {mode} in set_mode");
                return;
            },
        };

        trace!("Setting public mode: {mode:?}");
        match mode {
            NamedMode::Insert => self.mode.insert(TermMode::INSERT),
            NamedMode::LineFeedNewLine => self.mode.insert(TermMode::LINE_FEED_NEW_LINE),
        }
    }

    #[inline]
    pub(super) fn unset_mode_inner(&mut self, mode: ansi::Mode) {
        match mode.raw() {
            2 => return self.mode.remove(TermMode::KEYBOARD_LOCKED),
            6 => return self.protocol.erase_guarded = false,
            12 => return self.mode.insert(TermMode::LOCAL_ECHO),
            _ => (),
        }
        let mode = match mode {
            ansi::Mode::Named(mode) => mode,
            ansi::Mode::Unknown(mode) => {
                debug!("Ignoring unknown mode {mode} in unset_mode");
                return;
            },
        };

        trace!("Setting public mode: {mode:?}");
        match mode {
            NamedMode::Insert => {
                self.mode.remove(TermMode::INSERT);
                self.mark_fully_damaged();
            },
            NamedMode::LineFeedNewLine => self.mode.remove(TermMode::LINE_FEED_NEW_LINE),
        }
    }

    #[inline]
    pub(super) fn report_mode_inner(&mut self, mode: ansi::Mode) {
        if self.protocol.level < 62 {
            return;
        }
        trace!("Reporting mode {mode:?}");
        let state = match mode {
            ansi::Mode::Named(mode) => match mode {
                NamedMode::Insert => self.mode.contains(TermMode::INSERT).into(),
                NamedMode::LineFeedNewLine => {
                    self.mode.contains(TermMode::LINE_FEED_NEW_LINE).into()
                },
            },
            ansi::Mode::Unknown(2) => self.mode.contains(TermMode::KEYBOARD_LOCKED).into(),
            ansi::Mode::Unknown(6) => self.protocol.erase_guarded.into(),
            ansi::Mode::Unknown(12) => (!self.mode.contains(TermMode::LOCAL_ECHO)).into(),
            ansi::Mode::Unknown(_) => ModeState::NotSupported,
        };

        self.write_reply(format!("\x1b[{};{}$y", mode.raw(), state as u8,));
    }
}

/// Private modes that only change how the frontend encodes input, with the
/// flag's value while the mode is set. 1036 (metaSendsEscape) and 1039
/// (altSendsEscape) share one state because Zetta has a single Alt/Meta
/// modifier; each can still be saved on its own.
fn input_mode_flag(raw: u16) -> Option<(TermMode, bool)> {
    Some(match raw {
        67 => (TermMode::BACKARROW_SENDS_BS, true),
        1034 => (TermMode::META_SETS_EIGHTH_BIT, true),
        1036 | 1039 => (TermMode::ALT_ESC_DISABLED, false),
        _ => return None,
    })
}

/// The state of the [`Mode`] and [`PrivateMode`].
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum ModeState {
    /// The mode is not supported.
    NotSupported = 0,
    /// The mode is currently set.
    Set = 1,
    /// The mode is currently not set.
    Reset = 2,
}

impl From<bool> for ModeState {
    fn from(value: bool) -> Self {
        if value { Self::Set } else { Self::Reset }
    }
}

#[cfg(test)]
#[path = "../tests/term/modes.rs"]
mod tests;
