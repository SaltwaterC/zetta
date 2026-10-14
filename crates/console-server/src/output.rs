//! Application-independent output/state hooks for an embedded console host.
//! Driver flags are translated here; consumers receive semantic modes.
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputMode {
    pub lf_implies_cr: bool,
    pub wrap: bool,
}
impl OutputMode {
    pub fn from_console_flags(flags: u32) -> Self {
        Self {
            lf_implies_cr: flags & 1 != 0 && flags & 8 == 0,
            wrap: flags & 2 != 0,
        }
    }
}
/// A color as the embedder's grid holds it. The server maps it to the legacy
/// attribute nibble, using the console's default attributes and palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenColor {
    Default,
    /// An xterm 256-color index; 0-15 are the ANSI colors.
    Indexed(u8),
    Rgb(u8, u8, u8),
}
/// The attributes the next printed character gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pen {
    pub foreground: PenColor,
    pub background: PenColor,
    pub reverse: bool,
    pub underline: bool,
}
/// Which half of a two-column character a cell holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Half {
    #[default]
    Whole,
    Leading,
    Trailing,
}
/// One grid cell as ReadConsoleOutput reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub pen: Pen,
    pub half: Half,
}
/// What screen-buffer queries report, read in one go after a flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub columns: i16,
    pub rows: i16,
    /// Column and row of the cursor within the visible screen.
    pub cursor: (i16, i16),
    pub cursor_visible: bool,
    /// None when the embedder cannot say; the server then reports the last
    /// attributes set through the console API.
    pub pen: Option<Pen>,
    /// The cursor is past the last column with the wrap deferred to the next
    /// character, as a terminal (and VT output) does it. False when the
    /// embedder cannot say.
    pub wrap_pending: bool,
}
pub trait ConsoleOutput: Send {
    fn write(&mut self, bytes: &[u8], mode: OutputMode) -> io::Result<()>;
    /// All previously published output must be parsed before this returns.
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
    fn cursor(&self) -> (i16, i16) {
        (0, 0)
    }
    fn size(&self) -> (i16, i16) {
        (80, 24)
    }
    /// Screen state for GetConsoleScreenBufferInfo and GetConsoleCursorInfo.
    /// Override to read it under one lock.
    fn screen(&self) -> Screen {
        let (columns, rows) = self.size();
        Screen {
            columns,
            rows,
            cursor: self.cursor(),
            cursor_visible: true,
            pen: None,
            wrap_pending: false,
        }
    }
    fn title(&self) -> Option<String> {
        None
    }
    /// The visible cells of `rows` (screen-relative) between `columns`, row
    /// by row, after a flush. None when the embedder keeps no grid, which
    /// makes the cell-level console APIs fail.
    fn read_cells(
        &self,
        _rows: std::ops::Range<i16>,
        _columns: std::ops::Range<i16>,
    ) -> Option<Vec<Cell>> {
        None
    }
    fn set_title(&mut self, _title: &str) -> io::Result<()> {
        Ok(())
    }
    /// Blocking consumers must return a handle that wakes write/flush on close.
    /// It runs without taking the output lock, including during teardown.
    fn cancellation(&self) -> Option<std::sync::Arc<dyn Fn() + Send + Sync>> {
        None
    }
    fn ended(&mut self) {}
    /// Readiness callbacks must not wait for a blocked output write/flush.
    fn input_readiness(&self) -> Option<std::sync::Arc<dyn Fn(bool) + Send + Sync>> {
        None
    }
    fn bytes(&self) -> u64 {
        0
    }
}
