use super::*;
use gpui::TestAppContext;

struct DraftModalScrollHarness {
    scroll: ScrollHandle,
    focus_scroll_request: Option<(SettingsControl, Pixels)>,
}

impl Render for DraftModalScrollHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let controls = profile_draft_controls();
        let body_controls = &controls[..7];
        let body = div()
            .id("draft-body")
            .h(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex().children(body_controls.iter().enumerate().map(|(index, control)| {
                    track_focus_scroll_from(
                        div()
                            .h(px(40.))
                            .debug_selector(move || format!("draft-control-{index}"))
                            .child(div().h_full()),
                        self.focus_scroll_request.as_ref(),
                        &self.scroll,
                        std::slice::from_ref(control),
                    )
                })),
            );
        let footer = h_flex()
            .h(px(32.))
            .gap_2()
            .child(
                div()
                    .debug_selector(|| "draft-close".to_owned())
                    .child("Close"),
            )
            .child(
                div()
                    .debug_selector(|| "draft-create".to_owned())
                    .child("Create"),
            );
        div()
            .size_full()
            .p_4()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(220.))
                    .h(px(128.))
                    .p_3()
                    .flex()
                    .flex_col()
                    .child(body.debug_selector(|| "draft-body".to_owned()))
                    .child(footer),
            )
    }
}

#[gpui::test]
fn draft_modal_keeps_focused_controls_visible_in_a_small_viewport(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| DraftModalScrollHarness {
        scroll: ScrollHandle::new(),
        focus_scroll_request: None,
    });
    cx.simulate_resize(size(px(260.), px(160.)));
    cx.run_until_parked();

    let requested_offset = view.read_with(cx, |view, _| view.scroll.offset().y);
    view.update(cx, |view, cx| {
        view.focus_scroll_request = Some((
            SettingsControl::Dropdown(SettingsDropdown::ProfileDraftDarkTheme),
            requested_offset,
        ));
        cx.notify();
    });
    cx.run_until_parked();

    let body = cx.debug_bounds("draft-body").expect("draft body bounds");
    let focused = cx
        .debug_bounds("draft-control-6")
        .expect("focused draft control bounds");
    let offset = view.read_with(cx, |view, _| view.scroll.offset());
    assert!(
        focused.top() + offset.y >= body.top() && focused.bottom() + offset.y <= body.bottom(),
        "the focused draft control should remain visible: {focused:?} outside {body:?}"
    );

    let viewport = cx.update(|window, _| window.bounds());
    for id in ["draft-close", "draft-create"] {
        let bounds = cx.debug_bounds(id).expect("draft action bounds");
        assert!(
            bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom(),
            "draft action {id} should remain visible in the modal viewport: {bounds:?} outside {viewport:?}"
        );
    }
}
