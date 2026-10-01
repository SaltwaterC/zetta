use super::*;

/// The values are pinned so a change to one is a decision rather than drift.
#[test]
fn the_shared_values_are_the_ones_the_surfaces_were_drawn_to() {
    assert!(
        SCRIM.is_transparent(),
        "modals have never dimmed the window; dimming them is a decision, not drift"
    );
    assert_eq!(RADIUS_SURFACE, px(8.));
    assert_eq!(RADIUS_CONTROL, px(4.));
    assert!(RADIUS_ROW < RADIUS_CONTROL);
    assert_eq!(DISABLED_OPACITY, 0.5);
    assert_eq!(CONTROL_COLUMN_WIDTH, px(330.));
    assert!(DENSE_ROW_MIN_HEIGHT < ROW_MIN_HEIGHT);
    assert!(DIALOG_WIDTH_SMALL < DIALOG_WIDTH_MEDIUM);
    assert!(DIALOG_WIDTH_MEDIUM < DIALOG_WIDTH_LARGE);
}
