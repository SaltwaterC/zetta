//! The picker drawn in a real window: its popups, the layout that must not
//! jump as suggestions and transports change, focus following the keyboard,
//! and results that arrive after it was dismissed.

use super::*;

struct RemoteSessionEscapeHarness {
    zetta: Entity<Zetta>,
    picker_focus: FocusHandle,
    child_focus: FocusHandle,
    bubble_seen: Rc<Cell<bool>>,
}

impl Render for RemoteSessionEscapeHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let zetta = self.zetta.downgrade();
        let bubble_seen = self.bubble_seen.clone();
        let error_visible = self
            .zetta
            .read(cx)
            .remote_session_picker
            .as_ref()
            .is_some_and(|picker| picker.error.is_some());
        div()
            .size_full()
            .capture_key_down(move |event, window, cx| {
                zetta
                    .update(cx, |zetta, cx| {
                        zetta.remote_session_key_down_capture(event, window, cx);
                    })
                    .ok();
            })
            .child(
                div().size_full().track_focus(&self.picker_focus).child(
                    div()
                        .size_full()
                        .track_focus(&self.child_focus)
                        .when(error_visible, |child| {
                            child.child("Remote session validation error")
                        })
                        .on_key_down(move |_, _, cx| {
                            bubble_seen.set(true);
                            cx.stop_propagation();
                        }),
                ),
            )
    }
}

struct RemoteSessionDropdownHarness {
    zetta: Entity<Zetta>,
}

impl Render for RemoteSessionDropdownHarness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.zetta.downgrade();
        self.zetta
            .update(cx, |zetta, zetta_cx| {
                zetta.render_remote_session_overlay(
                    &ThemeColors::light(),
                    red(),
                    &handle,
                    window,
                    zetta_cx,
                )
            })
            .expect("the remote picker should be open")
    }
}

struct RemoteSessionKeyboardHarness {
    zetta: Entity<Zetta>,
    picker_focus: FocusHandle,
}

impl Render for RemoteSessionKeyboardHarness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.zetta.downgrade();
        let picker_focus = self.picker_focus.clone();
        let overlay = self
            .zetta
            .update(cx, |zetta, zetta_cx| {
                zetta.render_remote_session_overlay(
                    &ThemeColors::light(),
                    red(),
                    &handle,
                    window,
                    zetta_cx,
                )
            })
            .expect("the remote picker should be open");
        div()
            .track_focus(&picker_focus)
            .capture_key_down(move |event, window, cx| {
                handle
                    .update(cx, |zetta, cx| {
                        zetta.remote_session_key_down(event, window, cx);
                    })
                    .ok();
            })
            .child(overlay)
    }
}

struct RemoteSessionSuggestionsHarness {
    suggestions: Vec<String>,
    scroll: UniformListScrollHandle,
    selected: Option<usize>,
}

impl Render for RemoteSessionSuggestionsHarness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let suggestions = remote_session_suggestion_rows(
            &gpui::WeakEntity::<Zetta>::new_invalid(),
            &ThemeColors::light(),
            &self.scroll,
            self.suggestions.clone(),
            self.selected,
        );
        let suggestions = remote_session_suggestion_list(suggestions, &self.scroll, window, cx);
        div().size_full().child(
            div()
                .w(px(320.))
                .flex_none()
                .debug_selector(|| "remote-session-suggestions-region".to_owned())
                .child(suggestions),
        )
    }
}

