//! Reconstruct path tokens wrapped by applications rather than by the terminal.
//!
//! TUIs can draw indented continuation rows with cursor movement, leaving no
//! WRAPLINE flag. Infer only adjacent, equally styled tokens at the right edge;
//! short lines and new drive/home prefixes remain separate. This is a bounded
//! text-only heuristic: hovering must never perform filesystem I/O on the render path.

use std::{ops::Range, time::Duration};

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Column, Direction, Line, Point},
    term::{cell::Flags, search::Match},
};
use regex::Regex;

use super::hyperlinks::match_path_text;

const MAX_ROWS: usize = 16;
const MAX_INDENT: usize = 8;
const RIGHT_MARGIN: usize = 2;

#[derive(Clone, Copy)]
struct Token {
    line: Line,
    columns: RangeColumns,
}

#[derive(Clone, Copy)]
struct RangeColumns {
    start: usize,
    end: usize,
}

impl Token {
    fn start(self) -> Point {
        Point::new(self.line, Column(self.columns.start))
    }

    fn end(self) -> Point {
        Point::new(self.line, Column(self.columns.end - 1))
    }
}

fn token_at<T>(term: &Term<T>, point: Point) -> Option<Token> {
    let row = &term.grid()[point.line];
    let is_space = |column: usize| {
        let cell = &row[Column(column)];
        cell.c.is_whitespace() && !cell.flags.contains(Flags::WIDE_CHAR_SPACER)
    };
    if is_space(point.column.0) {
        return None;
    }
    let mut start = point.column.0;
    let mut end = start + 1;
    while start > 0 && !is_space(start - 1) {
        start -= 1;
    }
    while end < term.columns() && !is_space(end) {
        end += 1;
    }
    Some(Token {
        line: point.line,
        columns: RangeColumns { start, end },
    })
}

fn first_token<T>(term: &Term<T>, line: Line) -> Option<Token> {
    (0..term.columns()).find_map(|column| token_at(term, Point::new(line, Column(column))))
}

fn last_token<T>(term: &Term<T>, line: Line) -> Option<Token> {
    (0..term.columns())
        .rev()
        .find_map(|column| token_at(term, Point::new(line, Column(column))))
}

fn continues<T>(term: &Term<T>, left: Token, right: Token) -> bool {
    if term.columns() - left.columns.end > RIGHT_MARGIN
        || right.columns.start == 0
        || right.columns.start > MAX_INDENT
        || term.grid().row_wraps(left.line)
    {
        return false;
    }
    let left_cell = &term.grid()[left.end()];
    let right_cell = &term.grid()[right.start()];
    // A drive prefix or home prefix starts a separate target. A slash can also
    // be the next character of a split path, so it cannot alone signal a root.
    if right_cell.c == '~'
        || (right_cell.c.is_ascii_alphabetic()
            && right.columns.end > right.columns.start + 1
            && term.grid()[Point::new(right.line, Column(right.columns.start + 1))].c == ':')
        || matches!(
            left_cell.c,
            ')' | ']' | '}' | '>' | '\'' | '"' | '`' | ',' | ';'
        )
    {
        return false;
    }
    let layout_flags = Flags::WRAPLINE
        | Flags::WIDE_CHAR
        | Flags::WIDE_CHAR_SPACER
        | Flags::LEADING_WIDE_CHAR_SPACER;
    left_cell.fg == right_cell.fg
        && left_cell.bg == right_cell.bg
        && (left_cell.flags - layout_flags) == (right_cell.flags - layout_flags)
}

pub(super) fn find<T>(
    term: &Term<T>,
    hovered: Point,
    regexes: &[Regex],
    timeout: Duration,
) -> Option<(String, Match)> {
    // Native soft wrapping is unambiguous and stays with the normal matcher.
    if term.grid().row_wraps(hovered.line)
        || (hovered.line > term.topmost_line() && term.grid().row_wraps(hovered.line - 1))
    {
        return None;
    }
    let token = token_at(term, hovered)?;
    let mut start = token;
    let mut end = token;
    while start.line > term.topmost_line() {
        if first_token(term, start.line)?.start() != start.start() {
            break;
        }
        let Some(previous) = last_token(term, start.line - 1) else {
            break;
        };
        if !continues(term, previous, start) {
            break;
        }
        start = previous;
        if (end.line.0 - start.line.0) as usize >= MAX_ROWS {
            return None;
        }
    }
    while end.line < term.bottommost_line() {
        if last_token(term, end.line)?.end() != end.end() {
            break;
        }
        let Some(next) = first_token(term, end.line + 1) else {
            break;
        };
        if !continues(term, end, next) {
            break;
        }
        end = next;
        if (end.line.0 - start.line.0) as usize >= MAX_ROWS {
            return None;
        }
    }
    if start.line == end.line {
        return None;
    }

    let mut text = String::new();
    let mut points = Vec::new();
    let mut hovered_offset = None;
    for row in start.line.0..=end.line.0 {
        let columns = if row == start.line.0 {
            start.columns.start..start.columns.end
        } else {
            let token = first_token(term, Line(row))?;
            token.columns.start..token.columns.end
        };
        append_token(term, Line(row), columns, &mut text, &mut points);
    }
    for &(offset, point) in &points {
        if point <= hovered && hovered <= term.expand_wide(point, Direction::Right) {
            hovered_offset = Some(offset);
            break;
        }
    }
    // A separator distinguishes path tokens from prose split inside a long word.
    if !text.contains(['/', '\\']) {
        return None;
    }
    let (path, range) = match_path_text(&text, hovered_offset?, regexes, timeout)?;
    if !path.contains(['/', '\\']) {
        return None;
    }
    let first = points.iter().find(|(offset, _)| *offset == range.start)?.1;
    let last = points
        .iter()
        .rev()
        .find(|(offset, _)| *offset < range.end)?
        .1;
    if first.line == last.line {
        return None;
    }
    Some((path, first..=term.expand_wide(last, Direction::Right)))
}

fn append_token<T>(
    term: &Term<T>,
    line: Line,
    columns: Range<usize>,
    text: &mut String,
    points: &mut Vec<(usize, Point)>,
) {
    for column in columns {
        let point = Point::new(line, Column(column));
        let cell = &term.grid()[point];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        points.push((text.len(), point));
        text.push(cell.c);
    }
}

#[cfg(test)]
#[path = "../tests/alacritty/wrapped_paths.rs"]
mod tests;
