//! DEC double-width and double-height rows, as the terminal element draws them.
//!
//! The grid keeps a doubled row's cells at their ordinary columns, so layout
//! stays in grid columns and only painting doubles them: cell positions and
//! widths by two, and the row's text through `Window::with_text_scale`, which
//! stretches it as it goes into the scene. A double-height row is one half of a
//! line drawn twice as tall, so its text is also stretched downwards from the
//! edge that half is cut from, and clipped to the row. Only the first half of a
//! doubled row's cells fits, so the rest are left out of layout altogether.

use gpui::{Bounds, ContentMask, Pixels, Point, Window, point, size};
use terminal::LineSize;

/// The rows on screen that are not single width, by display row. Empty, which
/// is nearly always, it costs each cell one length check.
#[derive(Clone, Debug, Default)]
pub(crate) struct DoubledRows(Vec<(i32, LineSize)>);

impl DoubledRows {
    /// From a snapshot's `line_sizes`, which are by grid line.
    pub(crate) fn new(line_sizes: &[(i32, LineSize)], display_offset: usize) -> Self {
        let offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
        Self(
            line_sizes
                .iter()
                .map(|&(line, size)| (line.saturating_add(offset), size))
                .collect(),
        )
    }

    pub(crate) fn size(&self, row: i32) -> LineSize {
        self.0
            .iter()
            .find(|(doubled, _)| *doubled == row)
            .map_or(LineSize::Single, |(_, size)| *size)
    }

    /// How many screen columns one of the row's cells spans.
    pub(crate) fn column_scale(&self, row: i32) -> f32 {
        match self.size(row) {
            LineSize::Single => 1.,
            _ => 2.,
        }
    }

    /// Whether the cell in grid `column` of display `row` is on screen.
    pub(crate) fn shows(&self, row: i32, column: usize, columns: usize) -> bool {
        self.0.is_empty() || column < self.size(row).visible_columns(columns)
    }

    /// Paints `paint`, a row's text laid out at its ordinary size, as the row's
    /// size draws it. `anchor_x` is where the stretch starts: the grid's left
    /// edge for the row's own text, or a glyph's own position for one drawn on
    /// its own (the cursor's). `row` is the row's bounds on screen.
    pub(crate) fn paint_text<R>(
        &self,
        row_index: i32,
        anchor_x: Pixels,
        row: Bounds<Pixels>,
        window: &mut Window,
        paint: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let anchor_y = match self.size(row_index) {
            LineSize::Single => return paint(window),
            LineSize::DoubleWidth => {
                return window.with_text_scale(point(anchor_x, row.top()), size(2., 1.), paint);
            }
            LineSize::DoubleHeightTop => row.top(),
            LineSize::DoubleHeightBottom => row.bottom(),
        };
        window.with_content_mask(Some(ContentMask { bounds: row }), |window| {
            window.with_text_scale(point(anchor_x, anchor_y), size(2., 2.), paint)
        })
    }
}

/// The screen bounds of display `row` of a grid drawn from `origin`.
pub(crate) fn row_bounds(
    origin: Point<Pixels>,
    row: i32,
    line_height: Pixels,
    width: Pixels,
) -> Bounds<Pixels> {
    Bounds::new(
        point(origin.x, origin.y + row as f32 * line_height),
        size(width, line_height),
    )
}

#[cfg(test)]
#[path = "tests/line_size.rs"]
mod tests;