#[gpui::test]
fn remote_picker_capture_escape_dismisses_error_and_ignores_stale_results(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let bubble_seen = Rc::new(Cell::new(false));
    let bubble_seen_for_view = bubble_seen.clone();
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker = Some(RemoteSessionPicker {
                error: Some("enter an SSH target".to_owned()),
                ..Default::default()
            });
            zetta
        });
        let picker_focus = zetta.read(cx).remote_session_focus.clone();
        RemoteSessionEscapeHarness {
            zetta,
            picker_focus,
            child_focus: cx.focus_handle(),
            bubble_seen: bubble_seen_for_view,
        }
    });
    let zetta = harness.update(cx, |harness, _| harness.zetta.clone());
    let child_focus = harness.update(cx, |harness, _| harness.child_focus.clone());
    harness.update_in(cx, |harness, window, cx| {
        harness.child_focus.focus(window, cx);
    });
    cx.run_until_parked();

    let validation_restored_focus = zetta.update_in(cx, |zetta, window, cx| {
        zetta.load_remote_sessions(window, cx);
        window.focused(cx) == Some(zetta.remote_session_focus.clone())
    });
    assert!(
        validation_restored_focus,
        "validation errors should return focus to the remote picker"
    );

    harness.update_in(cx, |harness, window, cx| {
        harness.child_focus.focus(window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("x");
    assert!(
        bubble_seen.get(),
        "the focused child should stop ordinary key events in the bubble phase"
    );

    bubble_seen.set(false);
    cx.simulate_keystrokes("escape");
    assert!(
        !bubble_seen.get(),
        "capture-phase Escape should run before the focused child can stop propagation"
    );
    assert!(zetta.update(cx, |zetta, _| zetta.remote_session_picker.is_none()));

    zetta.update(cx, |zetta, _| {
        let mut picker = RemoteSessionPicker {
            field: RemoteSessionField::Profile,
            profiles: vec!["System".to_owned()],
            ..Default::default()
        };
        assert!(picker.open_dropdown(RemoteSessionDropdown::Profile, Point::default()));
        zetta.remote_session_picker = Some(picker);
    });
    harness.update_in(cx, |harness, window, cx| {
        harness.child_focus.focus(window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    assert!(zetta.update(cx, |zetta, _| {
        zetta
            .remote_session_picker
            .as_ref()
            .is_some_and(|picker| picker.open_dropdown.is_none())
    }));
    cx.simulate_keystrokes("escape");
    assert!(zetta.update(cx, |zetta, _| zetta.remote_session_picker.is_none()));

    zetta.update(cx, |zetta, _| {
        zetta.remote_session_picker = Some(RemoteSessionPicker {
            generation: 7,
            ..Default::default()
        });
    });
    let stale_result_preserved_focus = zetta.update_in(cx, |zetta, window, cx| {
        zetta.dismiss_remote_session_picker(window, cx);
        child_focus.focus(window, cx);
        zetta.apply_remote_session_result(
            7,
            Err(anyhow::anyhow!("late remote result")),
            window,
            cx,
        );
        window.focused(cx) == Some(child_focus.clone())
    });
    assert!(
        stale_result_preserved_focus,
        "a result for a dismissed picker must not restore its focus"
    );
    assert!(zetta.update(cx, |zetta, _| zetta.remote_session_picker.is_none()));
}

#[gpui::test]
fn remote_dropdown_triggers_render_an_anchored_popup_and_commit_selection(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker = Some(RemoteSessionPicker {
                field: RemoteSessionField::Profile,
                profiles: vec!["System".to_owned(), "Remote shell".to_owned()],
                templates: vec![
                    RemoteSessionTemplate::SinglePane,
                    RemoteSessionTemplate::Configured("one-right".to_owned()),
                ],
                ..Default::default()
            });
            zetta
        });
        RemoteSessionDropdownHarness { zetta }
    });
    cx.simulate_resize(size(px(720.), px(600.)));
    cx.run_until_parked();

    assert!(cx.debug_bounds("remote-session-profile-trigger").is_some());
    let template_trigger = cx
        .debug_bounds("remote-session-template-trigger")
        .expect("the template trigger should be laid out");
    cx.simulate_click(template_trigger.center(), Default::default());
    cx.run_until_parked();

    let popup = cx
        .debug_bounds("remote-session-dropdown-Template-options")
        .expect("the template popup should be anchored and visible");
    assert!(
        cx.debug_bounds("dropdown-first-option").is_some(),
        "the popup should render visible option rows"
    );
    let configured_option = cx
        .debug_bounds("dropdown-second-option")
        .expect("the configured template row should be visible");
    assert!(configured_option.origin.x >= popup.origin.x);
    cx.simulate_click(configured_option.center(), Default::default());
    cx.run_until_parked();

    let (selected_template, popup_closed) = harness.update(cx, |harness, cx| {
        let picker = harness
            .zetta
            .read(cx)
            .remote_session_picker
            .as_ref()
            .expect("the picker should remain open after selection");
        (picker.selected_template, picker.open_dropdown.is_none())
    });
    assert_eq!(selected_template, 1);
    assert!(popup_closed);
}

#[gpui::test]
fn remote_description_reserves_two_lines_for_every_transport(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker = Some(RemoteSessionPicker::default());
            zetta
        });
        RemoteSessionDropdownHarness { zetta }
    });
    cx.simulate_resize(size(px(720.), px(600.)));
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("remote-session-forward-agent").is_some(),
        "SSH should show the agent-forwarding control"
    );
    let ssh_description_height = cx
        .debug_bounds("remote-session-description")
        .expect("the SSH description should be rendered")
        .size
        .height;
    let ssh_target_y = cx
        .debug_bounds("remote-session-target")
        .expect("the SSH target should be rendered")
        .origin
        .y;

    let zetta = harness.update(cx, |harness, _| harness.zetta.clone());
    zetta.update(cx, |zetta, cx| {
        zetta
            .remote_session_picker
            .as_mut()
            .expect("the picker should remain open")
            .toggle_transport();
        cx.notify();
    });
    cx.run_until_parked();

    let zosh_description_height = cx
        .debug_bounds("remote-session-description")
        .expect("the Zosh description should be rendered")
        .size
        .height;
    let zosh_target_y = cx
        .debug_bounds("remote-session-target")
        .expect("the Zosh target should be rendered")
        .origin
        .y;

    assert_eq!(ssh_description_height, px(45.));
    assert_eq!(zosh_description_height, px(45.));
    assert_eq!(ssh_target_y, zosh_target_y);
}

