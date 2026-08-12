use super::*;

#[test]
fn compact_neighbor_control_mask_covers_the_tab_wing() {
    let background = gpui::white();
    let mut mask = compact_tab_neighbor_control_mask(background);
    let style = mask.style();

    assert_eq!(
        style.background.as_ref().and_then(gpui::Fill::color),
        Some(background.into())
    );
}

#[test]
fn compact_new_tab_button_removes_the_left_gap_without_changing_its_footprint() {
    let mut container = new_tab_button_container(true, px(36.));
    let style = container.style();

    assert_eq!(
        style.margin.left,
        Some(gpui::Length::Definite(Pixels::ZERO.into()))
    );
    assert_eq!(
        style.margin.right,
        Some(gpui::Length::Definite(px(12.).into()))
    );
}

#[test]
fn title_bar_controls_hide_labels_before_they_crowd_the_window() {
    assert!(!title_bar_shows_control_labels(
        px(719.),
        false,
        false,
        false
    ));
    assert!(title_bar_shows_control_labels(
        px(720.),
        false,
        false,
        false
    ));
    assert!(!title_bar_shows_control_labels(
        px(799.),
        true,
        false,
        false
    ));
    assert!(title_bar_shows_control_labels(px(800.), true, false, false));
    assert!(!title_bar_shows_control_labels(
        px(1000.),
        false,
        true,
        false
    ));
}

#[test]
fn compact_mode_hides_labels_and_regular_title_bar_buttons() {
    assert!(!title_bar_shows_control_labels(
        px(1000.),
        true,
        false,
        true
    ));
    assert!(!title_bar_buttons_visible(true, false));
    assert!(title_bar_buttons_visible(false, false));
    assert!(!title_bar_buttons_visible(false, true));
}

