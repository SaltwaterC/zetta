use super::*;
use gpui::TestAppContext;
use std::{cell::RefCell, rc::Rc};

#[test]
fn modifier_keys_do_not_consume_the_one_shot() {
    for key in ["shift", "control", "ctrl", "alt", "platform", "function"] {
        assert!(is_modifier_key(key));
    }
    for key in ["left", "right", "up", "down", "escape", "a", "f9"] {
        assert!(!is_modifier_key(key));
    }
}

#[test]
fn each_direction_and_other_bound_keys_go_to_the_terminal() {
    for key in ["left", "right", "up", "down", "tab", "f11", "a"] {
        assert_eq!(
            route_key(None, key),
            KeyRoute::Forward { first_press: true }
        );
    }
    assert_eq!(route_key(None, "escape"), KeyRoute::Cancel);
    assert_eq!(route_key(None, "shift"), KeyRoute::Ignore);
}

#[test]
fn only_repeats_of_the_first_key_continue_until_release() {
    assert_eq!(
        route_key(Some("left"), "left"),
        KeyRoute::Forward { first_press: false }
    );
    assert_eq!(route_key(Some("left"), "right"), KeyRoute::Ignore);
    assert_eq!(route_key(Some("left"), "escape"), KeyRoute::Ignore);
    assert_eq!(
        route_key(None, "right"),
        KeyRoute::Forward { first_press: true }
    );
}

#[gpui::test]
fn shift_f9_arms_the_focused_terminal_and_escape_cancels(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        TerminalSettings::init(cx);
        cx.bind_keys([
            KeyBinding::new("shift-f9", SendNextKeyToTerminal, Some("Zetta > Terminal")),
            KeyBinding::new("shift-f11", ToggleFullscreen, Some("Zetta > Terminal")),
        ]);
        cx.bind_keys([
            KeyBinding::new("alt-left", FocusPaneLeft, Some("Zetta > Terminal")),
            KeyBinding::new("alt-right", FocusPaneRight, Some("Zetta > Terminal")),
            KeyBinding::new("alt-up", FocusPaneUp, Some("Zetta > Terminal")),
            KeyBinding::new("alt-down", FocusPaneDown, Some("Zetta > Terminal")),
        ]);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.push(Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        });
        Zetta::new(
            config,
            None,
            ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        )
    });
    cx.run_until_parked();
    let received = Rc::new(RefCell::new(Vec::new()));
    let received_for_view = received.clone();
    let focused = zetta.update_in(cx, |zetta, window, cx| {
        let builder = TerminalBuilder::new_display_only(
            terminal::terminal_settings::CursorShape::Block,
            terminal::terminal_settings::AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view = cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), None, window, cx));
        view.update(cx, |view, _| view.set_emit_input_events(true));
        cx.subscribe_in(&view, window, move |_, _, event, _, _| {
            if let TerminalViewEvent::Input(input) = event {
                received_for_view.borrow_mut().push(input.clone());
            }
        })
        .detach();
        let tab = &mut zetta.tabs[zetta.active_tab];
        let pane = tab.pane_mut(tab.active_pane).unwrap();
        pane.terminal = Some(terminal);
        pane.view = Some(view);
        zetta.focus_active(window, cx);
        zetta.tabs[zetta.active_tab]
            .active_pane()
            .and_then(TerminalPane::selected_view)
            .is_some()
    });
    assert!(focused);
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-f9");
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_some()));
    assert!(cx.debug_bounds("key-passthrough-indicator").is_some());
    assert!(
        received.borrow().is_empty(),
        "the prefix must not reach the terminal"
    );
    cx.simulate_keystrokes("escape");
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_none()));
    assert!(cx.debug_bounds("key-passthrough-indicator").is_none());
    assert!(received.borrow().is_empty(), "Escape cancels without input");

    for key in [
        "alt-left",
        "alt-right",
        "alt-up",
        "alt-down",
        "shift-f11",
        "a",
    ] {
        cx.simulate_keystrokes("shift-f9");
        assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_some()));
        let count = received.borrow().len();
        cx.simulate_keystrokes(key);
        assert_eq!(
            received.borrow().len(),
            count + 1,
            "{key} must reach the terminal"
        );
        let sent = received
            .borrow()
            .last()
            .cloned()
            .expect("terminal input event");
        let expected = gpui::Keystroke::parse(key).unwrap();
        match sent {
            TerminalInput::Keystroke(sent) => {
                assert_eq!(sent.key, expected.key);
                assert_eq!(sent.modifiers, expected.modifiers);
            }
            TerminalInput::Text(text) if key == "a" => assert_eq!(text, "a"),
            _ => panic!("unexpected terminal input for {key}"),
        }
        if key == "alt-left" {
            cx.simulate_keystrokes(key);
            assert_eq!(
                received.borrow().len(),
                count + 2,
                "held repeat is forwarded"
            );
        }
        assert!(zetta.update(cx, |zetta, _| {
            zetta
                .key_passthrough
                .as_ref()
                .is_some_and(|state| state.held_key.is_some())
        }));
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyUp(KeyUpEvent {
                    keystroke: expected,
                }),
                cx,
            );
        });
        assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_none()));
    }
    let count = received.borrow().len();
    cx.simulate_keystrokes("alt-left");
    assert_eq!(
        received.borrow().len(),
        count,
        "unarmed binding keeps its action"
    );

    cx.simulate_keystrokes("shift-f9");
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_some()));
    let other_focus = zetta.update(cx, |zetta, _| zetta.rename_focus.clone());
    cx.update(|window, cx| {
        other_focus.focus(window, cx);
        assert!(other_focus.is_focused(window));
    });
    cx.run_until_parked();
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_none()));

    let other_window = cx.cx.add_window(|_, _| gpui::EmptyView);
    zetta.update_in(cx, |zetta, window, cx| zetta.focus_active(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-f9");
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_some()));
    cx.cx.simulate_keystrokes(other_window.into(), "alt-left");
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_some()));
    cx.simulate_keystrokes("escape");
    assert!(zetta.update(cx, |zetta, _| zetta.key_passthrough.is_none()));

    let second_pane = zetta.update(cx, |zetta, cx| {
        let tab = &mut zetta.tabs[zetta.active_tab];
        let first_pane = tab.active_pane;
        let second_pane = first_pane + 100;
        let profile = tab.active_pane().unwrap().profile.clone();
        assert!(tab.layout.split(
            first_pane,
            SplitAxis::Vertical,
            second_pane,
            SplitPosition::After,
        ));
        tab.pane_indices.insert(second_pane, tab.panes.len());
        tab.panes
            .push(TerminalPane::new(second_pane, profile).with_label_number(2));
        cx.notify();
        second_pane
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("alt-right");
    assert_eq!(
        zetta.update(cx, |zetta, _| zetta.tabs[zetta.active_tab].active_pane),
        second_pane
    );
}
