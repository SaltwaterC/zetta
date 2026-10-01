use super::*;
use gpui::{Context, Modifiers, Render, TestAppContext, px, size};
use std::{cell::Cell, rc::Rc};

struct ButtonHarness {
    enabled: bool,
    focused: bool,
    clicks: Rc<Cell<usize>>,
}

impl Render for ButtonHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ThemeColors::dark();
        let clicks = self.clicks.clone();
        h_flex().child(
            div()
                .flex_none()
                .debug_selector(|| "button".to_owned())
                .child(
                    DialogButton::new("button", "Save", ButtonRole::Primary)
                        .enabled(self.enabled)
                        .focused(self.focused)
                        .render(&colors, move |_, _, _| clicks.set(clicks.get() + 1)),
                ),
        )
    }
}

fn init_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(
            theme::LoadThemes::All(Box::new(crate::zetta_assets::ZettaAssets)),
            cx,
        );
        let registry = theme::ThemeRegistry::global(cx);
        theme::GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
    });
}

fn button_bounds(
    enabled: bool,
    focused: bool,
    cx: &mut TestAppContext,
) -> (gpui::Bounds<gpui::Pixels>, usize) {
    let clicks = Rc::new(Cell::new(0));
    let counter = clicks.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| ButtonHarness {
        enabled,
        focused,
        clicks: counter,
    });
    cx.simulate_resize(size(px(300.), px(100.)));
    cx.run_until_parked();
    let bounds = cx.debug_bounds("button").expect("the button is laid out");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.run_until_parked();
    (bounds, clicks.get())
}

#[gpui::test]
fn a_disabled_button_ignores_clicks(cx: &mut TestAppContext) {
    init_theme(cx);
    let (_, clicks) = button_bounds(true, false, cx);
    assert_eq!(clicks, 1);
    let (_, clicks) = button_bounds(false, false, cx);
    assert_eq!(clicks, 0);
}

/// The ring is drawn in both states, transparent when unfocused, so tabbing
/// onto a button never nudges the row it sits in.
#[gpui::test]
fn focusing_a_button_does_not_change_its_size(cx: &mut TestAppContext) {
    init_theme(cx);
    let (unfocused, _) = button_bounds(true, false, cx);
    let (focused, _) = button_bounds(true, true, cx);
    assert_eq!(unfocused.size, focused.size);
}

/// A key that did not parse would drop its chip from the tooltip silently.
#[test]
fn every_surface_key_parses_as_a_keystroke() {
    for key in [SurfaceKey::Escape, SurfaceKey::Enter] {
        assert!(Keystroke::parse(key.keystroke()).is_ok(), "{key:?}");
    }
}