#[test]
fn compact_chrome_keeps_windowed_reservations() {
    assert_eq!(
        macos_title_bar_reservations_enabled(false),
        cfg!(target_os = "macos")
    );
    assert!(!compact_leading_controls_reservation_enabled(false, false));
    assert_eq!(
        compact_leading_controls_reservation_enabled(true, false),
        cfg!(target_os = "macos")
    );
    assert!(compact_drag_area_visible(true, false));
    assert_eq!(
        compact_drag_area_reserve_width(compact_drag_area_visible(true, false)),
        COMPACT_DRAG_AREA_MIN_WIDTH
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_fullscreen_compact_chrome_removes_reservations() {
    assert!(!macos_title_bar_reservations_enabled(true));
    assert!(!compact_leading_controls_reservation_enabled(true, true));
    assert!(!compact_drag_area_visible(true, true));
    assert_eq!(
        compact_drag_area_reserve_width(compact_drag_area_visible(true, true)),
        px(0.)
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_fullscreen_compact_chrome_keeps_reservations() {
    // The chrome assembly's platform-qualified fullscreen flag is always false
    // off macOS, even when the window itself is fullscreen.
    let is_macos_fullscreen = false;

    assert!(!macos_title_bar_reservations_enabled(is_macos_fullscreen));
    assert!(!compact_leading_controls_reservation_enabled(
        true,
        is_macos_fullscreen
    ));
    assert!(compact_drag_area_visible(true, is_macos_fullscreen));
    assert_eq!(
        compact_drag_area_reserve_width(compact_drag_area_visible(true, is_macos_fullscreen,)),
        COMPACT_DRAG_AREA_MIN_WIDTH
    );
}

#[test]
fn hiding_title_bar_buttons_hides_broadcast_in_compact_mode() {
    assert!(title_bar_broadcast_visible(false));
    assert!(!title_bar_broadcast_visible(true));
    assert!(title_bar_silent_visible(false));
    assert!(!title_bar_silent_visible(true));
}

#[test]
fn silent_title_bar_control_uses_the_bell_state_icons() {
    assert_eq!(title_bar_silent_icon(false), IconName::Bell);
    assert_eq!(title_bar_silent_icon(true), IconName::BellOff);
}

#[test]
fn reconnect_control_moves_to_the_right_when_title_bar_buttons_are_hidden() {
    assert!(title_bar_background_indicator_on_right(true, false, 1));
    assert!(title_bar_background_indicator_on_right(true, true, 1));
    assert!(title_bar_background_indicator_on_right(false, true, 1));
    assert!(!title_bar_background_indicator_on_right(false, false, 1));
    assert!(!title_bar_background_indicator_on_right(true, true, 0));
}

#[test]
fn compact_mode_hides_pane_size() {
    assert!(!title_bar_pane_size_visible(true, false));
    assert!(title_bar_pane_size_visible(false, false));
    assert!(!title_bar_pane_size_visible(false, true));
}

#[test]
fn title_bar_menu_visibility_is_platform_specific() {
    assert_eq!(
        title_bar_menus_visible(true),
        cfg!(not(target_os = "macos"))
    );
    assert!(title_bar_menus_visible(false));
}

#[test]
fn fallback_reconnect_control_is_icon_only() {
    assert_eq!(reconnect_control_label(false), "");
    assert_eq!(reconnect_control_label(true), "Reconnect");
}

#[test]
fn tab_container_keeps_responsive_flex_constraints() {
    let mut container = responsive_tab_container(div(), false, px(32.), false);
    let style = container.style();

    assert_eq!(style.size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.min_size.width, Some(TAB_MIN_WIDTH.into()));
    assert_eq!(style.max_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.flex_shrink, Some(1.));
}

#[test]
fn renaming_tab_restores_its_original_width() {
    let mut container = responsive_tab_container(div(), false, px(32.), true);
    let style = container.style();

    assert_eq!(style.min_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.max_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.flex_shrink, Some(0.));
}

#[test]
fn tabs_grow_to_fill_the_bar_in_compact_mode() {
    let mut container = responsive_tab_container(div(), true, px(32.), false);
    let style = container.style();

    assert_eq!(style.min_size.width, Some(TAB_MIN_WIDTH.into()));
    assert_eq!(style.max_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.flex_grow, Some(1.));
    assert_eq!(style.flex_shrink, Some(1.));
}

#[test]
fn a_renamed_tab_does_not_grow_even_in_compact_mode() {
    let mut container = responsive_tab_container(div(), true, px(32.), true);
    let style = container.style();

    assert_eq!(style.max_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.flex_grow, None);
    assert_eq!(style.flex_shrink, Some(0.));
}

#[test]
fn pinned_tab_container_uses_a_fixed_indicator_slot_and_expands_for_rename() {
    let mut container = pinned_tab_container(div(), false, px(32.), false);
    let style = container.style();
    assert_eq!(style.size.width, Some(PINNED_TAB_WIDTH.into()));
    assert_eq!(style.min_size.width, Some(PINNED_TAB_WIDTH.into()));
    assert_eq!(style.max_size.width, Some(PINNED_TAB_WIDTH.into()));
    assert_eq!(style.flex_grow, Some(0.));
    assert_eq!(style.flex_shrink, Some(0.));

    let mut renamed = pinned_tab_container(div(), false, px(32.), true);
    let renamed_style = renamed.style();
    assert_eq!(renamed_style.size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(renamed_style.min_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(renamed_style.max_size.width, Some(TAB_MAX_WIDTH.into()));
}

#[test]
fn tab_overflow_reserves_room_for_the_trigger() {
    assert_eq!(tab_bar_visible_tab_range(px(520.), 6, 0, false, None), 0..5);
    assert_eq!(tab_bar_visible_tab_range(px(520.), 5, 0, false, None), 0..5);
    assert_eq!(tab_bar_visible_tab_range(px(160.), 4, 0, false, None), 0..1);
    assert_eq!(tab_bar_visible_tab_range(px(520.), 0, 0, false, None), 0..0);
}

#[test]
fn pinned_tabs_can_leave_all_unpinned_tabs_in_overflow() {
    assert_eq!(
        tab_bar_visible_tab_range_with_pinned_tabs(px(0.), 3, 0, false, None, true),
        0..0
    );
    assert_eq!(
        tab_bar_visible_tab_range_with_pinned_tabs(px(160.), 3, 0, false, None, true),
        0..1
    );
}

#[test]
fn tab_icons_hide_when_tabs_start_shrinking() {
    assert!(!tab_bar_tabs_are_shrinking(px(764.), false, 4));
    assert!(tab_bar_tabs_are_shrinking(px(763.), false, 4));
    assert!(tab_bar_tabs_are_shrinking(px(1000.), true, 5));
    assert!(!tab_bar_tabs_are_shrinking(px(1000.), false, 0));
}

#[test]
fn renaming_tab_reserves_its_full_width() {
    assert_eq!(tab_bar_visible_tab_range(px(520.), 6, 5, true, None), 2..6);
    assert_eq!(tab_bar_visible_tab_range(px(520.), 3, 0, true, None), 0..3);
    assert_eq!(tab_bar_visible_tab_range(px(160.), 4, 0, true, None), 0..1);
}

#[test]
fn renaming_a_hidden_tab_temporarily_renders_it_in_the_tab_bar() {
    assert_eq!(tab_bar_visible_tab_range(px(520.), 6, 5, true, None), 2..6);
    assert_eq!(tab_bar_visible_tab_range(px(520.), 6, 0, true, None), 0..4);
}

#[test]
fn overflow_selection_places_tabs_at_the_selected_side() {
    assert_eq!(
        tab_bar_visible_tab_range(px(520.), 10, 7, false, Some(true)),
        3..8
    );
    assert_eq!(
        tab_bar_visible_tab_range(px(520.), 10, 2, false, Some(false)),
        2..7
    );
}

#[cfg(target_os = "macos")]
#[test]
fn profile_shortcut_alias_uses_unmapped_number_row_modifiers() {
    let keystroke = profile_shortcut_alias_keystroke(3);
    let inner = keystroke.inner();

    assert_eq!(inner.key, "3");
    assert!(inner.modifiers.control);
    assert!(inner.modifiers.shift);
    assert!(!inner.modifiers.platform);
}

#[cfg(target_os = "macos")]
#[test]
fn tenth_profile_shortcut_alias_uses_zero() {
    let keystroke = profile_shortcut_alias_keystroke(10);
    let inner = keystroke.inner();

    assert_eq!(inner.key, "0");
    assert!(inner.modifiers.control);
    assert!(inner.modifiers.shift);
    assert!(!inner.modifiers.platform);
}
