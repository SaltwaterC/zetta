//! Applies layout geometry to the live grid and backend, and chooses bounded
//! synchronous work or asynchronous retained-history reflow. Grid preparation
//! must succeed before notifying the shell of its new dimensions.

use super::*;

// Above this cell budget, reflow the complete history on a private worker.
// The bounded mutable tier supplies the live preview while the worker catches
// up with ordered output; painting never waits on history-sized traversal.
// Release measurements put a million-cell reflow beyond a frame's budget.
// Keep inline work close to the size of the engine's bounded live preview.
const MAX_SYNCHRONOUS_REFLOW_CELLS: usize = 128 * 1024;

fn synchronous_reflow_is_bounded(history_lines: usize, columns: usize) -> bool {
    history_lines.saturating_mul(columns) <= MAX_SYNCHRONOUS_REFLOW_CELLS
}

fn resize_grid(
    term: &mut AlacrittyTerm,
    bounds: TerminalBounds,
    reflow: bool,
) -> std::io::Result<()> {
    if term.reflow_reader().is_some()
        || (reflow && !synchronous_reflow_is_bounded(term.retained_history_size(), term.columns()))
    {
        term.resize_with_async_history(bounds)
    } else {
        resize(term, bounds, reflow);
        Ok(())
    }
}

impl Terminal {
    pub(super) fn apply_terminal_resize(
        &mut self,
        term: &mut AlacrittyTerm,
        new_bounds: TerminalBounds,
        reflow: bool,
        local_grid_changed: bool,
        effective_resize: bool,
        cx: &mut Context<Self>,
    ) {
        let new_bounds = normalize_terminal_bounds(new_bounds);
        trace!("Resizing: new_bounds={new_bounds:?}");

        // Compare against the grid rather than `last_content`, which
        // `set_size` already advanced when it queued this event.
        let grid_size_changed = term.screen_lines() != new_bounds.num_lines()
            || term.columns() != new_bounds.num_columns();
        let columns_changed = term.columns() != new_bounds.num_columns();
        let was_size_initialized = self.terminal_size_initialized;
        self.last_content.terminal_bounds = new_bounds;
        self.terminal_size_initialized = true;
        self.size_initialization_queued = false;

        if effective_resize {
            if let Err(error) = resize_grid(term, new_bounds, reflow) {
                // Keep the backend at the actual grid size. Rolling back the
                // queued bounds lets the next layout retry instead of leaving
                // a shell sized differently from the grid answering its queries.
                let mut actual = new_bounds;
                actual.bounds.size.width = actual.cell_width * term.columns() as f32;
                actual.bounds.size.height = actual.line_height * term.screen_lines() as f32;
                self.last_content.terminal_bounds = actual;
                self.terminal_size_initialized = was_size_initialized;
                log::error!("could not start terminal history reflow: {error}");
                cx.emit(Event::Wakeup);
                return;
            }
            // A byte-stream terminal has no local PTY event loop, so
            // its supplied controller is the only resize route.
            if matches!(self.terminal_type, TerminalType::DisplayOnly)
                && let Some(control) = &self.pty_control
            {
                control.resize(
                    new_bounds.num_columns() as u16,
                    new_bounds.num_lines() as u16,
                );
            }
            #[cfg(windows)]
            if let TerminalType::Pty { pty_tx, .. } = &self.terminal_type {
                if let Some(control) = &self.pty_control {
                    control.resize(
                        new_bounds.num_columns() as u16,
                        new_bounds.num_lines() as u16,
                    );
                } else if let Some(pty_tx) = pty_tx {
                    pty_tx.resize(new_bounds, self.display_scale_factor);
                }
            }
            #[cfg(not(windows))]
            if let TerminalType::Pty {
                pty_tx: Some(pty_tx),
                ..
            } = &self.terminal_type
            {
                pty_tx.resize(new_bounds, self.display_scale_factor);
            }
        }
        self.redraw_attached_after_first_layout();
        if grid_size_changed || !was_size_initialized {
            cx.emit(Event::GridSizeChanged);
        }
        if local_grid_changed {
            cx.emit(Event::LocalGridSizeChanged);
        }
        // A reflow rewraps the scrollback, which moves every line the
        // recorded directories are keyed by.
        if columns_changed {
            self.reset_cwd_history();
        }
        // If there are matches we need to emit a wake up event to
        // invalidate the matches and recalculate their locations
        // in the new terminal layout
        if !self.matches.is_empty() {
            cx.emit(Event::Wakeup);
        }
    }
}

#[cfg(test)]
#[path = "tests/resize.rs"]
mod tests;
