use super::*;

#[test]
fn tab_move_context_menu_is_only_available_with_two_tabs() {
    assert!(!tab_move_menu_entry_available(0));
    assert!(!tab_move_menu_entry_available(1));
    assert!(tab_move_menu_entry_available(2));
}

#[test]
fn tab_silent_indicator_is_between_pin_and_custom_icon() {
    assert_eq!(
        tab_leading_icons(true, true, Some(IconName::Terminal), true),
        (
            Some(IconName::Pin),
            Some(IconName::BellOff),
            Some(IconName::Terminal)
        )
    );
    assert_eq!(
        tab_leading_icons(false, false, Some(IconName::Terminal), false),
        (None, None, None)
    );
    assert_eq!(
        tab_leading_icons(false, true, Some(IconName::Terminal), false),
        (None, Some(IconName::BellOff), None)
    );
}

#[test]
fn active_tab_shape_requires_compact_mode_and_selection() {
    assert!(active_tab_shape_visible(true, true));
    assert!(!active_tab_shape_visible(false, true));
    assert!(!active_tab_shape_visible(true, false));
}

#[test]
fn active_tab_transitions_inherit_each_neighbor_background() {
    let fallback = gpui::black();
    let left = gpui::red();
    let right = gpui::blue();

    assert_eq!(
        active_tab_transition_backgrounds(Some(left), Some(right), fallback),
        (left, right)
    );
    assert_eq!(
        active_tab_transition_backgrounds(None, Some(right), fallback),
        (fallback, right)
    );
    assert_eq!(
        active_tab_transition_backgrounds(Some(left), None, fallback),
        (left, fallback)
    );
}

#[test]
fn active_tab_top_corner_underlays_use_the_surrounding_backgrounds() {
    let radius = px(8.);
    let left_background = gpui::red();
    let right_background = gpui::blue();
    let mut left = render_active_tab_top_transition_background(true, left_background, radius);
    let mut right = render_active_tab_top_transition_background(false, right_background, radius);
    let positive_radius = gpui::Length::Definite(radius.into());

    assert_eq!(left.style().inset.left, Some(positive_radius));
    assert_eq!(right.style().inset.right, Some(positive_radius));
    assert_eq!(
        left.style().background.as_ref().and_then(gpui::Fill::color),
        Some(left_background.into())
    );
    assert_eq!(
        right
            .style()
            .background
            .as_ref()
            .and_then(gpui::Fill::color),
        Some(right_background.into())
    );
}

#[test]
fn active_tab_shape_body_resolves_to_the_full_measured_tab_width() {
    let radius = px(8.);
    let mut shape = render_active_tab_shape(
        true,
        true,
        true,
        true,
        gpui::white(),
        gpui::black(),
        gpui::black(),
        radius,
    );
    let mut body = render_active_tab_shape_base_fill(true, true, true, true, radius, gpui::white());
    let negative_radius = gpui::Length::Definite((-radius).into());
    let positive_radius = gpui::Length::Definite(radius.into());

    assert_eq!(shape.style().inset.left, Some(negative_radius));
    assert_eq!(shape.style().inset.right, Some(negative_radius));
    assert_eq!(body.style().inset.left, Some(positive_radius));
    assert_eq!(body.style().inset.right, Some(positive_radius));
    assert_eq!(
        body.style()
            .background
            .as_ref()
            .and_then(gpui::Fill::color)
            .and_then(|background| background.as_solid()),
        Some(gpui::white())
    );
}

#[test]
fn active_tab_bottom_transitions_stay_on_the_expanded_canvas_edges() {
    let radius = px(8.);
    let mut left =
        render_active_tab_bottom_transition(true, gpui::white(), Hsla::default(), radius);
    let mut right =
        render_active_tab_bottom_transition(false, gpui::white(), Hsla::default(), radius);
    let zero = gpui::Length::Definite(Pixels::ZERO.into());

    assert_eq!(left.style().inset.left, Some(zero));
    assert_eq!(right.style().inset.right, Some(zero));
}
