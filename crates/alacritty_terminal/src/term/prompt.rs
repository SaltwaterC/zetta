//! Keeps a shell's prompt out of reflow while the shell is reading a command.
//!
//! zsh and fish redraw their whole prompt when the window size changes. They
//! do it by moving the cursor up the number of rows *they* drew the prompt
//! and its input on, then clearing to the end of the screen and drawing it
//! again. Reflow breaks that count: narrowing wraps a prompt line that filled
//! the old width (a right-aligned `RPROMPT` always does) onto one more row,
//! so the shell's move up stops short and leaves the old first line behind,
//! and widening joins wrapped input back onto fewer rows, so the move up
//! overshoots into the output above. Each resize left another stale copy.
//!
//! So between an OSC 133 prompt mark and the command the shell then runs,
//! [`Term::resize`] truncates the prompt's rows to the new width and drops
//! their wrap flags first. Nothing in them is lost that the shell does not
//! draw again, they keep their row count, and everything above them still
//! reflows. Only a shell that redraws its prompt may send the mark, which is
//! why Zetta's zsh and fish integrations do and its bash and PowerShell ones
//! do not.
//!
//! The prompt is kept as a screen line, moved with every scroll of the whole
//! screen. A scroll inside a smaller region, or the alternate screen, means
//! something other than a shell's line editor is drawing, so the mark is
//! dropped rather than guessed at.

use super::*;
use crate::grid::Row;
use crate::vte::ansi::SemanticPromptMark;

impl<T> Term<T> {
    pub(super) fn mark_prompt(&mut self, mark: SemanticPromptMark) {
        self.prompt = match mark {
            SemanticPromptMark::CommandStart => None,
            SemanticPromptMark::PromptStart if self.mode.contains(TermMode::ALT_SCREEN) => None,
            SemanticPromptMark::PromptStart => {
                // A mark printed after output that did not end its line comes
                // before zsh's `PROMPT_SP` moves the prompt to a fresh one.
                let cursor = &self.grid.cursor;
                let fresh_line = cursor.point.column == Column(0) && !cursor.input_needs_wrap;
                Some(if fresh_line { cursor.point.line } else { cursor.point.line + 1 })
            },
        };
    }

    /// Moves the prompt with a scroll of `region` by `lines`, upwards when
    /// negative.
    pub(super) fn scroll_prompt(&mut self, region: &Range<Line>, lines: i32) {
        let Some(prompt) = self.prompt else { return };
        let screen = Line(0)..Line(self.screen_lines() as i32);
        self.prompt = if *region == screen {
            Some(prompt + lines).filter(|prompt| *prompt < screen.end)
        } else if region.contains(&prompt) {
            None
        } else {
            Some(prompt)
        };
    }

    /// A clear of the whole screen while at the prompt is the shell clearing
    /// it (`clear-screen`), which draws the prompt again from the cursor.
    pub(super) fn clear_screen_under_prompt(&mut self) {
        if self.prompt.is_some() {
            self.prompt = Some(self.grid.cursor.point.line);
        }
    }

    /// Prepares the prompt's rows for a resize to `columns`; see the module
    /// docs. Returns how far above the cursor the prompt starts, for
    /// [`Self::reattach_prompt`].
    pub(super) fn detach_prompt_from_reflow(&mut self, columns: usize) -> Option<i32> {
        let prompt = self.prompt?;
        let cursor = self.grid.cursor.point;
        if self.mode.contains(TermMode::ALT_SCREEN) || cursor.line < prompt {
            self.prompt = None;
            return None;
        }
        let old_columns = self.columns();
        if columns != old_columns {
            let top = cmp::max(prompt, Line(0));
            // A prompt the shell started at the end of a wrapped line would
            // otherwise be joined onto it.
            if top > self.grid.topmost_line() {
                let above: &mut Row<Cell> = &mut self.grid[top - 1];
                above[Column(old_columns - 1)].flags.remove(Flags::WRAPLINE);
            }
            for line in top.0..self.screen_lines() as i32 {
                let row: &mut Row<Cell> = &mut self.grid[Line(line)];
                row[Column(old_columns - 1)].flags.remove(Flags::WRAPLINE);
                if columns < old_columns {
                    // A wide character cut in half goes with its spacer.
                    let start = if row[Column(columns - 1)].flags.contains(Flags::WIDE_CHAR) {
                        columns - 1
                    } else {
                        columns
                    };
                    row[Column(start)..Column(old_columns)].fill(Cell::default());
                }
            }
            // Reflow wraps a cursor past the new width onto a row of its own.
            if columns < old_columns && cursor.column.0 >= columns {
                self.grid.cursor.point.column = Column(columns - 1);
                self.grid.cursor.input_needs_wrap = false;
            }
        }
        Some(cursor.line.0 - prompt.0)
    }

    /// Puts the prompt back above the cursor once the grid has been resized.
    /// Its rows moved with the cursor, because none of them reflowed.
    pub(super) fn reattach_prompt(&mut self, rows_above_cursor: Option<i32>) {
        if let Some(rows) = rows_above_cursor {
            self.prompt = Some(self.grid.cursor.point.line - rows);
        }
    }

    /// The line the shell's current prompt starts on, while it is reading a
    /// command.
    pub fn prompt_line(&self) -> Option<Line> {
        self.prompt
    }
}

#[cfg(test)]
#[path = "../tests/term/prompt.rs"]
mod tests;
