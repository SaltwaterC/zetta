//! How a standalone terminal element turns the space it is given into a grid,
//! and the space a grid needs.
//!
//! The element lays out in two steps that are not a simple multiple of the
//! cell size: one cell of width is a gutter, and the height is snapped to whole
//! rows of the line height rounded to device pixels, so a 15.6px line takes
//! 16px of height at a scale of 1. Sizing a window for a grid by multiplying
//! the cell size out therefore missed rows, and a window can only be sized
//! before it is first shown, when there is no terminal yet to ask. This module
//! owns both directions, and the element uses the same functions, so the two
//! cannot drift apart.
//!
//! Zetta-authored, with no upstream counterpart.

use gpui::{
    App, Bounds, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString,
    Size, px,
};
use terminal::{TerminalBounds, terminal_settings::TerminalSettings};
use theme_settings::ThemeSettings;

use crate::terminal_element::{default_terminal_font_features, resolve_cell_width};

/// The terminal font as settings resolve it.
pub(crate) struct TerminalFont {
    pub(crate) family: SharedString,
    pub(crate) fallbacks: Option<FontFallbacks>,
    pub(crate) features: FontFeatures,
    pub(crate) weight: FontWeight,
    /// A multiple of the font size.
    pub(crate) line_height: f32,
    /// The size a standalone terminal uses when nothing has zoomed it.
    pub(crate) standalone_size: Pixels,
}

impl TerminalFont {
    pub(crate) fn from_settings(cx: &App) -> Self {
        // Borrowed, not cloned: the element calls this for every pane on every
        // frame, and only a few fields are read.
        let theme_settings = ThemeSettings::get_global(cx);
        let terminal_settings = TerminalSettings::get_global(cx);
        let buffer_font = &theme_settings.buffer_font;
        Self {
            family: terminal_settings.font_family.as_ref().map_or_else(
                || buffer_font.family.clone(),
                |font_family| font_family.0.clone().into(),
            ),
            fallbacks: terminal_settings
                .font_fallbacks
                .as_ref()
                .or(buffer_font.fallbacks.as_ref())
                .cloned(),
            // Zetta does not set `font_features`, so this fallback is the
            // default path rather than the rare one.
            features: terminal_settings
                .font_features
                .clone()
                .unwrap_or_else(default_terminal_font_features),
            weight: terminal_settings.font_weight.unwrap_or_default(),
            line_height: terminal_settings.line_height.value(),
            standalone_size: terminal_settings.font_size.map_or_else(
                || theme_settings.buffer_font_size(cx),
                |size| theme_settings::adjusted_font_size(size, cx),
            ),
        }
    }

    /// The font a `TextStyle` built from these fields resolves to.
    pub(crate) fn font(&self) -> Font {
        Font {
            family: self.family.clone(),
            features: self.features.clone(),
            fallbacks: self.fallbacks.clone(),
            weight: self.weight,
            style: FontStyle::Normal,
        }
    }
}

/// The cell a standalone terminal lays its grid out in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalCellMetrics {
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl TerminalCellMetrics {
    /// Terminal bounds holding exactly `columns` by `rows` of these cells, for
    /// a terminal that has to start at a grid before it is laid out.
    pub fn bounds_for_grid(self, columns: usize, rows: usize) -> TerminalBounds {
        TerminalBounds::new(
            self.line_height,
            self.cell_width,
            Bounds {
                origin: Default::default(),
                size: Size {
                    width: self.cell_width * columns as f32,
                    height: self.line_height * rows as f32,
                },
            },
        )
    }

    /// What a new standalone terminal view, one nothing has zoomed, uses.
    pub fn standalone(cx: &App) -> Self {
        let font = TerminalFont::from_settings(cx);
        Self {
            cell_width: resolve_cell_width(&font.font(), font.standalone_size, cx),
            line_height: px(f32::from(font.standalone_size) * font.line_height),
        }
    }
}

/// The height a standalone terminal draws in when given `available`: whole rows
/// of `line_height` rounded to device pixels. Returns that height and the
/// padding left below it.
pub(crate) fn snap_to_rows(
    available: Pixels,
    line_height: Pixels,
    scale_factor: f32,
) -> (Pixels, Pixels) {
    let line_height_device_px = (f32::from(line_height) * scale_factor).round().max(1.0) as i32;
    let available_device_px = (f32::from(available) * scale_factor).floor().max(0.0) as i32;
    let rows = (available_device_px / line_height_device_px).max(1);
    let snapped_device_px = rows * line_height_device_px;
    let padding_device_px = (available_device_px - snapped_device_px).max(0);
    (
        px(snapped_device_px as f32 / scale_factor.max(1.0)),
        px(padding_device_px as f32 / scale_factor.max(1.0)),
    )
}

/// The columns and rows a standalone terminal element of `size` lays out.
pub fn grid_for_element_size(
    size: Size<Pixels>,
    metrics: TerminalCellMetrics,
    scale_factor: f32,
) -> (usize, usize) {
    // One cell of gutter, and never fewer than two columns.
    let width = (size.width - metrics.cell_width).max(metrics.cell_width * 2.0);
    let (height, _) = snap_to_rows(size.height, metrics.line_height, scale_factor);
    let bounds = TerminalBounds::new(
        metrics.line_height,
        metrics.cell_width,
        Bounds {
            origin: Default::default(),
            size: Size { width, height },
        },
    );
    (bounds.num_columns(), bounds.num_lines())
}

/// The element size that lays out `columns` by `rows`, in the middle of the
/// range that does, so that a pixel of rounding either way keeps the grid.
///
/// With a line height that is not a whole number of device pixels some row
/// counts cannot be laid out at all; this then gives the next larger one.
pub fn element_size_for_grid(
    columns: usize,
    rows: usize,
    metrics: TerminalCellMetrics,
    scale_factor: f32,
) -> Size<Pixels> {
    let width = metrics.cell_width * (columns.max(2) as f32 + 1.5);
    let device_row = (f32::from(metrics.line_height) * scale_factor)
        .round()
        .max(1.0)
        / scale_factor;
    let mut device_rows = 1;
    let height = loop {
        let height = px(device_row * (device_rows as f32 + 0.5));
        let size = Size { width, height };
        if grid_for_element_size(size, metrics, scale_factor).1 >= rows {
            break height;
        }
        device_rows += 1;
    };
    Size { width, height }
}

#[cfg(test)]
#[path = "tests/grid_fit.rs"]
mod tests;
