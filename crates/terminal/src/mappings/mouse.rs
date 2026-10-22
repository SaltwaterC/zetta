use std::cmp;
use std::iter::repeat_n;

/// Most of the code, and specifically the constants, in this are copied from Alacritty,
/// with modifications for our circumstances
use gpui::{Modifiers, MouseButton, Pixels, Point as GpuiPoint, ScrollEvent, px};

use crate::{LineSize, Modes, Point, SelectionSide, TerminalBounds};

enum MouseFormat {
    Sgr,
    SgrPixels,
    Normal(bool),
}

/// Where a mouse event happened, as a grid cell and as a 1-based device pixel
/// in the text area for SGR-Pixels (xterm 1016) reports.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MousePosition {
    pub cell: Point,
    pub pixel: (u32, u32),
}

impl MousePosition {
    /// `position` is relative to the text area's origin. Pixels are clamped to
    /// the text area, as cells are, so a drag outside it reports its edge.
    pub(crate) fn new(
        position: GpuiPoint<Pixels>,
        bounds: TerminalBounds,
        display_offset: usize,
        line_sizes: &[(i32, LineSize)],
        scale_factor: f32,
    ) -> Self {
        let device = |logical: Pixels, extent: Pixels| {
            let extent = (f32::from(extent) * scale_factor).max(1.0);
            (f32::from(logical) * scale_factor).clamp(0.0, extent - 1.0) as u32 + 1
        };
        Self {
            cell: grid_point(position, bounds, display_offset, line_sizes),
            pixel: (
                device(position.x, bounds.bounds.size.width),
                device(position.y, bounds.bounds.size.height),
            ),
        }
    }
}

impl MouseFormat {
    fn from_mode(mode: Modes) -> Self {
        if mode.contains(Modes::SGR_PIXEL_MOUSE) {
            MouseFormat::SgrPixels
        } else if mode.contains(Modes::SGR_MOUSE) {
            MouseFormat::Sgr
        } else if mode.contains(Modes::UTF8_MOUSE) {
            MouseFormat::Normal(true)
        } else {
            MouseFormat::Normal(false)
        }
    }
}

#[derive(Debug)]
enum MouseButtonCode {
    LeftButton = 0,
    MiddleButton = 1,
    RightButton = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
    Other = 99,
}

impl MouseButtonCode {
    fn from_move_button(e: Option<MouseButton>) -> Self {
        match e {
            Some(gpui::MouseButton::Left) => MouseButtonCode::LeftMove,
            Some(gpui::MouseButton::Middle) => MouseButtonCode::MiddleMove,
            Some(gpui::MouseButton::Right) => MouseButtonCode::RightMove,
            Some(gpui::MouseButton::Navigate(_)) => MouseButtonCode::Other,
            None => MouseButtonCode::NoneMove,
        }
    }

    fn from_button(e: MouseButton) -> Self {
        match e {
            gpui::MouseButton::Left => MouseButtonCode::LeftButton,
            gpui::MouseButton::Middle => MouseButtonCode::MiddleButton,
            gpui::MouseButton::Right => MouseButtonCode::RightButton,
            gpui::MouseButton::Navigate(_) => MouseButtonCode::Other,
        }
    }

    fn from_scroll(e: &ScrollEvent) -> Self {
        let is_positive = match e.delta {
            gpui::ScrollDelta::Pixels(pixels) => pixels.y > px(0.),
            gpui::ScrollDelta::Lines(lines) => lines.y > 0.,
        };

        if is_positive {
            MouseButtonCode::ScrollUp
        } else {
            MouseButtonCode::ScrollDown
        }
    }

    fn is_other(&self) -> bool {
        matches!(self, MouseButtonCode::Other)
    }
}

pub(crate) fn scroll_report(
    point: MousePosition,
    scroll_lines: i32,
    e: &ScrollEvent,
    mode: Modes,
) -> Option<impl Iterator<Item = Vec<u8>>> {
    if mode.intersects(Modes::MOUSE_MODE) {
        mouse_report(
            point,
            MouseButtonCode::from_scroll(e),
            true,
            e.modifiers,
            MouseFormat::from_mode(mode),
        )
        .map(|report| repeat_n(report, scroll_lines.unsigned_abs() as usize))
    } else {
        None
    }
}

pub(crate) fn alt_scroll(scroll_lines: i32) -> Vec<u8> {
    let cmd = if scroll_lines > 0 { b'A' } else { b'B' };

    let mut content = Vec::with_capacity(scroll_lines.unsigned_abs() as usize * 3);
    for _ in 0..scroll_lines.abs() {
        content.push(0x1b);
        content.push(b'O');
        content.push(cmd);
    }
    content
}

