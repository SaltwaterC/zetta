//! ANSI operations and their ordered recording during asynchronous reflow.
//! Record only the outermost operation: many ANSI methods call other methods.

use super::*;
use crate::vte::ansi::SemanticPromptMark;

impl<T: EventListener> Handler for Term<T> {
    fn vt52_mode(&self) -> bool {
        self.protocol.level == 52
    }
    fn csi_extension(
        &mut self,
        params: &crate::vte::Params,
        intermediates: &[u8],
        action: char,
    ) -> bool {
        self.dispatch_csi_extension(params, intermediates, action)
    }

    fn esc_extension(&mut self, intermediates: &[u8], byte: u8) -> bool {
        self.dispatch_esc_extension(intermediates, byte)
    }

    fn dcs_sequence(
        &mut self,
        params: &crate::vte::Params,
        intermediates: &[u8],
        action: char,
        data: &[u8],
    ) {
        self.handle_dcs(params, intermediates, action, data);
    }

    fn soft_reset(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.soft_reset());
        self.apply_soft_reset();
    }

    fn set_horizontal_margins(&mut self, left: usize, right: usize) -> bool {
        let _recording = self.reflow.record(|| {
            move |term| {
                term.set_horizontal_margins(left, right);
            }
        });
        self.horizontal_margins(left, right)
    }

    /// A character to be displayed.
    #[inline(never)]
    fn input(&mut self, c: char) {
        let _recording = self.reflow.record_char(c);
        // Number of cells the char will occupy.
        let width = match c.width() {
            Some(width) => width,
            None => return,
        };

        if self.extend_grapheme(c) {
            return;
        }

        // Prepend characters start a cluster despite having no standalone
        // glyph width; the following base determines its presentation.
        let width = if width == 0 && grapheme::is_prepend(c) { 1 } else { width };
        // Handle zero-width characters.
        if width == 0 {
            // Get previous column.
            let mut column = self.grid.cursor.point.column;
            if !self.grid.cursor.input_needs_wrap {
                column.0 = column.saturating_sub(1);
            }

            // Put zerowidth characters over first fullwidth character cell.
            let line = self.grid.cursor.point.line;
            if self.grid[line][column].flags.contains(Flags::WIDE_CHAR_SPACER) {
                column.0 = column.saturating_sub(1);
            }

            if self.grid[line][column].zerowidth().map_or(0, <[char]>::len) < 23 {
                self.grid[line][column].push_zerowidth(c);
            }
            return;
        }

        // A two-cell glyph cannot have a trailing spacer in a one-column
        // grid. Keep its scalar in one cell after wrapping, so a narrow pane
        // remains usable without advancing the cursor beyond its only column.
        if width > 1 && self.columns() == 1 {
            if self.mode.contains(TermMode::LINE_WRAP) {
                self.wrapline();
                self.write_at_cursor(c);
            }
            self.grid.cursor.input_needs_wrap = true;
            return;
        }

        // Move cursor to next line.
        if self.grid.cursor.input_needs_wrap {
            self.wrapline();
        }
        self.clamp_cursor_to_line_size();

        // If in insert mode, first shift cells to the right.
        let columns = self.input_right_edge();
        if self.mode.contains(TermMode::INSERT) && self.grid.cursor.point.column + width < columns {
            self.insert_blank_cells(width);
        }

        if width == 1 {
            self.write_at_cursor(c);
        } else {
            if self.grid.cursor.point.column + 1 >= columns {
                if self.mode.contains(TermMode::LINE_WRAP) {
                    // Insert placeholder before wide char if glyph does not fit in this row.
                    self.grid.cursor.template.flags.insert(Flags::LEADING_WIDE_CHAR_SPACER);
                    self.write_at_cursor(' ');
                    self.grid.cursor.template.flags.remove(Flags::LEADING_WIDE_CHAR_SPACER);
                    self.wrapline();
                } else {
                    // Prevent out of bounds crash when linewrapping is disabled.
                    self.grid.cursor.input_needs_wrap = true;
                    return;
                }
            }

            // Write full width glyph to current cursor cell.
            self.grid.cursor.template.flags.insert(Flags::WIDE_CHAR);
            self.write_at_cursor(c);
            self.grid.cursor.template.flags.remove(Flags::WIDE_CHAR);

            // Write spacer to cell following the wide glyph.
            self.grid.cursor.point.column += 1;
            self.grid.cursor.template.flags.insert(Flags::WIDE_CHAR_SPACER);
            self.write_at_cursor(' ');
            self.grid.cursor.template.flags.remove(Flags::WIDE_CHAR_SPACER);
        }

        if self.grid.cursor.point.column + 1 < columns {
            self.grid.cursor.point.column += 1;
        } else {
            self.grid.cursor.input_needs_wrap = true;
        }
    }

    fn repeat_char(&mut self, c: char, count: u16) {
        if UnicodeWidthChar::width(c).unwrap_or(0) == 0 {
            return;
        }
        for _ in 0..count {
            self.input(c);
        }
    }

    /// Zetta patch: a run of plain text is written a row segment at a time.
    /// Upstream called `input` per character, which made it the largest cost of
    /// reading ordinary output, and of binary output the replacement characters
    /// the parser now folds into these runs.
    #[inline]
    fn input_ascii(&mut self, run: &[u8]) {
        let _recording = self.reflow.record_ascii(run);
        // Insert mode shifts the row per character, a line-drawing charset maps
        // each one, and without autowrap every character past the margin lands
        // on the last column. All three are rare; take the per-character path.
        if self.mode.contains(TermMode::INSERT)
            || !self.mode.contains(TermMode::LINE_WRAP)
            || self.protocol.single_shift.is_some()
            || self.grid.cursor.charsets[self.active_charset] != StandardCharset::Ascii
        {
            for &byte in run {
                self.input(ascii_run_char(byte));
            }
            return;
        }

        // Every character of a run, U+FFFD included, is one cell wide. ASCII
        // joins nothing but a Prepend printed just before it; see `grapheme.rs`.
        let mut bytes = run;
        if self.prepend_pending
            && let Some((&first, rest)) = bytes.split_first()
            && self.extend_grapheme(crate::vte::ascii_run_char(first))
        {
            bytes = rest;
        }
        while !bytes.is_empty() {
            if self.grid.cursor.input_needs_wrap {
                self.wrapline();
            }
            // Each row the run reaches may be a different size.
            self.clamp_cursor_to_line_size();
            let columns = self.input_right_edge();

            let column = self.grid.cursor.point.column.0;
            let count = bytes.len().min(columns - column);
            let (segment, rest) = bytes.split_at(count);
            self.write_ascii_at_cursor(segment);
            bytes = rest;

            if column + count < columns {
                self.grid.cursor.point.column = Column(column + count);
            } else {
                self.grid.cursor.point.column = Column(columns - 1);
                self.grid.cursor.input_needs_wrap = true;
            }
        }
    }

    #[inline]
    fn decaln(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.decaln());
        trace!("Decalnning");
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);
        if self.horizontal_margin.is_some() {
            self.horizontal_margin = Some(Column(0)..Column(self.columns()));
        }
        self.mode.remove(TermMode::ORIGIN);
        self.goto(0, 0);

        for line in (0..self.screen_lines()).map(Line::from) {
            self.reset_line_size(line);
            for column in 0..self.columns() {
                let cell = &mut self.grid[line][Column(column)];
                *cell = Cell::default();
                cell.c = 'E';
                cell.flags.insert(Flags::DRAWN);
            }
        }

        self.mark_fully_damaged();
    }

    #[inline]
    fn goto(&mut self, line: i32, col: usize) {
        let _recording = self.reflow.record(|| move |term| term.goto(line, col));
        let line = Line(line);
        let col = Column(col);

        trace!("Going to: line={line}, col={col}");
        let (y_offset, max_y) = if self.mode.contains(TermMode::ORIGIN) {
            (self.scroll_region.start, self.scroll_region.end - 1)
        } else {
            (Line(0), self.bottommost_line())
        };

        self.damage_cursor();
        self.grid.cursor.point.line = cmp::max(cmp::min(line + y_offset, max_y), Line(0));
        let horizontal = if self.mode.contains(TermMode::ORIGIN) {
            self.horizontal_bounds()
        } else {
            Column(0)..Column(self.columns())
        };
        self.grid.cursor.point.column = (horizontal.start + col).min(horizontal.end - 1);
        self.damage_cursor();
        self.grid.cursor.input_needs_wrap = false;
    }

    #[inline]
    fn goto_line(&mut self, line: i32) {
        let _recording = self.reflow.record(|| move |term| term.goto_line(line));
        trace!("Going to line: {line}");
        let offset =
            if self.mode.contains(TermMode::ORIGIN) { self.horizontal_bounds().start.0 } else { 0 };
        self.goto(line, self.grid.cursor.point.column.0.saturating_sub(offset))
    }

    #[inline]
    fn goto_col(&mut self, col: usize) {
        let _recording = self.reflow.record(|| move |term| term.goto_col(col));
        trace!("Going to column: {col}");
        let offset =
            if self.mode.contains(TermMode::ORIGIN) { self.scroll_region.start.0 } else { 0 };
        self.goto(self.grid.cursor.point.line.0 - offset, col)
    }

    #[inline]
    fn insert_blank(&mut self, count: usize) {
        let _recording = self.reflow.record(|| move |term| term.insert_blank(count));
        self.insert_blank_cells(count);
    }

    #[inline]
    fn move_up(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.move_up(lines));
        trace!("Moving up: {lines}");

        self.move_vertical(-(lines.min(i32::MAX as usize) as i32));
    }

    #[inline]
    fn move_down(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.move_down(lines));
        trace!("Moving down: {lines}");

        self.move_vertical(lines.min(i32::MAX as usize) as i32);
    }

    #[inline]
    fn move_forward(&mut self, cols: usize) {
        let _recording = self.reflow.record(|| move |term| term.move_forward(cols));
        trace!("Moving forward: {cols}");
        let right = if self.horizontal_bounds().contains(&self.grid.cursor.point.column) {
            self.horizontal_bounds().end - 1
        } else {
            self.last_column()
        };
        let last_column = cmp::min(self.grid.cursor.point.column + cols, right);

        let cursor_line = self.grid.cursor.point.line.0 as usize;
        self.damage.damage_line(cursor_line, self.grid.cursor.point.column.0, last_column.0);

        self.grid.cursor.point.column = last_column;
        self.grid.cursor.input_needs_wrap = false;
    }

    #[inline]
    fn move_backward(&mut self, cols: usize) {
        let _recording = self.reflow.record(|| move |term| term.move_backward(cols));
        trace!("Moving backward: {cols}");
        if self.mode.contains(TermMode::LINE_WRAP)
            && self.mode.intersects(TermMode::REVERSE_WRAP | TermMode::REVERSE_WRAP_ALL)
        {
            self.move_backward_wrapping(cols);
            return;
        }
        let left = if self.horizontal_bounds().contains(&self.grid.cursor.point.column) {
            self.horizontal_bounds().start.0
        } else {
            0
        };
        let column = self.grid.cursor.point.column.saturating_sub(cols).max(left);

        let cursor_line = self.grid.cursor.point.line.0 as usize;
        self.damage.damage_line(cursor_line, column, self.grid.cursor.point.column.0);

        self.grid.cursor.point.column = Column(column);
        self.grid.cursor.input_needs_wrap = false;
    }

    #[inline]
    fn identify_terminal(&mut self, intermediate: Option<char>) {
        match intermediate {
            None => {
                trace!("Reporting primary device attributes");
                self.write_reply(String::from(PRIMARY_DEVICE_ATTRIBUTES));
            },
            Some('>') => {
                trace!("Reporting secondary device attributes");
                let version = version_number(env!("CARGO_PKG_VERSION"));
                let text = format!("\x1b[>0;{version};1c");
                self.write_reply(text);
            },
            _ => debug!("Unsupported device attributes intermediate"),
        }
    }

    #[inline]
    fn report_keyboard_mode(&mut self) {
        if !self.config.kitty_keyboard {
            return;
        }

        trace!("Reporting active keyboard mode");
        let current_mode =
            self.keyboard_mode_stack.last().unwrap_or(&KeyboardModes::NO_MODE).bits();
        let text = format!("\x1b[?{current_mode}u");
        self.write_reply(text);
    }

    #[inline]
    fn push_keyboard_mode(&mut self, mode: KeyboardModes) {
        let _recording = self.reflow.record(|| move |term| term.push_keyboard_mode(mode));
        if !self.config.kitty_keyboard {
            return;
        }

        trace!("Pushing `{mode:?}` keyboard mode into the stack");

        if self.keyboard_mode_stack.len() >= KEYBOARD_MODE_STACK_MAX_DEPTH {
            let removed = self.title_stack.remove(0);
            trace!(
                "Removing '{removed:?}' from bottom of keyboard mode stack that exceeds its \
                 maximum depth"
            );
        }

        self.keyboard_mode_stack.push(mode);
        self.set_keyboard_mode(mode.into(), KeyboardModesApplyBehavior::Replace);
    }

    #[inline]
    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        let _recording = self.reflow.record(|| move |term| term.pop_keyboard_modes(to_pop));
        if !self.config.kitty_keyboard {
            return;
        }

        trace!("Attempting to pop {to_pop} keyboard modes from the stack");
        let new_len = self.keyboard_mode_stack.len().saturating_sub(to_pop as usize);
        self.keyboard_mode_stack.truncate(new_len);

        // Reload active mode.
        let mode = self.keyboard_mode_stack.last().copied().unwrap_or(KeyboardModes::NO_MODE);
        self.set_keyboard_mode(mode.into(), KeyboardModesApplyBehavior::Replace);
    }

    #[inline]
    fn set_keyboard_mode(&mut self, mode: KeyboardModes, apply: KeyboardModesApplyBehavior) {
        let _recording =
            self.reflow.record(|| move |term| Handler::set_keyboard_mode(term, mode, apply));
        if !self.config.kitty_keyboard {
            return;
        }

        self.set_keyboard_mode(mode.into(), apply);
    }

    #[inline]
    fn device_status(&mut self, arg: usize) {
        trace!("Reporting device status: {arg}");
        match arg {
            5 => {
                let text = String::from("\x1b[0n");
                self.write_reply(text);
            },
            6 => {
                let pos = self.report_cursor_position();
                let text = format!("\x1b[{};{}R", pos.line + 1, pos.column + 1);
                self.write_reply(text);
            },
            _ => debug!("unknown device status query: {arg}"),
        };
    }

    #[inline]
    fn move_down_and_cr(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.move_down_and_cr(lines));
        trace!("Moving down and cr: {lines}");

        self.move_down(lines);
        self.carriage_return();
    }

    #[inline]
    fn move_up_and_cr(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.move_up_and_cr(lines));
        trace!("Moving up and cr: {lines}");

        self.move_up(lines);
        self.carriage_return();
    }

    /// Insert tab at cursor position.
    #[inline]
    fn put_tab(&mut self, mut count: u16) {
        let _recording = self.reflow.record(|| move |term| term.put_tab(count));
        // HT does not trigger delayed autowrap unless the xterm workaround
        // is enabled, and must not cancel a pending wrap for the next glyph.
        if self.grid.cursor.input_needs_wrap {
            if self.protocol.tab_wrap {
                self.wrapline();
            }
            return;
        }

        while self.grid.cursor.point.column < self.columns() && count != 0 {
            count -= 1;

            let c = self.grid.cursor.charsets[self.active_charset].map('\t');
            let cell = self.grid.cursor_cell();
            if cell.c == ' ' {
                cell.c = c;
            }

            loop {
                if (self.grid.cursor.point.column + 1) == self.columns() {
                    break;
                }

                self.grid.cursor.point.column += 1;

                if self.tabs[self.grid.cursor.point.column] {
                    break;
                }
            }
        }
    }

    /// Backspace.
    #[inline]
    fn backspace(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.backspace());
        trace!("Backspace");

        if self.grid.cursor.input_needs_wrap {
            self.grid.cursor.input_needs_wrap = false;
            if self.mode.intersects(TermMode::REVERSE_WRAP | TermMode::REVERSE_WRAP_ALL) {
                return;
            }
        }
        if self.mode.contains(TermMode::LINE_WRAP)
            && self.mode.intersects(TermMode::REVERSE_WRAP | TermMode::REVERSE_WRAP_ALL)
        {
            self.move_backward_wrapping(1);
            return;
        }
        let left = if self.grid.cursor.point.column >= self.horizontal_bounds().start {
            self.horizontal_bounds().start
        } else {
            Column(0)
        };
        if self.grid.cursor.point.column > left {
            let line = self.grid.cursor.point.line.0 as usize;
            let column = self.grid.cursor.point.column.0;
            self.grid.cursor.point.column -= 1;
            self.grid.cursor.input_needs_wrap = false;
            self.damage.damage_line(line, column - 1, column);
        }
    }

    /// Carriage return.
    #[inline]
    fn carriage_return(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.carriage_return());
        trace!("Carriage return");
        let left = self.horizontal_bounds().start.0;
        let new_col = if self.grid.cursor.point.column >= left { left } else { 0 };
        let line = self.grid.cursor.point.line.0 as usize;
        self.damage.damage_line(line, new_col, self.grid.cursor.point.column.0);
        self.grid.cursor.point.column = Column(new_col);
        self.grid.cursor.input_needs_wrap = false;
    }

    /// Linefeed.
    #[inline]
    fn linefeed(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.linefeed());
        trace!("Linefeed");
        // Like xterm's CursorDown, every line feed (LF, VT, FF, IND) cancels
        // a deferred wrap: the next graphic lands in the last column below.
        self.grid.cursor.input_needs_wrap = false;
        let next = self.grid.cursor.point.line + 1;
        if next == self.scroll_region.end {
            if self.horizontal_bounds().contains(&self.grid.cursor.point.column) {
                self.scroll_up(1);
            }
        } else if next < self.screen_lines() {
            self.damage_cursor();
            self.grid.cursor.point.line += 1;
            self.damage_cursor();
        }
    }

    fn linefeed_with_mode(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.linefeed_with_mode());
        self.linefeed();
        if self.mode.intersects(TermMode::LF_IMPLIES_CR | TermMode::LINE_FEED_NEW_LINE) {
            self.carriage_return();
        }
    }

    /// Set current position as a tabstop.
    #[inline]
    fn bell(&mut self) {
        trace!("Bell");
        self.event_proxy.send_event(Event::Bell);
    }

    #[inline]
    fn substitute(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.substitute());
        trace!("[unimplemented] Substitute");
    }

    /// Run LF/NL.
    ///
    /// LF/NL mode has some interesting history. According to ECMA-48 4th
    /// edition, in LINE FEED mode,
    ///
    /// > The execution of the formatter functions LINE FEED (LF), FORM FEED
    /// > (FF), LINE TABULATION (VT) cause only movement of the active position in
    /// > the direction of the line progression.
    ///
    /// In NEW LINE mode,
    ///
    /// > The execution of the formatter functions LINE FEED (LF), FORM FEED
    /// > (FF), LINE TABULATION (VT) cause movement to the line home position on
    /// > the following line, the following form, etc. In the case of LF this is
    /// > referred to as the New Line (NL) option.
    ///
    /// Additionally, ECMA-48 4th edition says that this option is deprecated.
    /// ECMA-48 5th edition only mentions this option (without explanation)
    /// saying that it's been removed.
    ///
    /// As an emulator, we need to support it since applications may still rely
    /// on it.
    #[inline]
    fn newline(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.newline());
        self.linefeed();

        if self.mode.contains(TermMode::LINE_FEED_NEW_LINE) {
            self.carriage_return();
        }
    }

    #[inline]
    fn set_horizontal_tabstop(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.set_horizontal_tabstop());
        trace!("Setting horizontal tabstop");
        self.tabs[self.grid.cursor.point.column] = true;
    }

    #[inline]
    fn scroll_up(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.scroll_up(lines));
        let origin = self.scroll_region.start;
        self.scroll_up_relative(origin, lines);
    }

    #[inline]
    fn scroll_down(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.scroll_down(lines));
        let origin = self.scroll_region.start;
        self.scroll_down_relative(origin, lines);
    }

    #[inline]
    fn insert_blank_lines(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.insert_blank_lines(lines));
        trace!("Inserting blank {lines} lines");

        let origin = self.grid.cursor.point.line;
        if self.scroll_region.contains(&origin) {
            self.scroll_down_relative(origin, lines);
            self.carriage_return();
        }
    }

    #[inline]
    fn delete_lines(&mut self, lines: usize) {
        let _recording = self.reflow.record(|| move |term| term.delete_lines(lines));
        let origin = self.grid.cursor.point.line;
        let lines = cmp::min(self.screen_lines() - origin.0 as usize, lines);

        trace!("Deleting {lines} lines");

        if lines > 0 && self.scroll_region.contains(&origin) {
            self.scroll_up_relative(origin, lines);
            self.carriage_return();
        }
    }

    #[inline]
    fn erase_chars(&mut self, count: usize) {
        let _recording = self.reflow.record(|| move |term| term.erase_chars(count));
        trace!("Erasing chars: count={}, col={}", count, self.grid.cursor.point.column);
        if !self.erase_guarded_chars(count) {
            self.erase_chars_cells(count);
        }
    }

    #[inline]
    fn delete_chars(&mut self, count: usize) {
        let _recording = self.reflow.record(|| move |term| term.delete_chars(count));
        self.delete_chars_cells(count);
    }

    #[inline]
    fn move_backward_tabs(&mut self, count: u16) {
        let _recording = self.reflow.record(|| move |term| term.move_backward_tabs(count));
        let left =
            if self.mode.contains(TermMode::ORIGIN) { self.horizontal_bounds().start.0 } else { 0 };
        self.damage_cursor();
        for _ in 0..count {
            let mut column = self.grid.cursor.point.column.0.saturating_sub(1).max(left);
            while column > left && !self.tabs[Column(column)] {
                column -= 1;
            }
            self.grid.cursor.point.column = Column(column);
        }
        self.grid.cursor.input_needs_wrap = false;
        self.damage_cursor();
    }

    #[inline]
    fn move_forward_tabs(&mut self, count: u16) {
        let _recording = self.reflow.record(|| move |term| term.move_forward_tabs(count));
        trace!("Moving forward {count} tabs");

        let num_cols = self.columns();
        let old_col = self.grid.cursor.point.column.0;
        for _ in 0..count {
            let mut col = self.grid.cursor.point.column;

            if col == num_cols - 1 {
                break;
            }

            for i in col.0 + 1..num_cols {
                col = index::Column(i);
                if self.tabs[col] {
                    break;
                }
            }

            self.grid.cursor.point.column = col;
        }

        let line = self.grid.cursor.point.line.0 as usize;
        self.damage.damage_line(line, old_col, self.grid.cursor.point.column.0);
    }

    #[inline]
    fn save_cursor_position(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.save_cursor_position());
        trace!("Saving cursor position");

        self.grid.saved_cursor = self.grid.cursor.clone();
        self.grid.saved_cursor.origin = self.mode.contains(TermMode::ORIGIN);
        self.grid.saved_cursor.active_charset = self.active_charset;
    }

    #[inline]
    fn restore_cursor_position(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.restore_cursor_position());
        trace!("Restoring cursor position");

        self.damage_cursor();
        self.grid.cursor = self.grid.saved_cursor.clone();
        self.mode.set(TermMode::ORIGIN, self.grid.cursor.origin);
        self.active_charset = self.grid.cursor.active_charset;
        self.damage_cursor();
    }

    #[inline]
    fn clear_line(&mut self, mode: ansi::LineClearMode) {
        let _recording = self.reflow.record(|| move |term| term.clear_line(mode));
        trace!("Clearing line: {mode:?}");
        // Every EL cancels a deferred wrap. At the last column EL 0 erases
        // that column, as in xterm, Ghostty and xterm.js.
        self.grid.cursor.input_needs_wrap = false;
        if self.erase_guarded_line(mode) {
            return;
        }

        let cursor = &self.grid.cursor;
        let blank = cursor.template.erased();
        let point = cursor.point;

        let (left, right) = match mode {
            ansi::LineClearMode::Right => (point.column, Column(self.columns())),
            ansi::LineClearMode::Left => (Column(0), point.column + 1),
            ansi::LineClearMode::All => (Column(0), Column(self.columns())),
        };

        self.clear_edit_boundary(point.line, left.0);
        self.clear_edit_boundary(point.line, right.0);
        self.damage.damage_line(point.line.0 as usize, left.0, right.0 - 1);

        let row = &mut self.grid[point.line];
        for cell in &mut row[left..right] {
            *cell = blank.clone();
        }

        let range = self.grid.cursor.point.line..=self.grid.cursor.point.line;
        self.selection = self.selection.take().filter(|s| !s.intersects_range(range));
    }

    /// Set the indexed color value.
    #[inline]
    fn set_color(&mut self, index: usize, color: Rgb) {
        let _recording = self.reflow.record(|| move |term| term.set_color(index, color));
        trace!("Setting color[{index}] = {color:?}");

        // Damage terminal if the color changed and it's not the cursor.
        if index != NamedColor::Cursor as usize && self.colors[index] != Some(color) {
            self.mark_fully_damaged();
        }

        self.colors[index] = Some(color);
    }

    /// Respond to a color query escape sequence.
    #[inline]
    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, terminator: &str) {
        trace!("Requested write of escape sequence for color code {prefix}: color[{index}]");

        let terminator = terminator.to_owned();
        self.event_proxy.send_event(Event::ColorRequest(
            index,
            Arc::new(move |color| {
                format!(
                    "\x1b]{};rgb:{1:02x}{1:02x}/{2:02x}{2:02x}/{3:02x}{3:02x}{4}",
                    prefix, color.r, color.g, color.b, terminator
                )
            }),
        ));
    }

    /// Reset the indexed color to original value.
    #[inline]
    fn reset_color(&mut self, index: usize) {
        let _recording = self.reflow.record(|| move |term| term.reset_color(index));
        trace!("Resetting color[{index}]");

        // Damage terminal if the color changed and it's not the cursor.
        if index != NamedColor::Cursor as usize && self.colors[index].is_some() {
            self.mark_fully_damaged();
        }

        self.colors[index] = None;
    }

    /// Store data into clipboard.
    #[inline]
    fn clipboard_store(&mut self, clipboard: u8, base64: &[u8]) {
        if !matches!(self.config.osc52, Osc52::OnlyCopy | Osc52::CopyPaste) {
            debug!("Denied osc52 store");
            return;
        }

        let clipboard_type = match clipboard {
            b'c' => ClipboardType::Clipboard,
            b'p' | b's' => ClipboardType::Selection,
            _ => return,
        };

        if let Ok(bytes) = Base64.decode(base64)
            && let Ok(text) = String::from_utf8(bytes)
        {
            self.event_proxy.send_event(Event::ClipboardStore(clipboard_type, text));
        }
    }

    /// Load data from clipboard.
    #[inline]
    fn clipboard_load(&mut self, clipboard: u8, terminator: &str) {
        if !matches!(self.config.osc52, Osc52::OnlyPaste | Osc52::CopyPaste) {
            debug!("Denied osc52 load");
            return;
        }

        let clipboard_type = match clipboard {
            b'c' => ClipboardType::Clipboard,
            b'p' | b's' => ClipboardType::Selection,
            _ => return,
        };

        let terminator = terminator.to_owned();

        self.event_proxy.send_event(Event::ClipboardLoad(
            clipboard_type,
            Arc::new(move |text| {
                let base64 = Base64.encode(text);
                format!("\x1b]52;{};{}{}", clipboard as char, base64, terminator)
            }),
        ));
    }

    #[inline]
    fn clear_screen(&mut self, mode: ansi::ClearMode) {
        let _recording = self.reflow.record(|| move |term| term.clear_screen(mode));
        trace!("Clearing screen: {mode:?}");
        if self.erase_guarded_screen(mode) {
            return;
        }
        let blank = self.grid.cursor.template.erased();

        let screen_lines = self.screen_lines();

        match mode {
            ansi::ClearMode::Above => {
                let cursor = self.grid.cursor.point;

                // If clearing more than one line.
                if cursor.line > 0 {
                    // Fully clear all lines before the current line.
                    self.grid.reset_region(..cursor.line);
                }

                // Clear up to the current column in the current line.
                let end = cmp::min(cursor.column + 1, Column(self.columns()));
                if end.0 == self.columns() {
                    self.reset_line_size(cursor.line);
                }
                self.clear_edit_boundary(cursor.line, 0);
                self.clear_edit_boundary(cursor.line, end.0);
                for cell in &mut self.grid[cursor.line][..end] {
                    *cell = blank.clone();
                }

                let range = Line(0)..=cursor.line;
                self.selection = self.selection.take().filter(|s| !s.intersects_range(range));
            },
            ansi::ClearMode::Below => {
                let cursor = self.grid.cursor.point;
                if cursor.column == 0 {
                    self.reset_line_size(cursor.line);
                }
                self.clear_edit_boundary(cursor.line, cursor.column.0);
                for cell in &mut self.grid[cursor.line][cursor.column..] {
                    *cell = blank.clone();
                }

                if (cursor.line.0 as usize) < screen_lines - 1 {
                    self.grid.reset_region((cursor.line + 1)..);
                }

                let range = cursor.line..Line(screen_lines as i32);
                self.selection = self.selection.take().filter(|s| !s.intersects_range(range));
            },
            ansi::ClearMode::All => {
                if self.mode.contains(TermMode::ALT_SCREEN) {
                    self.grid.reset_region(..);
                } else {
                    let old_offset = self.grid.display_offset();

                    self.grid.clear_viewport();
                    self.clear_screen_under_prompt();

                    // Compute number of lines scrolled by clearing the viewport.
                    let lines = self.grid.display_offset().saturating_sub(old_offset);

                    self.vi_mode_cursor.point.line =
                        (self.vi_mode_cursor.point.line - lines).grid_clamp(self, Boundary::Grid);
                }

                self.selection = None;
            },
            ansi::ClearMode::Saved if self.history_size() > 0 => {
                defer_history_drop(self.grid.take_history());

                self.vi_mode_cursor.point.line =
                    self.vi_mode_cursor.point.line.grid_clamp(self, Boundary::Cursor);

                self.selection = self.selection.take().filter(|s| !s.intersects_range(..Line(0)));
            },
            // We have no history to clear.
            ansi::ClearMode::Saved => (),
        }

        self.mark_fully_damaged();
    }

    #[inline]
    fn clear_tabs(&mut self, mode: ansi::TabulationClearMode) {
        let _recording = self.reflow.record(|| move |term| term.clear_tabs(mode));
        trace!("Clearing tabs: {mode:?}");
        match mode {
            ansi::TabulationClearMode::Current => {
                self.tabs[self.grid.cursor.point.column] = false;
            },
            ansi::TabulationClearMode::All => {
                self.tabs.clear_all();
            },
        }
    }

    /// Reset all important fields in the term struct.
    #[inline]
    fn reset_state(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.reset_state());
        self.prompt = None;
        if self.mode.contains(TermMode::ALT_SCREEN) {
            mem::swap(&mut self.grid, &mut self.inactive_grid);
        }
        self.active_charset = Default::default();
        self.cursor_style = None;
        self.grid.reset();
        self.inactive_grid.reset();
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);
        self.horizontal_margin = None;
        self.protocol = protocol::State::default();
        self.tabs = TabStops::new(self.columns());
        self.title_stack = Vec::new();
        self.title = None;
        self.selection = None;
        self.vi_mode_cursor = Default::default();
        self.keyboard_mode_stack = Default::default();
        self.inactive_keyboard_mode_stack = Default::default();

        // Preserve vi mode across resets.
        self.mode &= TermMode::VI;
        self.mode.insert(TermMode::default());
        self.restore_output_mode();

        self.event_proxy.send_event(Event::CursorBlinkingChange);
        self.mark_fully_damaged();
    }

    #[inline]
    fn reverse_index(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.reverse_index());
        trace!("Reversing index");
        self.grid.cursor.input_needs_wrap = false;
        // If cursor is at the top.
        if self.grid.cursor.point.line == self.scroll_region.start {
            if self.horizontal_bounds().contains(&self.grid.cursor.point.column) {
                self.scroll_down(1);
            }
        } else {
            self.damage_cursor();
            self.grid.cursor.point.line = cmp::max(self.grid.cursor.point.line - 1, Line(0));
            self.damage_cursor();
        }
    }

    #[inline]
    fn semantic_prompt_mark(&mut self, mark: SemanticPromptMark) {
        let _recording = self.reflow.record(|| move |term| term.semantic_prompt_mark(mark));
        self.mark_prompt(mark);
    }

    #[inline]
    fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        let _recording = self.reflow.record(|| {
            let hyperlink = hyperlink.clone();
            move |term| term.set_hyperlink(hyperlink)
        });
        trace!("Setting hyperlink: {hyperlink:?}");
        self.grid.cursor.template.set_hyperlink(hyperlink.map(|e| e.into()));
    }

    #[inline]
    fn desktop_notification(&mut self, metadata: &[u8], payload: &[u8], terminator: &str) {
        self.handle_desktop_notification(metadata, payload, terminator);
    }

    /// Set a terminal attribute.
    #[inline]
    fn terminal_attribute(&mut self, attr: Attr) {
        let _recording = self.reflow.record(|| move |term| term.terminal_attribute(attr));
        trace!("Setting attribute: {attr:?}");
        self.grid.cursor.template.apply_attribute(attr);
    }

    #[inline]
    fn set_private_mode(&mut self, mode: PrivateMode) {
        let _recording = self.reflow.record(|| move |term| term.set_private_mode(mode));
        self.set_private_mode_inner(mode);
    }

    #[inline]
    fn unset_private_mode(&mut self, mode: PrivateMode) {
        let _recording = self.reflow.record(|| move |term| term.unset_private_mode(mode));
        self.unset_private_mode_inner(mode);
    }

    #[inline]
    fn report_private_mode(&mut self, mode: PrivateMode) {
        self.report_private_mode_inner(mode);
    }

    #[inline]
    fn set_mode(&mut self, mode: ansi::Mode) {
        let _recording = self.reflow.record(|| move |term| term.set_mode(mode));
        self.set_mode_inner(mode);
    }

    #[inline]
    fn unset_mode(&mut self, mode: ansi::Mode) {
        let _recording = self.reflow.record(|| move |term| term.unset_mode(mode));
        self.unset_mode_inner(mode);
    }

    #[inline]
    fn report_mode(&mut self, mode: ansi::Mode) {
        self.report_mode_inner(mode);
    }

    #[inline]
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let _recording = self.reflow.record(|| move |term| term.set_scrolling_region(top, bottom));
        // Fallback to the last line as default.
        let bottom = bottom.unwrap_or_else(|| self.screen_lines()).min(self.screen_lines());

        if top >= bottom {
            debug!("Invalid scrolling region: ({top};{bottom})");
            return;
        }

        // Bottom should be included in the range, but range end is not
        // usually included. One option would be to use an inclusive
        // range, but instead we just let the open range end be 1
        // higher.
        let start = Line(top as i32 - 1);
        let end = Line(bottom as i32);

        trace!("Setting scrolling region: ({start};{end})");

        let screen_lines = Line(self.screen_lines() as i32);
        self.scroll_region.start = cmp::min(start, screen_lines);
        self.scroll_region.end = cmp::min(end, screen_lines);
        self.goto(0, 0);
    }

    #[inline]
    fn set_keypad_application_mode(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.set_keypad_application_mode());
        trace!("Setting keypad application mode");
        self.mode.insert(TermMode::APP_KEYPAD);
    }

    #[inline]
    fn unset_keypad_application_mode(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.unset_keypad_application_mode());
        trace!("Unsetting keypad application mode");
        self.mode.remove(TermMode::APP_KEYPAD);
    }

    #[inline]
    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        let _recording = self.reflow.record(|| move |term| term.configure_charset(index, charset));
        trace!("Configuring charset {index:?} as {charset:?}");
        // Without DECNRCM a national designation selects ASCII.
        let charset = match charset {
            StandardCharset::National(_) if !self.protocol.nrcs => StandardCharset::Ascii,
            charset => charset,
        };
        self.grid.cursor.charsets[index] = charset;
    }

    #[inline]
    fn set_active_charset(&mut self, index: CharsetIndex) {
        let _recording = self.reflow.record(|| move |term| term.set_active_charset(index));
        trace!("Setting active charset {index:?}");
        self.active_charset = index;
    }

    #[inline]
    fn set_cursor_style(&mut self, style: Option<CursorStyle>) {
        let _recording = self.reflow.record(|| move |term| term.set_cursor_style(style));
        trace!("Setting cursor style {style:?}");
        self.cursor_style = style;

        // Notify UI about blinking changes.
        self.event_proxy.send_event(Event::CursorBlinkingChange);
    }

    #[inline]
    fn set_cursor_shape(&mut self, shape: CursorShape) {
        let _recording = self.reflow.record(|| move |term| term.set_cursor_shape(shape));
        trace!("Setting cursor shape {shape:?}");

        let style = self.cursor_style.get_or_insert(self.config.default_cursor_style);
        style.shape = shape;
    }

    #[inline]
    fn set_title(&mut self, title: Option<String>) {
        let _recording = self.reflow.record(|| {
            let title = title.clone();
            move |term| term.set_title(title)
        });
        trace!("Setting title to '{title:?}'");

        self.title.clone_from(&title);

        let title_event = match title {
            Some(title) => Event::Title(title),
            None => Event::ResetTitle,
        };

        self.event_proxy.send_event(title_event);
    }

    #[inline]
    fn push_title(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.push_title());
        trace!("Pushing '{:?}' onto title stack", self.title);

        if self.title_stack.len() >= TITLE_STACK_MAX_DEPTH {
            let removed = self.title_stack.remove(0);
            trace!(
                "Removing '{removed:?}' from bottom of title stack that exceeds its maximum depth"
            );
        }

        self.title_stack.push(self.title.clone());
    }

    #[inline]
    fn pop_title(&mut self) {
        let _recording = self.reflow.record(|| move |term| term.pop_title());
        trace!("Attempting to pop title from stack...");

        if let Some(popped) = self.title_stack.pop() {
            trace!("Title '{popped:?}' popped from stack");
            self.set_title(popped);
        }
    }

    #[inline]
    fn text_area_size_pixels(&mut self) {
        self.event_proxy.send_event(Event::TextAreaSizeRequest(Arc::new(move |window_size| {
            let height = u32::from(window_size.num_lines) * u32::from(window_size.cell_height);
            let width = u32::from(window_size.num_cols) * u32::from(window_size.cell_width);
            format!("\x1b[4;{height};{width}t")
        })));
    }

    #[inline]
    fn character_cell_size_pixels(&mut self) {
        self.event_proxy.send_event(Event::TextAreaSizeRequest(Arc::new(move |window_size| {
            format!("\x1b[6;{};{}t", window_size.cell_height, window_size.cell_width)
        })));
    }

    #[inline]
    fn text_area_size_chars(&mut self) {
        let text = format!("\x1b[8;{};{}t", self.screen_lines(), self.columns());
        self.write_reply(text);
    }
}
