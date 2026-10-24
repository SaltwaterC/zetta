//! The cells of a screen buffer that is not on screen: one a client created,
//! or the main buffer while another one is shown.
//!
//! A client can size a buffer at 32766x32766 cells, about 4.3 GB of them.
//! Conhost commits a row the first time it is touched, so even it pays for
//! every row a client writes, fills or scrolls through. Here a row whose
//! cells are all the same (never written, filled with one cell, or scrolled
//! in blank) is kept as that one cell. A buffer then costs a row of cells
//! only for each row that holds more than one, and scrolls by rotating its
//! rows instead of moving every cell up. The row table and those rows are
//! charged to the session's budget.
use crate::budget::{self, Budget, Charge};
use crate::cells::{self, CharInfo, Rect};
use crate::protocol::try_vec;
use crate::state::mode;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;

/// Per-buffer settings that belong to whichever buffer is active.
#[derive(Clone, Copy, Debug)]
pub struct Saved {
    pub output_mode: u32,
    pub cursor_size: u32,
    pub attributes: u16,
    pub cursor_visible: bool,
    pub cursor: (i16, i16),
}

#[derive(Clone)]
enum Row {
    /// Every cell of the row.
    Same(CharInfo),
    Cells(Box<[CharInfo]>),
}
impl Row {
    fn get(&self, x: usize) -> CharInfo {
        match self {
            Row::Same(cell) => *cell,
            Row::Cells(cells) => cells[x],
        }
    }
    /// What the row is charged for besides its slot in the table.
    fn bytes(&self) -> usize {
        match self {
            Row::Same(_) => 0,
            Row::Cells(cells) => size_of_val(&**cells),
        }
    }
}

pub struct ScreenBuffer {
    pub size: (i16, i16),
    /// The window's size; its origin is the buffer's.
    pub window: (i16, i16),
    /// One for each row of `size`.
    rows: VecDeque<Row>,
    /// The row table and the rows that hold cells of their own.
    charge: Charge,
    pub saved: Saved,
    /// Open handles; the buffer goes with the last one.
    pub handles: usize,
    /// The main buffer was changed off screen by more than text, so it is
    /// redrawn from its cells when it comes back.
    pub dirty: bool,
}
impl ScreenBuffer {
    /// A buffer of `size` every cell of which is `blank`.
    pub fn new(
        budget: &Arc<Budget>,
        size: (i16, i16),
        window: (i16, i16),
        blank: CharInfo,
        saved: Saved,
    ) -> io::Result<Self> {
        let (charge, rows) = blank_rows(budget, size.1.max(0) as usize, blank)?;
        Ok(Self {
            size,
            window,
            rows,
            charge,
            saved,
            handles: 0,
            dirty: false,
        })
    }
    pub fn width(&self) -> usize {
        self.size.0.max(0) as usize
    }
    pub fn height(&self) -> usize {
        self.size.1.max(0) as usize
    }
    pub fn cell(&self, x: i32, y: i32) -> Option<CharInfo> {
        let x = usize::try_from(x).ok().filter(|&x| x < self.width())?;
        Some(self.rows.get(usize::try_from(y).ok()?)?.get(x))
    }
    /// Row `y`'s cells, given cells of its own first if it had one.
    fn split(&mut self, y: usize) -> io::Result<&mut [CharInfo]> {
        if let Row::Same(cell) = self.rows[y] {
            let bytes = self.width() * size_of::<CharInfo>();
            self.charge.grow(bytes)?;
            let cells = try_vec(cell, self.width()).inspect_err(|_| self.charge.shrink(bytes))?;
            self.rows[y] = Row::Cells(cells.into_boxed_slice());
        }
        let Row::Cells(cells) = &mut self.rows[y] else {
            unreachable!("the row was just given cells of its own");
        };
        Ok(cells)
    }
    fn make_same(&mut self, y: usize, cell: CharInfo) {
        let old = std::mem::replace(&mut self.rows[y], Row::Same(cell));
        self.charge.shrink(old.bytes());
    }
    /// Writes `cells` into row `y` from column `left`, as far as the row
    /// goes. Cells that make the row, or leave it, all the same keep it as
    /// one cell.
    pub fn write_row(&mut self, left: i32, y: i32, cells: &[CharInfo]) -> io::Result<()> {
        let (Ok(left), Ok(y)) = (usize::try_from(left), usize::try_from(y)) else {
            return Ok(());
        };
        if y >= self.height() {
            return Ok(());
        }
        let cells = &cells[..cells.len().min(self.width().saturating_sub(left))];
        let Some(&first) = cells.first() else {
            return Ok(());
        };
        let same = cells.iter().all(|cell| *cell == first);
        if same && cells.len() == self.width() {
            self.make_same(y, first);
            return Ok(());
        }
        if same && matches!(self.rows[y], Row::Same(cell) if cell == first) {
            return Ok(());
        }
        self.split(y)?[left..left + cells.len()].copy_from_slice(cells);
        Ok(())
    }
    pub fn cell_mut(&mut self, x: i32, y: i32) -> io::Result<Option<&mut CharInfo>> {
        if self.cell(x, y).is_none() {
            return Ok(None);
        }
        Ok(Some(&mut self.split(y as usize)?[x as usize]))
    }