#[gpui::test]
fn hiding_host_suggestions_does_not_resize_the_remote_picker(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker = Some(picker_with_suggestions(""));
            zetta
        });
        RemoteSessionDropdownHarness { zetta }
    });
    cx.simulate_resize(size(px(720.), px(600.)));
    cx.run_until_parked();

    let target_focused_bounds = cx
        .debug_bounds("remote-session-picker")
        .expect("the picker should be rendered");
    let picker_right = target_focused_bounds.origin.x + target_focused_bounds.size.width;
    for selector in [
        "remote-session-cancel-action",
        "remote-session-load-action",
        "remote-session-create-action",
        "remote-session-attach-action",
    ] {
        let action = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} should be rendered"));
        assert!(
            action.origin.x + action.size.width <= picker_right,
            "{selector} should stay inside the picker"
        );
    }

    let zetta = harness.update(cx, |harness, _| harness.zetta.clone());
    zetta.update(cx, |zetta, cx| {
        let picker = zetta
            .remote_session_picker
            .as_mut()
            .expect("the picker should remain open");
        picker.toggle_transport();
        picker.field = RemoteSessionField::Protocol;
        cx.notify();
    });
    cx.run_until_parked();

    let protocol_focused_bounds = cx
        .debug_bounds("remote-session-picker")
        .expect("the picker should remain rendered");
    assert_eq!(target_focused_bounds, protocol_focused_bounds);
}

#[gpui::test]
fn remote_transport_controls_have_the_same_height(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker = Some(RemoteSessionPicker {
                transport: RemotePaneTransport::Zosh {
                    keep_alive_ms: Some(REMOTE_KEEP_ALIVE_DEFAULT_MS),
                    forward_agent: false,
                },
                keep_alive: TextField::new(REMOTE_KEEP_ALIVE_DEFAULT_MS.to_string()),
                ..Default::default()
            });
            zetta
        });
        RemoteSessionDropdownHarness { zetta }
    });
    cx.simulate_resize(size(px(720.), px(600.)));
    cx.run_until_parked();

    let heights = [
        "remote-session-protocol",
        "remote-session-keep-alive-control",
        "remote-session-forward-agent",
    ]
    .map(|selector| {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} should be rendered"))
            .size
            .height
    });
    assert_eq!(heights, [px(36.); 3]);
}