pub(crate) fn mouse_button_report(
    point: MousePosition,
    button: gpui::MouseButton,
    modifiers: Modifiers,
    pressed: bool,
    mode: Modes,
) -> Option<Vec<u8>> {
    let button = MouseButtonCode::from_button(button);
    if !button.is_other() && mode.intersects(Modes::MOUSE_MODE) {
        mouse_report(
            point,
            button,
            pressed,
            modifiers,
            MouseFormat::from_mode(mode),
        )
    } else {
        None
    }
}

pub(crate) fn mouse_moved_report(
    point: MousePosition,
    button: Option<MouseButton>,
    modifiers: Modifiers,
    mode: Modes,
) -> Option<Vec<u8>> {
    let button = MouseButtonCode::from_move_button(button);

    if !button.is_other() && mode.intersects(Modes::MOUSE_MOTION | Modes::MOUSE_DRAG) {
        //Only drags are reported in drag mode, so block NoneMove.
        if mode.contains(Modes::MOUSE_DRAG) && matches!(button, MouseButtonCode::NoneMove) {
            None
        } else {
            mouse_report(point, button, true, modifiers, MouseFormat::from_mode(mode))
        }
    } else {
        None
    }
}

pub(crate) fn grid_point(
    pos: GpuiPoint<Pixels>,
    cur_size: TerminalBounds,
    display_offset: usize,
    line_sizes: &[(i32, LineSize)],
) -> Point {
    grid_point_and_side(pos, cur_size, display_offset, line_sizes).0
}

/// The cell under `pos`. Zetta patch: a row in `line_sizes` (see
/// `Content::line_sizes`) draws each cell two columns wide.
pub(crate) fn grid_point_and_side(
    pos: GpuiPoint<Pixels>,
    cur_size: TerminalBounds,
    display_offset: usize,
    line_sizes: &[(i32, LineSize)],
) -> (Point, SelectionSide) {
    let mut line = (pos.y / cur_size.line_height) as i32;
    let bottommost_line = i32::try_from(cur_size.num_lines().saturating_sub(1)).unwrap_or(i32::MAX);
    let line_side = if line > bottommost_line {
        line = bottommost_line;
        Some(SelectionSide::Right)
    } else if line < 0 {
        Some(SelectionSide::Left)
    } else {
        None
    };
    let display_offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    let line = line.saturating_sub(display_offset);

    let line_size = line_sizes
        .iter()
        .find(|(doubled, _)| *doubled == line)
        .map_or(LineSize::Single, |(_, size)| *size);
    let cell_width = match line_size {
        LineSize::Single => cur_size.cell_width,
        _ => cur_size.cell_width * 2.0,
    };
    let mut column = (pos.x / cell_width) as usize;
    let cell_x = cmp::max(px(0.), pos.x) % cell_width;
    let mut side = if cell_x > cell_width / 2.0 {
        SelectionSide::Right
    } else {
        SelectionSide::Left
    };

    let last_column = line_size
        .visible_columns(cur_size.num_columns())
        .saturating_sub(1);
    if column > last_column {
        column = last_column;
        side = SelectionSide::Right;
    }
    (Point::new(line, column), line_side.unwrap_or(side))
}

///Generate the bytes to send to the terminal, from the cell location, a mouse event, and the terminal mode
fn mouse_report(
    position: MousePosition,
    button: MouseButtonCode,
    pressed: bool,
    modifiers: Modifiers,
    format: MouseFormat,
) -> Option<Vec<u8>> {
    let point = position.cell;
    if point.line < 0 {
        return None;
    }

    let mut mods = 0;
    if modifiers.shift {
        mods += 4;
    }
    if modifiers.alt {
        mods += 8;
    }
    if modifiers.control {
        mods += 16;
    }

    match format {
        MouseFormat::Sgr => {
            let (column, line) = (point.column + 1, point.line as usize + 1);
            Some(sgr_mouse_report((column, line), button as u8 + mods, pressed).into_bytes())
        }
        MouseFormat::SgrPixels => {
            let (x, y) = position.pixel;
            Some(
                sgr_mouse_report((x as usize, y as usize), button as u8 + mods, pressed)
                    .into_bytes(),
            )
        }
        MouseFormat::Normal(utf8) => {
            if pressed {
                normal_mouse_report(point, button as u8 + mods, utf8)
            } else {
                normal_mouse_report(point, 3 + mods, utf8)
            }
        }
    }
}

