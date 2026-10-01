use super::*;

/// Content that fits draws no scrollbar at all. The Add profile modal used to
/// show a full-height thumb beside a form with nothing to scroll.
#[test]
fn a_region_with_nothing_to_scroll_has_no_thumb() {
    let track = gpui::Bounds::new(point(px(0.), px(0.)), gpui::size(px(10.), px(200.)));

    assert_eq!(scroll_thumb_bounds(&ScrollHandle::new(), track), None);
}