    /// ResizeTraditional: what fits stays where it was; the rest is blank.
    pub fn resize(&mut self, size: (i16, i16)) -> io::Result<()> {
        let blank = CharInfo::blank(self.saved.attributes);
        let (width, height) = (size.0.max(0) as usize, size.1.max(0) as usize);
        let kept = self.width().min(width);
        let (mut charge, mut rows) = blank_rows(self.charge.budget(), height, blank)?;
        for (y, old) in self.rows.iter().take(height).enumerate() {
            rows[y] = match old {
                Row::Same(cell) if *cell == blank || kept == width => Row::Same(*cell),
                old => {
                    charge.grow(width * size_of::<CharInfo>())?;
                    let mut cells = try_vec(blank, width)?;
                    for (x, cell) in cells[..kept].iter_mut().enumerate() {
                        *cell = old.get(x);
                    }
                    Row::Cells(cells.into_boxed_slice())
                }
            };
        }
        self.rows = rows;
        self.charge = charge;
        self.size = size;
        self.window = (self.window.0.min(size.0), self.window.1.min(size.1));
        self.saved.cursor = (
            self.saved.cursor.0.min(size.0 - 1).max(0),
            self.saved.cursor.1.min(size.1 - 1).max(0),
        );
        self.dirty = true;
        Ok(())
    }

    /// What of the buffer fits on a terminal of `terminal` cells, as VT.
    /// WriteScreenInfo draws no more than the screen either.
    pub fn draw(&self, out: &mut String, terminal: (i16, i16), row_width: i16, defaults: u16) {
        let columns = self.width().min(terminal.0.max(0) as usize);
        if columns == 0 {
            return;
        }
        let mut same = Vec::new();
        for (y, row) in self
            .rows
            .iter()
            .take(terminal.1.max(0) as usize)
            .enumerate()
        {
            let cells = match row {
                Row::Cells(cells) => &cells[..columns],
                Row::Same(cell) => {
                    same.clear();
                    same.resize(columns, *cell);
                    &same[..]
                }
            };
            cells::write_infos(out, (0, y as i16), cells, defaults, row_width);
        }
    }

