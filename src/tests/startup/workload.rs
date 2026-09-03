use super::*;

#[test]
fn checkerboard_background_changes_every_cell_on_each_frame() {
    assert_ne!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 0, 1)
    );
    assert_ne!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 1, 0)
    );
    assert_eq!(
        checkerboard_background(0, 0, 0),
        checkerboard_background(0, 0, 2)
    );
}
