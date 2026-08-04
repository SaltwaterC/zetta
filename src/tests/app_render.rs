use super::*;

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
fn compact_mode_always_keeps_broadcast_visible() {
    assert!(title_bar_broadcast_visible(false, false));
    assert!(!title_bar_broadcast_visible(false, true));
    assert!(title_bar_broadcast_visible(true, true));
}

#[test]
fn compact_mode_keeps_background_indicator_on_the_right() {
    assert!(title_bar_background_indicator_on_right(
        !title_bar_buttons_visible(true, false),
        1
    ));
    assert!(title_bar_background_indicator_on_right(
        !title_bar_buttons_visible(true, true),
        1
    ));
    assert!(!title_bar_background_indicator_on_right(false, 1));
    assert!(!title_bar_background_indicator_on_right(true, 0));
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
fn minimized_shelf_capacity_fits_only_complete_entries() {
    assert_eq!(minimized_pane_capacity(px(0.), 4), 1);
    assert_eq!(minimized_pane_capacity(px(180.), 4), 1);
    assert_eq!(minimized_pane_capacity(px(363.), 4), 1);
    assert_eq!(minimized_pane_capacity(px(364.), 4), 2);
    assert_eq!(minimized_pane_capacity(px(1000.), 4), 4);
    assert_eq!(minimized_pane_capacity(px(1000.), 0), 0);
}

#[test]
fn minimized_shelf_keeps_selection_in_a_full_visible_page() {
    assert_eq!(visible_minimized_pane_range(5, 0, 3), 0..3);
    assert_eq!(visible_minimized_pane_range(5, 2, 3), 0..3);
    assert_eq!(visible_minimized_pane_range(5, 3, 3), 2..5);
    assert_eq!(visible_minimized_pane_range(5, 4, 3), 2..5);
}

#[test]
fn minimized_shelf_resolves_metadata_only_for_visible_entries() {
    let mut resolved = Vec::new();
    let entries = resolve_visible_minimized_panes(63, 62, 3, |index| {
        resolved.push(index);
        Some(index)
    });

    assert_eq!(entries, [60, 61, 62]);
    assert_eq!(resolved, [60, 61, 62]);
}

#[test]
fn tab_container_keeps_responsive_flex_constraints() {
    let mut container = responsive_tab_container(div(), false, px(32.));
    let style = container.style();

    assert_eq!(style.size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.min_size.width, Some(TAB_MIN_WIDTH.into()));
    assert_eq!(style.max_size.width, Some(TAB_MAX_WIDTH.into()));
    assert_eq!(style.flex_shrink, Some(1.));
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
