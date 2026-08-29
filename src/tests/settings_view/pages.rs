use super::*;
use gpui::TestAppContext;

struct ProfileFieldsGridHarness;

impl Render for ProfileFieldsGridHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let fields = (0..4).map(|index| {
            div()
                .min_w(px(180.))
                .h_9()
                .debug_selector(move || format!("profile-field-{index}"))
                .into_any_element()
        });
        div()
            .w(px(488.))
            .p_3()
            .debug_selector(|| "profile-card".to_owned())
            .child(profile_fields_grid(fields))
    }
}

#[gpui::test]
fn profile_fields_stay_inside_the_card_at_the_minimum_dialog_width(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| ProfileFieldsGridHarness);
    cx.run_until_parked();

    let card = cx
        .debug_bounds("profile-card")
        .expect("profile card bounds");
    for (index, name) in [
        "profile-field-0",
        "profile-field-1",
        "profile-field-2",
        "profile-field-3",
    ]
    .into_iter()
    .enumerate()
    {
        let field = cx.debug_bounds(name).expect("profile field bounds");
        assert!(
            field.left() >= card.left() && field.right() <= card.right(),
            "profile field {index} must not overflow the card: {field:?} outside {card:?}"
        );
        assert!(
            field.size.width >= px(180.),
            "profile field {index} must remain usable instead of collapsing"
        );
    }
}

struct ProfileCardScrollHarness {
    scroll: ScrollHandle,
    focus_scroll_request: Option<(SettingsControl, Pixels)>,
}

impl Render for ProfileCardScrollHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let control = SettingsControl::Dropdown(SettingsDropdown::ProfileDarkTheme(0));
        let controls = profile_controls(0, true);
        let card = div()
            .h(px(60.))
            .w_full()
            .debug_selector(|| "profile-card".to_owned())
            .child(
                div()
                    .h(px(40.))
                    .debug_selector(|| "profile-card-control".to_owned()),
            );
        let tracked_card = track_focus_scroll_from(
            div().w_full().child(card),
            self.focus_scroll_request.as_ref(),
            &self.scroll,
            &controls,
        );
        div()
            .size_full()
            .id("profile-scroll")
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .child(
                        div()
                            .h(px(80.))
                            .debug_selector(|| "profile-card-top".to_owned()),
                    )
                    .child(tracked_card)
                    .child(
                        div()
                            .h(px(100.))
                            .debug_selector(|| "profile-card-bottom".to_owned()),
                    ),
            )
            .when(self.focus_scroll_request.is_some(), |view| {
                view.debug_selector(|| format!("profile-card-focus-{control:?}"))
            })
    }
}

#[gpui::test]
fn a_focused_profile_card_scrolls_as_a_complete_container(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| ProfileCardScrollHarness {
        scroll: ScrollHandle::new(),
        focus_scroll_request: None,
    });
    cx.simulate_resize(size(px(240.), px(100.)));
    cx.run_until_parked();

    let requested_offset = view.read_with(cx, |view, _| view.scroll.offset().y);
    view.update(cx, |view, cx| {
        view.focus_scroll_request = Some((
            SettingsControl::Dropdown(SettingsDropdown::ProfileDarkTheme(0)),
            requested_offset,
        ));
        cx.notify();
    });
    cx.run_until_parked();

    let card = cx
        .debug_bounds("profile-card")
        .expect("profile card bounds");
    let viewport = view.read_with(cx, |view, _| view.scroll.bounds());
    let maximum = view.read_with(cx, |view, _| view.scroll.max_offset());
    let offset = view.read_with(cx, |view, _| view.scroll.offset());
    assert!(
        card.top() + offset.y >= viewport.top() && card.bottom() + offset.y <= viewport.bottom(),
        "the complete card should be visible after focusing one of its controls: {card:?} outside {viewport:?}, max {maximum:?}, offset {offset:?}"
    );
}
