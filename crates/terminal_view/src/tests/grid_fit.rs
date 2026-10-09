use super::*;
use gpui::size;

/// DejaVu Sans Mono at 12px with a 1.3 line height, as Zetta lays it out.
const METRICS: TerminalCellMetrics = TerminalCellMetrics {
    cell_width: px(7.2246094),
    line_height: px(15.6),
};

/// The sizes measured in a running window: the element's height snaps to 16px
/// rows at a scale of 1, so its row count is not its height over 15.6px.
#[test]
fn a_grid_comes_from_the_space_the_element_draws_in() {
    let grid =
        |width: f32, height: f32| grid_for_element_size(size(px(width), px(height)), METRICS, 1.0);
    assert_eq!(grid(1078., 632.), (148, 40));
    assert_eq!(grid(602., 392.), (82, 24));
    // 382.2px holds 24.5 lines of 15.6px, but only 23 rows of 16px.
    assert_eq!(grid(589., 382.2), (80, 23));
}

/// Sizing a window for a grid has to land on that grid, whatever the scale and
/// font, or the window is resized again after it is shown.
#[test]
fn the_size_for_a_grid_lays_out_that_grid() {
    let fonts = [
        METRICS,
        TerminalCellMetrics {
            cell_width: px(8.),
            line_height: px(16.),
        },
        TerminalCellMetrics {
            cell_width: px(9.6),
            line_height: px(19.5),
        },
        TerminalCellMetrics {
            cell_width: px(6.6),
            line_height: px(14.3),
        },
    ];
    for metrics in fonts {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for columns in [2, 3, 80, 132, 241] {
                for rows in [1, 2, 24, 25, 39, 40, 61] {
                    let fitted = element_size_for_grid(columns, rows, metrics, scale);
                    let (got_columns, got_rows) = grid_for_element_size(fitted, metrics, scale);
                    assert_eq!(
                        got_columns, columns,
                        "{metrics:?} at {scale}: {columns}x{rows}"
                    );
                    // Some row counts cannot be drawn; then the next one up.
                    assert!(got_rows >= rows, "{metrics:?} at {scale}: {columns}x{rows}");
                    let reachable = (1..=rows + 2).any(|device_rows| {
                        let height = px(f32::from(metrics.line_height) * device_rows as f32);
                        grid_for_element_size(size(fitted.width, height), metrics, scale).1 == rows
                    });
                    if reachable {
                        assert_eq!(got_rows, rows, "{metrics:?} at {scale}: {columns}x{rows}");
                    }
                }
            }
        }
    }
}

/// The size sits in the middle of the range that lays out the grid, so a
/// window manager rounding it by a pixel does not cost a column or a row.
#[test]
fn the_size_for_a_grid_survives_a_pixel_either_way() {
    let fitted = element_size_for_grid(80, 24, METRICS, 1.0);
    for (width, height) in [(-1., -1.), (1., 1.), (-1., 1.), (1., -1.)] {
        let nudged = size(fitted.width + px(width), fitted.height + px(height));
        assert_eq!(grid_for_element_size(nudged, METRICS, 1.0), (80, 24));
    }
}

/// A terminal started at a grid before its first layout must report exactly
/// that grid, and the cell its first layout will use, so that layout changes
/// nothing the child can see.
#[test]
fn bounds_for_a_grid_hold_exactly_that_grid_in_the_laid_out_cell() {
    for (columns, rows) in [(2, 1), (80, 24), (132, 43), (241, 61)] {
        let bounds = METRICS.bounds_for_grid(columns, rows);
        assert_eq!((bounds.num_columns(), bounds.num_lines()), (columns, rows));
        assert_eq!(
            (bounds.cell_width(), bounds.line_height()),
            (METRICS.cell_width, METRICS.line_height)
        );
    }
}
