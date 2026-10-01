use super::*;
use gpui::{
    Context, Modifiers, Render, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, point,
    px, size,
};

#[test]
fn key_hints_share_one_separator_and_order() {
    assert_eq!(
        key_hints(&[("Enter", "run"), ("Esc", "cancel")]).as_ref(),
        "Enter run · Esc cancel"
    );
    assert_eq!(key_hints(&[]).as_ref(), "");
}

/// A terminal-like layer under a modal, counting what reaches it.
struct BackdropHarness {
    click: fn() -> BackdropClick,
    scrolls_underneath: usize,
    clicks_underneath: usize,
    dismissed: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Render for BackdropHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ThemeColors::dark();
        let click = match (self.click)() {
            BackdropClick::Swallow => BackdropClick::Swallow,
            BackdropClick::Dismiss(_) => {
                let dismissed = self.dismissed.clone();
                BackdropClick::dismiss(move |_, _| dismissed.set(true))
            }
        };
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("underneath")
                    .absolute()
                    .inset_0()
                    .on_scroll_wheel(cx.listener(|this, _, _, _| this.scrolls_underneath += 1))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.clicks_underneath += 1),
                    ),
            )
            .child(modal(
                modal_backdrop("backdrop", Placement::Centered, click),
                modal_panel("panel", &colors)
                    .w(px(100.))
                    .h(px(100.))
                    .debug_selector(|| "panel".to_owned()),
            ))
    }
}

fn open_backdrop(
    click: fn() -> BackdropClick,
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<BackdropHarness>,
    &mut gpui::VisualTestContext,
    std::rc::Rc<std::cell::Cell<bool>>,
) {
    let dismissed = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = dismissed.clone();
    let (view, cx) = cx.add_window_view(move |_, _| BackdropHarness {
        click,
        scrolls_underneath: 0,
        clicks_underneath: 0,
        dismissed: flag,
    });
    cx.simulate_resize(size(px(400.), px(300.)));
    cx.run_until_parked();
    (view, cx, dismissed)
}

/// Only two of the thirteen modals used to occlude, so the wheel scrolled the
/// terminal behind the others.
#[gpui::test]
fn the_backdrop_keeps_the_wheel_and_clicks_from_the_terminal_underneath(cx: &mut TestAppContext) {
    let (view, cx, dismissed) = open_backdrop(|| BackdropClick::Swallow, cx);
    let outside = point(px(10.), px(10.));

    cx.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    cx.simulate_click(outside, Modifiers::default());

    cx.read_entity(&view, |view, _| {
        assert_eq!(view.scrolls_underneath, 0, "the wheel reached the terminal");
        assert_eq!(view.clicks_underneath, 0, "the click reached the terminal");
    });
    assert!(!dismissed.get(), "a swallowing backdrop does not dismiss");
}

#[gpui::test]
fn a_dismissing_backdrop_dismisses_on_a_click_outside_the_panel_only(cx: &mut TestAppContext) {
    let (_view, cx, dismissed) = open_backdrop(|| BackdropClick::dismiss(|_, _| {}), cx);
    let panel = cx.debug_bounds("panel").expect("the panel is laid out");

    cx.simulate_click(panel.center(), Modifiers::default());
    assert!(!dismissed.get(), "a click inside the panel stays inside it");

    cx.simulate_click(point(px(10.), px(10.)), Modifiers::default());
    assert!(dismissed.get());
}