#[gpui::test]
fn remote_action_buttons_follow_keyboard_focus_in_the_real_overlay(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker = Some(RemoteSessionPicker {
                target: TextField::new("dev.example"),
                profiles: vec!["System".to_owned()],
                sessions: vec![remote_session_summary(4, false)],
                field: RemoteSessionField::List,
                ..Default::default()
            });
            zetta
        });
        RemoteSessionKeyboardHarness {
            picker_focus: zetta.read(cx).remote_session_focus.clone(),
            zetta,
        }
    });
    cx.simulate_resize(size(px(720.), px(600.)));
    harness.update_in(cx, |harness, window, cx| {
        harness.picker_focus.focus(window, cx);
    });
    cx.run_until_parked();

    let help = cx
        .debug_bounds("remote-session-action-help")
        .expect("remote-session action help should be rendered");
    let actions = cx
        .debug_bounds("remote-session-actions")
        .expect("remote-session actions should be rendered");
    assert!(
        help.bottom() <= actions.top(),
        "remote-session action help should be above the buttons: {help:?} vs {actions:?}"
    );

    for (expected, selector) in [
        (RemoteSessionField::Cancel, "remote-session-cancel-action"),
        (RemoteSessionField::Load, "remote-session-load-action"),
        (RemoteSessionField::Create, "remote-session-create-action"),
        (RemoteSessionField::Attach, "remote-session-attach-action"),
    ] {
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        let field = harness.update(cx, |harness, cx| {
            harness
                .zetta
                .read(cx)
                .remote_session_picker
                .as_ref()
                .expect("the picker should remain open")
                .field
        });
        assert_eq!(field, expected);
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the focused action should remain rendered"
        );
    }
}

#[gpui::test]
fn a_dismissed_remote_attach_cannot_fill_a_reopened_picker(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
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
        RemoteSessionEscapeHarness {
            picker_focus: zetta.read(cx).remote_session_focus.clone(),
            child_focus: cx.focus_handle(),
            zetta,
            bubble_seen: Rc::new(Cell::new(false)),
        }
    });
    let zetta = harness.update(cx, |harness, _| harness.zetta.clone());

    let reopened_picker_survives = zetta.update_in(cx, |zetta, window, cx| {
        let old_generation = zetta.next_remote_session_operation_generation();
        zetta.remote_session_picker = Some(RemoteSessionPicker {
            target: TextField::new("old-host"),
            sessions: vec![remote_session_summary(7, true)],
            attach_generation: Some(old_generation),
            loading: true,
            ..Default::default()
        });
        assert!(
            zetta
                .remote_session_picker
                .as_ref()
                .is_some_and(|picker| picker.loading)
        );

        zetta.dismiss_remote_session_picker(window, cx);
        zetta.open_remote_session(&OpenRemoteSession, window, cx);
        zetta.apply_remote_attach_result(
            old_generation,
            zmux::remote::RemoteTarget::new("old-host"),
            remote_session_summary(7, true),
            Ok(RemoteAttachOutcome::AuthenticationRequired),
            window,
            cx,
        );
        zetta.remote_session_picker.is_some() && zetta.session_authentication.is_none()
    });
    assert!(reopened_picker_survives);
}

#[gpui::test]
fn loaded_sessions_move_the_picker_onto_the_list(cx: &mut TestAppContext) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
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
        RemoteSessionEscapeHarness {
            picker_focus: zetta.read(cx).remote_session_focus.clone(),
            child_focus: cx.focus_handle(),
            zetta,
            bubble_seen: Rc::new(Cell::new(false)),
        }
    });
    let zetta = harness.update(cx, |harness, _| harness.zetta.clone());

    let (field, suggestion_navigation_cleared) = zetta.update_in(cx, |zetta, window, cx| {
        let mut picker = picker_with_suggestions("prod");
        picker.generation = 11;
        picker.navigate_suggestions(false);
        picker.generation = 11;
        zetta.remote_session_picker = Some(picker);
        zetta.apply_remote_session_result(
            11,
            Ok(vec![
                remote_session_summary(3, false),
                remote_session_summary(5, false),
            ]),
            window,
            cx,
        );
        let picker = zetta
            .remote_session_picker
            .as_ref()
            .expect("a successful load keeps the picker open");
        (picker.field, picker.suggestion_navigation.is_none())
    });
    assert_eq!(
        field,
        RemoteSessionField::List,
        "loaded sessions should put Enter and the arrow keys on the list"
    );
    assert!(suggestion_navigation_cleared);

    let (field, error) = zetta.update_in(cx, |zetta, window, cx| {
        let picker = zetta
            .remote_session_picker
            .as_mut()
            .expect("the picker is still open");
        picker.generation = 12;
        picker.field = RemoteSessionField::Target;
        zetta.apply_remote_session_result(12, Ok(Vec::new()), window, cx);
        let picker = zetta
            .remote_session_picker
            .as_ref()
            .expect("an empty load keeps the picker open");
        (picker.field, picker.error.clone())
    });
    assert_eq!(
        field,
        RemoteSessionField::Target,
        "a host with no shared sessions should leave the user on the target field"
    );
    assert!(error.is_some());
}

