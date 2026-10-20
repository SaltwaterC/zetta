use super::*;

#[test]
fn rows_are_found_by_display_row_and_single_unless_listed() {
    // Scrolled back two lines, grid line -1 is the second row on screen.
    let rows = DoubledRows::new(
        &[(-1, LineSize::DoubleWidth), (3, LineSize::DoubleHeightTop)],
        2,
    );
    assert_eq!(rows.size(1), LineSize::DoubleWidth);
    assert_eq!(rows.size(5), LineSize::DoubleHeightTop);
    assert_eq!(rows.size(0), LineSize::Single);
    assert_eq!([rows.column_scale(0), rows.column_scale(1)], [1., 2.]);
}

#[test]
fn only_the_first_half_of_a_doubled_row_is_shown() {
    let rows = DoubledRows::new(&[(0, LineSize::DoubleWidth)], 0);
    assert!(rows.shows(0, 3, 9));
    assert!(!rows.shows(0, 4, 9));
    assert!(rows.shows(1, 8, 9));
    assert!(DoubledRows::default().shows(0, 8, 9));
}

#[test]
fn row_bounds_stack_rows_from_the_grid_origin() {
    let bounds = row_bounds(
        point(gpui::px(5.), gpui::px(7.)),
        2,
        gpui::px(10.),
        gpui::px(80.),
    );
    assert_eq!(bounds.origin, point(gpui::px(5.), gpui::px(27.)));
    assert_eq!(bounds.size, size(gpui::px(80.), gpui::px(10.)));
}