    /// ScrollRegion on the buffer's own rows, without a copy of the region:
    /// what moves goes first, a row at a time in the order that reads each
    /// row before anything is written over it, then the fill wherever
    /// nothing moved to.
    pub fn scroll(
        &mut self,
        fill_rect: Rect,
        source: Rect,
        target: Rect,
        fill: CharInfo,
    ) -> io::Result<()> {
        if target.is_valid() {
            let down = target.top > source.top;
            let mut moved = Vec::new();
            for row in 0..source.height() {
                let row = if down { source.height() - 1 - row } else { row };
                moved.clear();
                moved.extend(
                    (source.left..=source.right).filter_map(|x| self.cell(x, source.top + row)),
                );
                self.write_row(target.left, target.top + row, &moved)?;
            }
        }
        if fill_rect.is_valid() {
            let cells = vec![fill; fill_rect.width() as usize];
            for y in fill_rect.top..=fill_rect.bottom {
                let spans = if target.is_valid() && (target.top..=target.bottom).contains(&y) {
                    [
                        (fill_rect.left, fill_rect.right.min(target.left - 1)),
                        (fill_rect.left.max(target.right + 1), fill_rect.right),
                    ]
                } else {
                    [(fill_rect.left, fill_rect.right), (0, -1)]
                };
                for (left, right) in spans {
                    if left <= right {
                        self.write_row(left, y, &cells[..(right - left + 1) as usize])?;
                    }
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// Moves the cursor to the next row, scrolling the buffer up one at the
    /// bottom: the top row goes and a blank one comes in below.
    fn newline(&mut self, y: &mut i32, attributes: u16) {
        *y += 1;
        if *y as usize >= self.height() {
            if let Some(row) = self.rows.pop_front() {
                self.charge.shrink(row.bytes());
            }
            self.rows.push_back(Row::Same(CharInfo::blank(attributes)));
            *y = self.height() as i32 - 1;
        }
    }
    /// The rows a text write to an inactive buffer would produce: printable
    /// text at the cursor with wrapping and CR/LF/BS/TAB. VT sequences are
    /// not interpreted off screen; they are skipped.
    pub fn write_text(&mut self, text: &str) -> io::Result<()> {
        let (width, height) = (i32::from(self.size.0), i32::from(self.size.1));
        if width <= 0 || height <= 0 {
            return Ok(());
        }
        let auto_return = self.saved.output_mode & mode::DISABLE_NEWLINE_AUTO_RETURN == 0;
        let wrap = self.saved.output_mode & mode::WRAP_AT_EOL_OUTPUT != 0;
        // A VT buffer defers the wrap to the next character, as a terminal
        // does; the legacy writer moves to the next line at once.
        let delayed = self.saved.output_mode & mode::VIRTUAL_TERMINAL_PROCESSING != 0;
        let attributes = self.saved.attributes;
        let (mut x, mut y) = (
            i32::from(self.saved.cursor.0),
            i32::from(self.saved.cursor.1),
        );
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\x1b' => {
                    // Skip one CSI or two-character escape.
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        for next in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&next) {
                                break;
                            }
                        }
                    } else {
                        chars.next();
                    }
                }
                '\r' => x = 0,
                '\n' => {
                    if auto_return {
                        x = 0;
                    }
                    self.newline(&mut y, attributes);
                }
                '\x08' => x = (x - 1).max(0),
                '\t' => x = (x + 8 - x % 8).min(width - 1),
                c if (c as u32) < 0x20 => {}
                c => {
                    if x >= width {
                        x = 0;
                        self.newline(&mut y, attributes);
                    }
                    let ch = u16::try_from(u32::from(c)).unwrap_or(0xfffd);
                    if let Some(cell) = self.cell_mut(x, y)? {
                        *cell = CharInfo { ch, attributes };
                    }
                    x += 1;
                    if x >= width && !wrap {
                        x = width - 1;
                    } else if x >= width && !delayed {
                        x = 0;
                        self.newline(&mut y, attributes);
                    }
                }
            }
        }
        self.saved.cursor = (x.min(width - 1) as i16, y as i16);
        Ok(())
    }
}

/// A table of `height` rows of `blank`, and its charge.
fn blank_rows(
    budget: &Arc<Budget>,
    height: usize,
    blank: CharInfo,
) -> io::Result<(Charge, VecDeque<Row>)> {
    let charge = budget.charge(height * size_of::<Row>())?;
    let mut rows = VecDeque::new();
    budget::reserve_exact(&mut rows, height)?;
    rows.extend(std::iter::repeat_n(Row::Same(blank), height));
    Ok((charge, rows))
}

#[cfg(test)]
#[path = "tests/screen_buffer.rs"]
mod tests;
