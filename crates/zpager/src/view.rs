//! Full-screen, read-only side-by-side diff pager and keyboard navigation.

use crate::diff_document::{
    DiffDocument, DiffLine, DiffRow, FileStart, LineKind, SideBySideLine, set_line_style,
};
use anyhow::{Context as _, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use std::io::{self, Write};
use std::time::Duration;
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

pub(crate) fn run(
    document: DiffDocument,
    color: bool,
    columns: usize,
    width: Option<usize>,
) -> Result<()> {
    let _terminal = TerminalGuard::enter()?;
    let mut state = ViewState {
        scroll: 0,
        cursor: 0,
        document,
        color,
        columns,
        width,
        layout_width: None,
        layout_columns: None,
        geometry: None,
        rows: Vec::new(),
        files: Vec::new(),
    };
    loop {
        state.draw()?;
        if !event::poll(Duration::from_millis(250)).context("failed to read terminal input")? {
            continue;
        }
        let Event::Key(key) = event::read().context("failed to read terminal input")? else {
            continue;
        };
        if !state.handle_key(key)? {
            break;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ViewCell {
    number: Option<LineNumber>,
    text: String,
    kind: LineKind,
}

#[derive(Clone, Debug)]
enum LineNumber {
    Single(usize),
    OldNew { old: usize, new: usize },
}

impl LineNumber {
    fn text(&self) -> String {
        match self {
            Self::Single(number) => number.to_string(),
            Self::OldNew { old, new } => format!("{old}/{new}"),
        }
    }

    fn display_width(&self) -> usize {
        match self {
            Self::Single(number) => number.to_string().len(),
            Self::OldNew { old, new } => old.to_string().len() + new.to_string().len() + 1,
        }
    }
}

#[derive(Clone, Debug)]
enum ViewRow {
    Full {
        text: String,
        kind: LineKind,
    },
    Sides {
        old: Option<ViewCell>,
        new: Option<ViewCell>,
    },
    Stacked {
        pane: Pane,
        cell: ViewCell,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Old,
    New,
    Shared,
}

impl Pane {
    fn label(self) -> &'static str {
        match self {
            Self::Old => "OLD",
            Self::New => "NEW",
            Self::Shared => "",
        }
    }
}

fn line_background(kind: LineKind, selected: bool) -> Option<Color> {
    // Keep the original diff colors on ordinary rows; use explicit RGB shades
    // for the cursor row so its cue remains visible across terminal palettes.
    match (kind, selected) {
        (LineKind::Added, true) => Some(Color::Rgb { r: 0, g: 48, b: 0 }),
        (LineKind::Added, false) => Some(Color::DarkGreen),
        (LineKind::Removed, true) => Some(Color::Rgb { r: 64, g: 0, b: 0 }),
        (LineKind::Removed, false) => Some(Color::DarkRed),
        (_, true) => Some(Color::DarkGrey),
        (_, false) => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutMode {
    SideBySide,
    Stacked,
}

#[derive(Clone, Copy, Debug)]
struct LayoutGeometry {
    content_x: u16,
    content_width: u16,
    mode: LayoutMode,
    old_x: u16,
    old_width: u16,
    divider_x: u16,
    new_x: u16,
    new_width: u16,
    side_text_width: usize,
    stacked_text_width: usize,
    stacked_number_width: usize,
    stacked_label_width: usize,
}

impl LayoutGeometry {
    fn new(
        width: u16,
        columns: usize,
        max_width: Option<usize>,
        number_width: usize,
        color: bool,
    ) -> Self {
        let width_usize = usize::from(width);
        let maximum_side_width = width_usize.saturating_sub(1) / 2;
        if columns > maximum_side_width {
            let content_width = width_usize.min(max_width.unwrap_or(width_usize));
            let content_x = (width_usize.saturating_sub(content_width)) / 2;
            let stacked_label_width = if color { 0 } else { 4 };
            let stacked_number_width =
                number_width.min(content_width.saturating_sub(stacked_label_width + 2));
            return Self {
                content_x: u16::try_from(content_x).unwrap_or(u16::MAX),
                content_width: u16::try_from(content_width).unwrap_or(u16::MAX),
                mode: LayoutMode::Stacked,
                old_x: 0,
                old_width: 0,
                divider_x: 0,
                new_x: 0,
                new_width: 0,
                side_text_width: 0,
                stacked_text_width: content_width
                    .saturating_sub(stacked_label_width + stacked_number_width + 1)
                    .max(1),
                stacked_number_width,
                stacked_label_width,
            };
        }

        let panel_width = u16::try_from(maximum_side_width.min(max_width.unwrap_or(usize::MAX)))
            .unwrap_or(u16::MAX);
        let content_width = panel_width.saturating_mul(2).saturating_add(1);
        let margin = width.saturating_sub(content_width) / 2;
        let old_x = margin;
        let divider_x = old_x.saturating_add(panel_width);
        let new_x = divider_x.saturating_add(1);
        let number_width = line_number_width(usize::from(panel_width));
        Self {
            content_x: margin,
            content_width,
            mode: LayoutMode::SideBySide,
            old_x,
            old_width: panel_width,
            divider_x,
            new_x,
            new_width: panel_width,
            side_text_width: usize::from(panel_width)
                .saturating_sub(number_width + 1)
                .max(1),
            stacked_text_width: 1,
            stacked_number_width: 0,
            stacked_label_width: 0,
        }
    }
}

struct ViewState {
    document: DiffDocument,
    scroll: usize,
    cursor: usize,
    color: bool,
    columns: usize,
    width: Option<usize>,
    layout_width: Option<u16>,
    layout_columns: Option<usize>,
    geometry: Option<LayoutGeometry>,
    rows: Vec<ViewRow>,
    files: Vec<FileStart>,
}

impl ViewState {
    fn draw(&mut self) -> Result<()> {
        let (width, height) = terminal::size().context("failed to read terminal size")?;
        let geometry = self.ensure_layout(width);
        let viewport_rows = usize::from(height.saturating_sub(3));
        self.keep_cursor_visible(viewport_rows);
        let mut output = io::stdout().lock();
        queue!(output, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
        self.draw_title(&mut output, geometry)?;
        self.draw_column_headers(&mut output, geometry)?;
        self.draw_viewport(&mut output, geometry, viewport_rows)?;
        self.draw_status(&mut output, width, height)?;
        output.flush().context("failed to draw terminal view")?;
        Ok(())
    }

    fn ensure_layout(&mut self, width: u16) -> LayoutGeometry {
        if self.layout_width == Some(width)
            && self.layout_columns == Some(self.columns)
            && let Some(geometry) = self.geometry
        {
            return geometry;
        }

        let geometry = LayoutGeometry::new(
            width,
            self.columns,
            self.width,
            stacked_line_number_width(&self.document),
            self.color,
        );
        let mut rows = Vec::new();
        let mut files = Vec::new();
        let mut next_file = 0;

        for (index, row) in self.document.rows.iter().enumerate() {
            if let Some(file) = self
                .document
                .files
                .get(next_file)
                .filter(|file| file.row == index)
            {
                files.push(FileStart {
                    row: rows.len(),
                    label: file.label.clone(),
                });
                next_file += 1;
            }
            rows.extend(wrap_row(row, geometry));
        }

        self.rows = rows;
        self.files = files;
        self.layout_width = Some(width);
        self.layout_columns = Some(self.columns);
        self.geometry = Some(geometry);
        geometry
    }

    fn keep_cursor_visible(&mut self, viewport_rows: usize) {
        if self.rows.is_empty() || viewport_rows == 0 {
            self.cursor = 0;
            self.scroll = 0;
            return;
        }

        self.cursor = self.cursor.min(self.rows.len() - 1);
        self.scroll = self.scroll.min(max_scroll(self.rows.len(), viewport_rows));
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + viewport_rows {
            self.scroll = self.cursor + 1 - viewport_rows;
        }
    }

    fn draw_title(&self, output: &mut impl Write, geometry: LayoutGeometry) -> Result<()> {
        let title = self.current_file().map_or_else(
            || format!("zpager · {} files", self.files.len()),
            |(index, file)| {
                format!(
                    "zpager · file {}/{} · {}",
                    index + 1,
                    self.files.len(),
                    file.label
                )
            },
        );
        if self.color {
            queue!(
                output,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold)
            )?;
        }
        queue!(
            output,
            MoveTo(geometry.content_x, 0),
            Print(truncate(&title, geometry.content_width))
        )?;
        if self.color {
            queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn draw_column_headers(&self, output: &mut impl Write, geometry: LayoutGeometry) -> Result<()> {
        if self.color {
            queue!(
                output,
                SetForegroundColor(Color::DarkGrey),
                SetAttribute(Attribute::Bold)
            )?;
        }
        queue!(output, MoveTo(0, 1), Clear(ClearType::CurrentLine))?;
        match geometry.mode {
            LayoutMode::SideBySide => queue!(
                output,
                MoveTo(geometry.old_x, 1),
                Print("OLD"),
                MoveTo(geometry.divider_x, 1),
                Print("│"),
                MoveTo(geometry.new_x, 1),
                Print("NEW")
            )?,
            LayoutMode::Stacked => {
                let title = if self.color {
                    "STACKED"
                } else {
                    "STACKED · OLD / NEW"
                };
                queue!(output, MoveTo(geometry.content_x, 1), Print(title))?;
            }
        }
        if self.color {
            queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn draw_viewport(
        &self,
        output: &mut impl Write,
        geometry: LayoutGeometry,
        viewport_rows: usize,
    ) -> Result<()> {
        for viewport_row in 0..viewport_rows {
            let row_index = self.scroll + viewport_row;
            let y = u16::try_from(viewport_row + 2).unwrap_or(u16::MAX);
            queue!(output, MoveTo(0, y), Clear(ClearType::CurrentLine))?;
            if let Some(row) = self.rows.get(row_index) {
                self.draw_row(output, row, geometry, y, row_index == self.cursor)?;
            }
        }
        Ok(())
    }

    fn draw_row(
        &self,
        output: &mut impl Write,
        row: &ViewRow,
        geometry: LayoutGeometry,
        y: u16,
        selected: bool,
    ) -> Result<()> {
        match row {
            ViewRow::Full { text, kind } => {
                self.draw_full_line(output, text, *kind, geometry, y, selected)
            }
            ViewRow::Sides { old, new } => {
                self.draw_side(
                    output,
                    geometry.old_x,
                    y,
                    geometry.old_width,
                    old.as_ref(),
                    selected,
                )?;
                self.draw_side(
                    output,
                    geometry.new_x,
                    y,
                    geometry.new_width,
                    new.as_ref(),
                    selected,
                )?;
                if self.color && selected {
                    queue!(
                        output,
                        SetForegroundColor(Color::White),
                        SetBackgroundColor(Color::DarkGrey)
                    )?;
                } else if self.color {
                    queue!(output, SetForegroundColor(Color::DarkGrey))?;
                } else if selected {
                    queue!(output, SetAttribute(Attribute::Reverse))?;
                }
                queue!(output, MoveTo(geometry.divider_x, y), Print("│"))?;
                if self.color || selected {
                    queue!(output, ResetColor)?;
                    queue!(output, SetAttribute(Attribute::Reset))?;
                }
                Ok(())
            }
            ViewRow::Stacked { pane, cell } => {
                self.draw_stacked_cell(output, geometry, y, *pane, cell, selected)
            }
        }
    }

    fn draw_full_line(
        &self,
        output: &mut impl Write,
        text: &str,
        kind: LineKind,
        geometry: LayoutGeometry,
        y: u16,
        selected: bool,
    ) -> Result<()> {
        if self.color {
            set_line_style(output, kind)?;
            if selected {
                queue!(
                    output,
                    SetForegroundColor(Color::White),
                    SetBackgroundColor(Color::DarkGrey),
                    SetAttribute(Attribute::Bold)
                )?;
            }
        } else if selected {
            queue!(output, SetAttribute(Attribute::Reverse))?;
        }
        let text = truncate(text, geometry.content_width);
        let padding = usize::from(geometry.content_width).saturating_sub(text.width());
        queue!(
            output,
            MoveTo(geometry.content_x, y),
            Print(text),
            Print(" ".repeat(padding))
        )?;
        if self.color || selected {
            queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn draw_side(
        &self,
        output: &mut impl Write,
        x: u16,
        y: u16,
        width: u16,
        cell: Option<&ViewCell>,
        selected: bool,
    ) -> Result<()> {
        if width == 0 {
            return Ok(());
        }
        let Some(cell) = cell else {
            if selected {
                if self.color {
                    queue!(
                        output,
                        SetForegroundColor(Color::White),
                        SetBackgroundColor(Color::DarkGrey)
                    )?;
                } else {
                    queue!(output, SetAttribute(Attribute::Reverse))?;
                }
                queue!(output, MoveTo(x, y), Print(" ".repeat(usize::from(width))))?;
                queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
            }
            return Ok(());
        };
        if self.color {
            let background = line_background(cell.kind, selected);
            if let Some(background) = background {
                queue!(
                    output,
                    SetForegroundColor(Color::White),
                    SetBackgroundColor(background),
                    SetAttribute(Attribute::Bold)
                )?;
            }
        } else if selected {
            queue!(output, SetAttribute(Attribute::Reverse))?;
        }

        let number_width = line_number_width(usize::from(width));
        let text_width = usize::from(width).saturating_sub(number_width + 1);
        let number = cell.number.as_ref().map_or_else(
            || " ".repeat(number_width),
            |number| format!("{:>number_width$}", number.text()),
        );
        let content = format!("{number} {}", truncate(&cell.text, text_width as u16));
        let content = truncate(&content, width);
        let padding = usize::from(width).saturating_sub(content.width());
        queue!(
            output,
            MoveTo(x, y),
            Print(content),
            Print(" ".repeat(padding))
        )?;
        if self.color || selected {
            queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn draw_stacked_cell(
        &self,
        output: &mut impl Write,
        geometry: LayoutGeometry,
        y: u16,
        pane: Pane,
        cell: &ViewCell,
        selected: bool,
    ) -> Result<()> {
        if self.color {
            let background = line_background(cell.kind, selected);
            if let Some(background) = background {
                queue!(
                    output,
                    SetForegroundColor(Color::White),
                    SetBackgroundColor(background),
                    SetAttribute(Attribute::Bold)
                )?;
            }
        } else if selected {
            queue!(output, SetAttribute(Attribute::Reverse))?;
        }

        let label = if geometry.stacked_label_width == 0 {
            String::new()
        } else {
            format!(
                "{:<4}",
                if pane == Pane::Shared {
                    ""
                } else {
                    pane.label()
                }
            )
        };
        let text = truncate(&cell.text, geometry.stacked_text_width as u16);
        let number = cell.number.as_ref();
        let number_text = number.map_or_else(String::new, LineNumber::text);
        let number_text = truncate(&number_text, geometry.stacked_number_width as u16);
        let number_padding = geometry
            .stacked_number_width
            .saturating_sub(number_text.width());
        let content_padding = usize::from(geometry.content_width).saturating_sub(
            geometry.stacked_label_width + geometry.stacked_number_width + 1 + text.width(),
        );
        queue!(output, MoveTo(geometry.content_x, y), Print(&label))?;
        if self.color
            && let Some(LineNumber::OldNew { old, new }) = number
            && (LineNumber::OldNew {
                old: *old,
                new: *new,
            })
            .display_width()
                <= geometry.stacked_number_width
        {
            queue!(
                output,
                Print(" ".repeat(number_padding)),
                SetForegroundColor(Color::Red),
                Print(old),
                SetForegroundColor(Color::DarkGrey),
                Print("/"),
                SetForegroundColor(Color::Green),
                Print(new),
            )?;
            if selected {
                queue!(output, SetForegroundColor(Color::White))?;
            } else {
                queue!(output, ResetColor)?;
            }
            queue!(
                output,
                Print(" "),
                Print(text),
                Print(" ".repeat(content_padding))
            )?;
        } else {
            let content = format!(
                "{number_text:>width$} {text}",
                width = geometry.stacked_number_width
            );
            let content = truncate(&content, geometry.content_width);
            let padding = usize::from(geometry.content_width).saturating_sub(content.width());
            queue!(output, Print(content), Print(" ".repeat(padding)))?;
        }
        if self.color || selected {
            queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn draw_status(&self, output: &mut impl Write, width: u16, height: u16) -> Result<()> {
        if height == 0 {
            return Ok(());
        }
        let view_rows = usize::from(height.saturating_sub(3));
        let end = (self.scroll + view_rows).min(self.rows.len());
        let file_status = self.current_file().map_or_else(
            || "no file".to_owned(),
            |(index, file)| format!("{}/{} {}", index + 1, self.files.len(), file.label),
        );
        let status = format!(
            " {file_status} · rows {}-{} of {} · ↑/↓ j/k move  n/p file  q quit ",
            self.scroll.saturating_add(1),
            end,
            self.rows.len()
        );
        if self.color {
            queue!(
                output,
                SetForegroundColor(Color::White),
                SetAttribute(Attribute::Bold)
            )?;
        }
        queue!(
            output,
            MoveTo(0, height - 1),
            Clear(ClearType::CurrentLine),
            Print(truncate(&status, width))
        )?;
        if self.color {
            queue!(output, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(false);
        }
        let page = terminal::size().map_or(1, |(_, height)| {
            usize::from(height.saturating_sub(3)).max(1)
        });
        let last_row = self.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = self.cursor.saturating_add(1).min(last_row);
            }
            KeyCode::PageUp => {
                self.cursor = self.cursor.saturating_sub(page);
                self.scroll = self.scroll.saturating_sub(page);
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.cursor = self.cursor.saturating_add(page).min(last_row);
                self.scroll = self.scroll.saturating_add(page);
            }
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor = last_row,
            KeyCode::Char('n') => self.next_file(),
            KeyCode::Char('p') => self.previous_file(),
            _ => {}
        }
        let viewport_rows =
            terminal::size().map_or(0, |(_, height)| usize::from(height.saturating_sub(3)));
        self.keep_cursor_visible(viewport_rows);
        Ok(true)
    }

    fn next_file(&mut self) {
        if let Some(next) = self.files.iter().find(|file| file.row > self.cursor) {
            self.cursor = next.row;
        }
    }

    fn previous_file(&mut self) {
        let Some((index, current)) = self.current_file() else {
            return;
        };
        if self.cursor > current.row {
            self.cursor = current.row;
        } else if let Some(previous) = index.checked_sub(1).and_then(|index| self.files.get(index))
        {
            self.cursor = previous.row;
        }
    }

    fn current_file(&self) -> Option<(usize, &FileStart)> {
        let index = self
            .files
            .partition_point(|file| file.row <= self.cursor)
            .checked_sub(1)?;
        self.files.get(index).map(|file| (index, file))
    }
}

fn wrap_row(row: &DiffRow, geometry: LayoutGeometry) -> Vec<ViewRow> {
    match row {
        DiffRow::Full(DiffLine { text, kind }) => {
            wrap_text(text, usize::from(geometry.content_width))
                .into_iter()
                .map(|text| ViewRow::Full { text, kind: *kind })
                .collect()
        }
        DiffRow::Sides { old, new } => match geometry.mode {
            LayoutMode::SideBySide => {
                let old = old
                    .as_ref()
                    .map(|line| wrap_side_line(line, geometry.side_text_width))
                    .unwrap_or_default();
                let new = new
                    .as_ref()
                    .map(|line| wrap_side_line(line, geometry.side_text_width))
                    .unwrap_or_default();
                let height = old.len().max(new.len());
                (0..height)
                    .map(|index| ViewRow::Sides {
                        old: old.get(index).cloned(),
                        new: new.get(index).cloned(),
                    })
                    .collect()
            }
            LayoutMode::Stacked => {
                let mut rows = Vec::new();
                if let (Some(old), Some(new)) = (old, new)
                    && old.kind == LineKind::Context
                    && new.kind == LineKind::Context
                    && old.text == new.text
                {
                    rows.extend(wrap_stacked_context(old, new, geometry.stacked_text_width));
                } else {
                    if let Some(old) = old {
                        rows.extend(wrap_stacked_line(
                            old,
                            Pane::Old,
                            geometry.stacked_text_width,
                        ));
                    }
                    if let Some(new) = new {
                        rows.extend(wrap_stacked_line(
                            new,
                            Pane::New,
                            geometry.stacked_text_width,
                        ));
                    }
                }
                rows
            }
        },
    }
}

fn wrap_side_line(line: &SideBySideLine, width: usize) -> Vec<ViewCell> {
    wrap_text(&line.text, width)
        .into_iter()
        .enumerate()
        .map(|(index, text)| ViewCell {
            number: (index == 0).then_some(LineNumber::Single(line.number)),
            text,
            kind: line.kind,
        })
        .collect()
}

fn wrap_stacked_line(line: &SideBySideLine, pane: Pane, width: usize) -> Vec<ViewRow> {
    wrap_text(&line.text, width)
        .into_iter()
        .enumerate()
        .map(|(index, text)| ViewRow::Stacked {
            pane,
            cell: ViewCell {
                number: (index == 0).then_some(LineNumber::Single(line.number)),
                text,
                kind: line.kind,
            },
        })
        .collect()
}

fn wrap_stacked_context(old: &SideBySideLine, new: &SideBySideLine, width: usize) -> Vec<ViewRow> {
    let number = if old.number == new.number {
        LineNumber::Single(old.number)
    } else {
        LineNumber::OldNew {
            old: old.number,
            new: new.number,
        }
    };
    wrap_text(&old.text, width)
        .into_iter()
        .enumerate()
        .map(|(index, text)| ViewRow::Stacked {
            pane: Pane::Shared,
            cell: ViewCell {
                number: (index == 0).then(|| number.clone()),
                text,
                kind: LineKind::Context,
            },
        })
        .collect()
}

fn stacked_line_number_width(document: &DiffDocument) -> usize {
    document
        .rows
        .iter()
        .filter_map(|row| {
            let DiffRow::Sides { old, new } = row else {
                return None;
            };
            let width = match (old, new) {
                (Some(old), Some(new))
                    if old.kind == LineKind::Context
                        && new.kind == LineKind::Context
                        && old.text == new.text =>
                {
                    if old.number == new.number {
                        LineNumber::Single(old.number).display_width()
                    } else {
                        LineNumber::OldNew {
                            old: old.number,
                            new: new.number,
                        }
                        .display_width()
                    }
                }
                (Some(old), Some(new)) => LineNumber::Single(old.number)
                    .display_width()
                    .max(LineNumber::Single(new.number).display_width()),
                (Some(line), None) | (None, Some(line)) => {
                    LineNumber::Single(line.number).display_width()
                }
                (None, None) => return None,
            };
            Some(width)
        })
        .max()
        .unwrap_or(1)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let text = sanitize_text(text);
    let characters = text.chars().collect::<Vec<_>>();
    if characters.is_empty() {
        return vec![String::new()];
    }

    let width = width.max(1);
    let mut wrapped = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let mut end = start;
        let mut columns = 0;
        while end < characters.len() {
            let character_width = characters[end].width().unwrap_or(0);
            if end > start && columns + character_width > width {
                break;
            }
            columns += character_width;
            end += 1;
            if columns >= width {
                break;
            }
        }
        end = end.max(start + 1);
        if end == characters.len() {
            wrapped.push(characters[start..end].iter().collect());
            break;
        }

        let break_at = (start..end)
            .rev()
            .find(|index| *index > start && characters[*index].is_whitespace());
        if let Some(break_at) = break_at {
            wrapped.push(characters[start..break_at].iter().collect());
            start = break_at;
            while start < characters.len() && characters[start].is_whitespace() {
                start += 1;
            }
        } else {
            wrapped.push(characters[start..end].iter().collect());
            start = end;
        }
    }
    wrapped
}

fn line_number_width(panel_width: usize) -> usize {
    panel_width.saturating_sub(2).min(5)
}

fn max_scroll(row_count: usize, viewport_rows: usize) -> usize {
    row_count.saturating_sub(viewport_rows)
}

fn sanitize_text(text: &str) -> String {
    let mut visible = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            if character == '\t' {
                visible.push_str("    ");
            } else {
                visible.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
        } else {
            visible.push(character);
        }
    }
    visible
}

fn truncate(text: &str, width: u16) -> String {
    let max_columns = usize::from(width);
    let mut columns = 0;
    let mut output = String::new();
    for character in sanitize_text(text).chars() {
        let character_width = character.width().unwrap_or(0);
        if columns + character_width > max_columns {
            break;
        }
        output.push(character);
        columns += character_width;
    }
    output
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable raw terminal mode")?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter alternate terminal screen");
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}