fn normal_mouse_report(point: Point, button: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };

    if point.line >= max_point || point.column >= max_point as usize {
        return None;
    }

    let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

    let mouse_pos_encode = |pos: usize| -> Vec<u8> {
        let pos = 32 + 1 + pos;
        let first = 0xC0 + pos / 64;
        let second = 0x80 + (pos & 63);
        vec![first as u8, second as u8]
    };

    if utf8 && point.column >= 95 {
        msg.append(&mut mouse_pos_encode(point.column));
    } else {
        msg.push(32 + 1 + point.column as u8);
    }

    if utf8 && point.line >= 95 {
        msg.append(&mut mouse_pos_encode(point.line as usize));
    } else {
        msg.push(32 + 1 + point.line as u8);
    }

    Some(msg)
}

/// `(x, y)` is 1-based: a cell for SGR, a device pixel for SGR-Pixels.
fn sgr_mouse_report((x, y): (usize, usize), button: u8, pressed: bool) -> String {
    let c = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{button};{x};{y}{c}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ScrollDelta, TouchPhase, point};

    #[test]
    fn scroll_report_repeats_for_negative_scroll_lines() {
        let grid_point = MousePosition {
            cell: Point::new(0, 0),
            pixel: (1, 1),
        };

        let scroll_event = ScrollEvent {
            delta: ScrollDelta::Lines(point(0., -1.)),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };

        let mode = Modes::MOUSE_MODE;
        let reports: Vec<Vec<u8>> = scroll_report(grid_point, -3, &scroll_event, mode)
            .expect("mouse mode should produce a scroll report")
            .collect();

        assert_eq!(reports.len(), 3);
    }

    #[test]
    fn scroll_report_repeats_for_positive_scroll_lines() {
        let grid_point = MousePosition {
            cell: Point::new(0, 0),
            pixel: (1, 1),
        };

        let scroll_event = ScrollEvent {
            delta: ScrollDelta::Lines(point(0., 1.)),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };

        let mode = Modes::MOUSE_MODE;
        let reports: Vec<Vec<u8>> = scroll_report(grid_point, 3, &scroll_event, mode)
            .expect("mouse mode should produce a scroll report")
            .collect();

        assert_eq!(reports.len(), 3);
    }

    #[test]
    fn sgr_pixel_reports_use_device_pixels_and_replace_cells() {
        let bounds = TerminalBounds::new(
            px(10.),
            px(5.),
            gpui::Bounds::new(point(px(0.), px(0.)), gpui::size(px(80.), px(20.))),
        );
        let position = MousePosition::new(point(px(12.25), px(7.5)), bounds, 0, &[], 2.0);
        assert_eq!((position.cell.column, position.cell.line), (2, 0));
        assert_eq!(position.pixel, (25, 16));

        let press = |mode| {
            mouse_button_report(
                position,
                MouseButton::Left,
                Modifiers::default(),
                true,
                mode,
            )
        };
        assert_eq!(
            press(Modes::MOUSE_REPORT_CLICK | Modes::SGR_PIXEL_MOUSE).unwrap(),
            b"\x1b[<0;25;16M"
        );
        assert_eq!(
            press(Modes::MOUSE_REPORT_CLICK | Modes::SGR_MOUSE).unwrap(),
            b"\x1b[<0;3;1M"
        );

        // A drag past the text area reports its last pixel, as cells clamp.
        let outside = MousePosition::new(point(px(-3.), px(400.)), bounds, 0, &[], 2.0);
        assert_eq!(outside.pixel, (1, 40));
    }

    #[test]
    fn a_doubled_row_maps_two_columns_to_each_cell_and_hides_its_second_half() {
        let bounds = TerminalBounds::new(
            px(10.),
            px(5.),
            gpui::Bounds::new(point(px(0.), px(0.)), gpui::size(px(80.), px(20.))),
        );
        // Scrolled back one line, the second screen row is grid line 0.
        let doubled = [(0, LineSize::DoubleWidth)];
        let at = |x, y| grid_point_and_side(point(px(x), px(y)), bounds, 1, &doubled);
        assert_eq!(at(12.0, 15.0), (Point::new(0, 1), SelectionSide::Left));
        assert_eq!(at(18.0, 15.0), (Point::new(0, 1), SelectionSide::Right));
        assert_eq!(at(79.0, 15.0), (Point::new(0, 7), SelectionSide::Right));
        assert_eq!(at(79.0, 5.0), (Point::new(-1, 15), SelectionSide::Right));
    }
}