#[gpui::test]
fn ssh_host_suggestions_keep_all_aliases_in_a_six_row_viewport(cx: &mut TestAppContext) {
    cx.update(|cx| theme::init(theme::LoadThemes::JustBase, cx));
    let aliases = (0..12)
        .map(|index| format!("host-{index}"))
        .collect::<Vec<_>>();
    let picker = RemoteSessionPicker {
        suggestions: aliases.clone(),
        ..Default::default()
    };
    assert_eq!(picker.visible_suggestions(), aliases);

    let scroll = UniformListScrollHandle::new();
    let scroll_for_view = scroll.clone();
    let aliases_for_view = picker.suggestions.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| RemoteSessionSuggestionsHarness {
        suggestions: aliases_for_view,
        scroll: scroll_for_view,
        selected: None,
    });
    cx.simulate_resize(size(px(400.), px(300.)));
    cx.run_until_parked();

    let region = cx
        .debug_bounds("remote-session-suggestions-region")
        .expect("the SSH suggestion list should be laid out");
    assert!(
        region.size.height <= px(168.),
        "the suggestion viewport must be capped at six rows: {region:?}"
    );
    assert!(
        scroll.is_scrollable(),
        "more than six SSH aliases must make the suggestion list scrollable"
    );
    let scrollbar = cx
        .debug_bounds("remote-session-suggestions-scrollbar")
        .expect("the SSH suggestion list should render a scrollbar layer");
    assert!(
        scrollbar.size.height > px(0.),
        "the SSH suggestion scrollbar should have visible bounds: {scrollbar:?}"
    );
    assert!(
        cx.debug_bounds("remote-session-suggestion-5").is_some(),
        "the sixth alias should fit in the initial viewport"
    );
    assert!(
        cx.debug_bounds("remote-session-suggestion-6").is_none(),
        "the seventh alias should be virtualized outside the initial viewport"
    );

    scroll.scroll_to_item(11, ScrollStrategy::Bottom);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();

    let last_alias = cx
        .debug_bounds("remote-session-suggestion-11")
        .expect("the last alias should be reachable at the end of the list");
    assert!(
        last_alias.bottom() <= region.bottom(),
        "the last alias should be inside the suggestion viewport: {last_alias:?} vs {region:?}"
    );
    assert_eq!(scroll.is_scrolled_to_end(), Some(true));
}

#[gpui::test]
fn keyboard_suggestion_navigation_reveals_the_selected_alias(cx: &mut TestAppContext) {
    cx.update(|cx| theme::init(theme::LoadThemes::JustBase, cx));
    let aliases = (0..12)
        .map(|index| format!("host-{index}"))
        .collect::<Vec<_>>();
    let mut picker = RemoteSessionPicker {
        suggestions: aliases.clone(),
        ..Default::default()
    };
    for _ in 0..8 {
        assert!(picker.navigate_suggestions(false));
    }
    let selected = picker
        .suggestion_navigation
        .as_ref()
        .expect("keyboard navigation should select an alias")
        .selected;
    assert_eq!(selected, 7);

    let scroll = picker.suggestion_scroll.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| RemoteSessionSuggestionsHarness {
        suggestions: aliases,
        scroll,
        selected: Some(selected),
    });
    cx.simulate_resize(size(px(400.), px(300.)));
    cx.run_until_parked();

    let region = cx
        .debug_bounds("remote-session-suggestions-region")
        .expect("the SSH suggestion list should be laid out");
    let selected_alias = cx
        .debug_bounds("remote-session-suggestion-7")
        .expect("keyboard navigation should scroll the selected alias into view");
    assert!(
        selected_alias.origin.y >= region.origin.y && selected_alias.bottom() <= region.bottom(),
        "the keyboard-selected alias should be inside the suggestion viewport: {selected_alias:?} vs {region:?}"
    );
}
