use super::*;
use gpui::{MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, TestAppContext, point, size};

#[test]
fn tab_move_context_menu_is_only_available_with_two_tabs() {
    assert!(!tab_move_menu_entry_available(0));
    assert!(!tab_move_menu_entry_available(1));
    assert!(tab_move_menu_entry_available(2));
}

#[test]
fn keep_running_context_menu_state_follows_tab_close_policy() {
    assert!(!tab_auto_background_enabled(&TabClosePolicy::Close));
    assert!(tab_auto_background_enabled(&TabClosePolicy::Background {
        authentication: None,
    }));
    assert!(tab_auto_background_enabled(&TabClosePolicy::Background {
        authentication: Some(SessionAuthentication::create("secret").unwrap()),
    }));
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
fn only_the_drop_surface_under_the_compact_lower_left_transition_is_deferred() {
    assert!(tab_drop_surface_needs_deferred_paint(
        true, 1, 2, true, true,
    ));
    assert!(!tab_drop_surface_needs_deferred_paint(
        false, 1, 2, true, true,
    ));
    assert!(!tab_drop_surface_needs_deferred_paint(
        true, 0, 2, true, true,
    ));
    assert!(!tab_drop_surface_needs_deferred_paint(
        true, 1, 2, false, true,
    ));
    assert!(!tab_drop_surface_needs_deferred_paint(
        true, 1, 2, true, false,
    ));
}

#[test]
fn active_tab_transitions_inherit_each_neighbor_background() {
    let fallback = gpui::black();
    let left = gpui::red();
    let right = gpui::blue();

    assert_eq!(
        active_tab_transition_backgrounds(Some(left), Some(right), None, fallback),
        (left, right)
    );
    assert_eq!(
        active_tab_transition_backgrounds(None, Some(right), None, fallback),
        (fallback, right)
    );
    assert_eq!(
        active_tab_transition_backgrounds(Some(left), None, None, fallback),
        (left, fallback)
    );
}

#[test]
fn first_active_tab_inherits_the_leading_title_bar_background() {
    let title_bar = gpui::green();
    let tab_bar = gpui::black();
    let neighboring_tab = gpui::red();

    assert_eq!(
        active_tab_transition_backgrounds(None, None, Some(title_bar), tab_bar),
        (title_bar, tab_bar)
    );
    assert_eq!(
        active_tab_transition_backgrounds(Some(neighboring_tab), None, Some(title_bar), tab_bar,),
        (neighboring_tab, tab_bar)
    );
}

#[test]
fn leading_background_only_applies_to_a_tab_touching_the_title_bar_controls() {
    let title_bar = gpui::green();

    assert_eq!(
        active_tab_left_edge_background(0, 0, 0, title_bar),
        Some(title_bar)
    );
    assert_eq!(
        active_tab_left_edge_background(0, 1, 2, title_bar),
        Some(title_bar)
    );
    assert_eq!(
        active_tab_left_edge_background(0, 0, 1, title_bar),
        None,
        "an overflow trigger is the leading neighbor"
    );
    assert_eq!(
        active_tab_left_edge_background(1, 1, 0, title_bar),
        None,
        "a preceding pinned tab is the leading neighbor"
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlappingTabLayer {
    DropSurface,
    ActiveTabWing,
}

struct DropSurfacePaintOrderView {
    clicked: Option<OverlappingTabLayer>,
}

impl DropSurfacePaintOrderView {
    fn layer(
        id: &'static str,
        layer: OverlappingTabLayer,
        color: u32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .absolute()
            .inset_0()
            .bg(gpui::rgb(color))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.clicked = Some(layer);
                cx.stop_propagation();
            }))
    }
}

impl Render for DropSurfacePaintOrderView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drop_surface = Self::layer(
            "overlapping-drop-surface",
            OverlappingTabLayer::DropSurface,
            0xff0000,
            cx,
        );
        let active_tab_wing = Self::layer(
            "overlapping-active-tab-wing",
            OverlappingTabLayer::ActiveTabWing,
            0x0000ff,
            cx,
        );

        div()
            .relative()
            .size_full()
            .child(tab_drop_surface_paint_layer(drop_surface, true))
            // Matches the real sibling order: the selected tab and its
            // expanded wing are rendered after the preceding tab's surface.
            .child(active_tab_wing)
    }
}

#[gpui::test]
fn deferred_drop_surface_paints_over_a_later_active_tab_wing(cx: &mut TestAppContext) {
    let window = cx.open_window(size(px(100.), px(100.)), |_, _| DropSurfacePaintOrderView {
        clicked: None,
    });
    cx.run_until_parked();

    cx.update(|cx| {
        cx.update_window(window.into(), |_, window, cx| {
            let position = point(px(50.), px(50.));
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
    });
    cx.run_until_parked();

    assert_eq!(
        window.update(cx, |view, _, _| view.clicked).unwrap(),
        Some(OverlappingTabLayer::DropSurface)
    );
}
