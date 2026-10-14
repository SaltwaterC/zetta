//! Screen state read from Zetta's terminal engine, for embedders whose grid
//! is an `alacritty_terminal::Term`: Zetta's own panes and the multiplexer's
//! console host both answer the console's screen queries from one.
use crate::output::{Cell, Half, Pen, PenColor, Screen};
use crate::state::Palette;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell as GridCell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::tty::ConsolePalette;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// The grid's colors as the console reports them: theme colors are the
/// console's defaults, and dim colors are their base ANSI color.
pub fn pen_color(color: Color) -> PenColor {
    match color {
        Color::Named(named) if (named as usize) < 16 => PenColor::Indexed(named as u8),
        Color::Named(named) if (NamedColor::DimBlack..=NamedColor::DimWhite).contains(&named) => {
            PenColor::Indexed((named as usize - NamedColor::DimBlack as usize) as u8)
        }
        Color::Named(_) => PenColor::Default,
        Color::Indexed(index) => PenColor::Indexed(index),
        Color::Spec(rgb) => PenColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

fn pen(cell: &GridCell) -> Pen {
    Pen {
        foreground: pen_color(cell.fg),
        background: pen_color(cell.bg),
        reverse: cell.flags.contains(Flags::INVERSE),
        underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
    }
}

fn clamp(value: usize) -> i16 {
    value.min(i16::MAX as usize) as i16
}

/// Size, cursor, cursor visibility, the pen the next character gets, and
/// whether a wrap is pending.
pub fn screen<T>(term: &Term<T>) -> Screen {
    let cursor = &term.grid().cursor;
    Screen {
        columns: clamp(term.columns()),
        rows: clamp(term.screen_lines()),
        cursor: (
            clamp(cursor.point.column.0),
            cursor.point.line.0.clamp(0, i32::from(i16::MAX)) as i16,
        ),
        cursor_visible: term.mode().contains(TermMode::SHOW_CURSOR),
        pen: Some(pen(&cursor.template)),
        wrap_pending: cursor.input_needs_wrap,
    }
}

/// The visible cells of `rows` between `columns`, row by row; cells outside
/// the screen read as blanks.
pub fn read_cells<T>(
    term: &Term<T>,
    rows: std::ops::Range<i16>,
    columns: std::ops::Range<i16>,
) -> Vec<Cell> {
    let grid = term.grid();
    let blank = Cell {
        ch: ' ',
        pen: Pen {
            foreground: PenColor::Default,
            background: PenColor::Default,
            reverse: false,
            underline: false,
        },
        half: Half::Whole,
    };
    let mut cells = Vec::new();
    for row in rows {
        for column in columns.clone() {
            if row < 0
                || column < 0
                || row as usize >= term.screen_lines()
                || column as usize >= term.columns()
            {
                cells.push(blank);
                continue;
            }
            let line = &grid[Line(i32::from(row))];
            let cell = &line[Column(column as usize)];
            let (ch, half) = if cell.flags.contains(Flags::WIDE_CHAR) {
                (cell.c, Half::Leading)
            } else if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && column > 0 {
                (line[Column(column as usize - 1)].c, Half::Trailing)
            } else if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                (' ', Half::Whole)
            } else {
                (cell.c, Half::Whole)
            };
            cells.push(Cell {
                ch,
                pen: pen(cell),
                half,
            });
        }
    }
    cells
}

/// The pane palette in console order: COLORREF entries indexed by legacy
/// attribute color, as ConPTY is given it through SetConsoleScreenBufferInfoEx.
pub fn console_palette(palette: ConsolePalette) -> Palette {
    let mut colors = [0; 16];
    for (ansi, [r, g, b]) in palette.colors.into_iter().enumerate() {
        let legacy = ConsolePalette::win32_index(ansi as u8);
        colors[usize::from(legacy)] = u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
    }
    Palette {
        colors,
        attributes: palette.win32_attributes(),
    }
}

#[cfg(test)]
#[path = "tests/grid.rs"]
mod tests;
