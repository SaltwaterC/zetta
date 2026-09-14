use super::*;
use gpui::{
    Context, FocusHandle, KeyDownEvent, Keystroke, Modifiers, TestAppContext,
    UniformListScrollHandle, px, red, size,
};
use std::{cell::Cell, rc::Rc};

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

fn picker_with_suggestions(target: &str) -> RemoteSessionPicker {
    RemoteSessionPicker {
        target: TextField::new(target),
        suggestions: vec![
            "production".to_owned(),
            "prod-west".to_owned(),
            "prod-staging".to_owned(),
            "staging".to_owned(),
        ],
        ..Default::default()
    }
}

fn remote_session_summary(
    id: u64,
    authentication_required: bool,
) -> zmux::protocol::BackgroundSessionSummary {
    zmux::protocol::BackgroundSessionSummary {
        id,
        title: format!("Session {id}"),
        authentication_required,
        active_pane: 1,
        layout: zmux::protocol::BackgroundPaneLayout::Pane { pane_id: 1 },
        panes: Vec::new(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn remote_key_event(key: &str, modifiers: Modifiers) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

#[test]
fn remote_picker_starts_on_the_target_field() {
    let picker = RemoteSessionPicker::default();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert!(picker.target.text.is_empty());
    assert!(picker.port.text.is_empty());
    assert!(picker.sessions.is_empty());
    assert!(!picker.loading);
    assert!(picker.suggestion_navigation.is_none());
}

#[test]
fn single_pane_is_first_and_selected_by_default() {
    let picker = RemoteSessionPicker::default();

    assert_eq!(picker.selected_template, 0);
    assert_eq!(picker.templates, vec![RemoteSessionTemplate::SinglePane]);
    assert_eq!(picker.templates[0].label(), "Single pane");
}

#[test]
fn single_pane_precedes_sorted_configured_templates() {
    let mut config = Config::defaults(None, None);
    config.pane_split_templates.clear();
    config.pane_split_templates.insert(
        "zulu".to_owned(),
        PaneSplitTemplateConfig {
            layout: PaneSplitTemplate::Pane(Box::default()),
            env: HashMap::new(),
        },
    );
    config.pane_split_templates.insert(
        "Alpha".to_owned(),
        PaneSplitTemplateConfig {
            layout: PaneSplitTemplate::Pane(Box::default()),
            env: HashMap::new(),
        },
    );

    assert_eq!(
        remote_session_templates(&config),
        vec![
            RemoteSessionTemplate::SinglePane,
            RemoteSessionTemplate::Configured("Alpha".to_owned()),
            RemoteSessionTemplate::Configured("zulu".to_owned()),
        ]
    );
}

#[test]
fn opening_a_remote_dropdown_does_not_change_its_selection() {
    let mut picker = RemoteSessionPicker {
        templates: vec![
            RemoteSessionTemplate::SinglePane,
            RemoteSessionTemplate::Configured("one".to_owned()),
            RemoteSessionTemplate::Configured("two".to_owned()),
        ],
        ..Default::default()
    };

    assert!(picker.open_dropdown(RemoteSessionDropdown::Template, Point::default()));

    assert_eq!(picker.selected_template, 0);
    assert_eq!(picker.dropdown.selected_index, 0);
}

#[test]
fn searching_and_committing_a_remote_template_updates_the_selection() {
    let mut picker = RemoteSessionPicker {
        templates: vec![
            RemoteSessionTemplate::SinglePane,
            RemoteSessionTemplate::Configured("one-right".to_owned()),
            RemoteSessionTemplate::Configured("two-right".to_owned()),
        ],
        ..Default::default()
    };

    picker.open_dropdown(RemoteSessionDropdown::Template, Point::default());
    picker.dropdown.set_query("two");

    assert!(picker.commit_dropdown("two-right".to_owned()));
    assert_eq!(picker.selected_template, 2);
    assert_eq!(picker.open_dropdown, None);
}

#[test]
fn searching_and_committing_a_remote_profile_updates_the_selection() {
    let mut picker = RemoteSessionPicker {
        profiles: vec!["System".to_owned(), "Remote shell".to_owned()],
        ..Default::default()
    };

    picker.open_dropdown(RemoteSessionDropdown::Profile, Point::default());
    picker.dropdown.set_query("shell");

    assert!(picker.commit_dropdown("Remote shell".to_owned()));
    assert_eq!(picker.selected_profile, 1);
    assert_eq!(picker.open_dropdown, None);
}

#[test]
fn a_no_match_remote_query_cannot_commit() {
    let mut picker = RemoteSessionPicker {
        templates: vec![
            RemoteSessionTemplate::SinglePane,
            RemoteSessionTemplate::Configured("one".to_owned()),
        ],
        ..Default::default()
    };

    picker.open_dropdown(RemoteSessionDropdown::Template, Point::default());
    picker.dropdown.set_query("missing");

    assert!(!picker.commit_dropdown("one".to_owned()));
    assert_eq!(picker.selected_template, 0);
    assert_eq!(picker.open_dropdown, Some(RemoteSessionDropdown::Template));
}

#[test]
fn remote_dropdown_escape_closes_without_dismissing_the_picker() {
    let mut picker = RemoteSessionPicker {
        profiles: vec!["System".to_owned()],
        ..Default::default()
    };

    picker.open_dropdown(RemoteSessionDropdown::Profile, Point::default());

    assert!(picker.close_dropdown());
    assert_eq!(picker.open_dropdown, None);
    assert_eq!(picker.field, RemoteSessionField::Profile);
}

#[test]
fn remote_profile_dropdown_cannot_open_while_loading_or_without_options() {
    let mut picker = RemoteSessionPicker {
        profiles_loading: true,
        profiles: vec!["System".to_owned()],
        ..Default::default()
    };
    assert!(!picker.open_dropdown(RemoteSessionDropdown::Profile, Point::default()));

    picker.profiles_loading = false;
    picker.profiles.clear();
    assert!(!picker.open_dropdown(RemoteSessionDropdown::Profile, Point::default()));
}

#[test]
fn remote_shell_startup_failures_are_short_and_readable() {
    let error = anyhow::anyhow!(
        "SSH endpoint query failed with exit status: 127: (anon):setopt:7: can't change option: monitor\n\n\x1b[31mERROR\x1b[39m: gitstatus failed to initialize.\n\n\x1b[32mexec zsh\x1b[39m\nzsh:1: command not found: zmux"
    );

    let message = remote_error_message(&error);

    assert_eq!(
        message,
        "Remote zmux could not be started (exit status 127). Make sure it is installed and available in the remote login shell's PATH."
    );
    assert!(!message.contains('\x1b'));
    assert!(!message.contains("gitstatus"));
}

#[test]
fn remote_errors_strip_terminal_sequences_and_are_bounded() {
    let error = anyhow::anyhow!(
        "\x1b]0;remote\x07\x1b[31mremote operation failed\x1b[39m\nsecond detail\n{}",
        "long detail ".repeat(40)
    );

    let message = remote_error_message(&error);

    assert!(message.starts_with("remote operation failed"));
    assert!(message.contains("second detail"));
    assert!(!message.contains('\x1b'));
    assert!(message.chars().count() <= REMOTE_ERROR_MAX_CHARS);
    assert!(message.ends_with('…'));
}

#[test]
fn create_is_available_only_after_remote_profiles_and_templates_load() {
    let mut picker = RemoteSessionPicker {
        field: RemoteSessionField::Create,
        profiles: vec!["System".to_owned()],
        templates: vec![RemoteSessionTemplate::Configured("split".to_owned())],
        ..Default::default()
    };

    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Create);

    picker.profiles_loading = true;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Ignore);

    picker.profiles_loading = false;
    picker.creating = true;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Ignore);
}

#[test]
fn create_is_available_with_remote_profiles_and_no_configured_templates() {
    let picker = RemoteSessionPicker {
        field: RemoteSessionField::Create,
        profiles: vec!["Remote shell".to_owned()],
        templates: vec![RemoteSessionTemplate::SinglePane],
        ..Default::default()
    };

    assert!(picker.can_create());
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Create);
}

#[test]
fn single_pane_creation_uses_the_selected_remote_profile() {
    let config = Config::defaults(None, None);
    let spec = build_remote_create_spec(
        &config,
        &RemoteSessionTemplate::SinglePane,
        "Remote shell",
        &["Remote shell".to_owned()],
    )
    .expect("single-pane creation should build a remote spec");

    assert_eq!(spec.panes.len(), 1);
    assert_eq!(
        spec.layout,
        zmux::messages::SharedDraftLayout::Draft { draft_id: 1 }
    );
    assert_eq!(
        spec.active_pane,
        zmux::messages::SharedPaneRef::Draft { draft_id: 1 }
    );
    let pane = &spec.panes[0];
    assert_eq!(pane.draft_id, 1);
    assert_eq!(pane.profile, "Remote shell");
    assert!(pane.command.is_none());
    assert!(pane.env.is_empty());
    assert!(pane.load_shell_integration);
    assert_eq!(pane.size.columns, zmux::headless::DEFAULT_COLUMNS);
    assert_eq!(pane.size.lines, zmux::headless::DEFAULT_LINES);
    assert_eq!(pane.metadata.label, "pane-1");
    assert_eq!(pane.metadata.profile, "Remote shell");
    assert_eq!(pane.metadata.application, "Remote shell");
}

#[test]
fn configured_split_template_creation_still_builds_every_pane() {
    let config = Config::defaults(None, None);
    let spec = build_remote_create_spec(
        &config,
        &RemoteSessionTemplate::Configured("three-right".to_owned()),
        "System",
        &["System".to_owned()],
    )
    .expect("configured split creation should build a remote spec");

    assert_eq!(spec.panes.len(), 3);
    assert!(matches!(
        spec.layout,
        zmux::messages::SharedDraftLayout::Split { .. }
    ));
}

#[test]
fn invalidating_picker_results_clears_a_pending_attach() {
    let mut picker = RemoteSessionPicker {
        attach_generation: Some(9),
        loading: true,
        ..Default::default()
    };

    picker.invalidate_results();

    assert_eq!(picker.attach_generation, None);
    assert!(!picker.loading);
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

#[test]
fn enter_attaches_the_selected_session_from_any_field() {
    let mut picker = RemoteSessionPicker {
        target: TextField::new("dev.example"),
        sessions: vec![
            remote_session_summary(4, false),
            remote_session_summary(9, false),
        ],
        selected: 1,
        ..Default::default()
    };

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Attach(1));

    picker.field = RemoteSessionField::List;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Attach(1));
}

#[test]
fn enter_loads_only_while_the_session_list_is_empty() {
    let mut picker = RemoteSessionPicker {
        target: TextField::new("dev.example"),
        ..Default::default()
    };

    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Load);

    picker.loading = true;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Ignore);

    picker.sessions = vec![remote_session_summary(4, false)];
    assert_eq!(
        picker.enter_action(),
        RemoteSessionEnterAction::Ignore,
        "an attach in flight must not be started twice"
    );
}

#[test]
fn enter_clamps_a_selection_past_the_end_of_the_list() {
    let picker = RemoteSessionPicker {
        sessions: vec![
            remote_session_summary(4, false),
            remote_session_summary(9, false),
        ],
        selected: 7,
        ..Default::default()
    };

    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Attach(1));
}

#[test]
fn focused_actions_map_enter_to_their_own_actions() {
    let mut picker = RemoteSessionPicker {
        profiles: vec!["System".to_owned()],
        sessions: vec![remote_session_summary(4, false)],
        ..Default::default()
    };

    picker.field = RemoteSessionField::Cancel;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Cancel);
    picker.field = RemoteSessionField::Load;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Load);
    picker.field = RemoteSessionField::Create;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Create);
    picker.field = RemoteSessionField::Attach;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Attach(0));

    picker.loading = true;
    picker.field = RemoteSessionField::Cancel;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Cancel);
    picker.field = RemoteSessionField::Load;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Ignore);
    picker.field = RemoteSessionField::Create;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Ignore);
    picker.field = RemoteSessionField::Attach;
    assert_eq!(picker.enter_action(), RemoteSessionEnterAction::Ignore);
}

#[test]
fn primary_and_alt_enter_map_to_create_and_attach_only_when_available() {
    let primary = Modifiers::secondary_key();
    let mut picker = RemoteSessionPicker {
        profiles: vec!["System".to_owned()],
        sessions: vec![
            remote_session_summary(4, false),
            remote_session_summary(9, false),
        ],
        selected: 1,
        ..Default::default()
    };

    assert_eq!(
        picker.shortcut_action(&remote_key_event("enter", primary)),
        Some(RemoteSessionEnterAction::Create)
    );
    assert_eq!(
        picker.shortcut_action(&remote_key_event("enter", Modifiers::alt())),
        Some(RemoteSessionEnterAction::Attach(1))
    );
    assert_eq!(
        picker.shortcut_action(&remote_key_event("enter", Modifiers::default())),
        None
    );

    picker.profiles.clear();
    assert_eq!(
        picker.shortcut_action(&remote_key_event("enter", primary)),
        Some(RemoteSessionEnterAction::Ignore)
    );
    picker.sessions.clear();
    assert_eq!(
        picker.shortcut_action(&remote_key_event("enter", Modifiers::alt())),
        Some(RemoteSessionEnterAction::Ignore)
    );
    assert_eq!(
        picker.shortcut_action(&remote_key_event(
            "enter",
            Modifiers {
                alt: true,
                shift: true,
                ..Default::default()
            },
        )),
        None
    );
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

#[test]
fn down_selects_the_first_host_without_leaving_the_target_field() {
    let mut picker = picker_with_suggestions("");

    assert!(picker.navigate_suggestions(false));
    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(picker.target.text, "production");
    assert_eq!(picker.suggestion_navigation.as_ref().unwrap().selected, 0);
    assert!(!picker.visible_suggestions().is_empty());
}

#[test]
fn suggestion_navigation_wraps_through_matching_hosts() {
    let mut picker = picker_with_suggestions("prod");

    picker.navigate_suggestions(false);
    assert_eq!(picker.target.text, "production");
    picker.navigate_suggestions(false);
    assert_eq!(picker.target.text, "prod-west");
    picker.navigate_suggestions(true);
    assert_eq!(picker.target.text, "production");
    picker.navigate_suggestions(true);
    assert_eq!(picker.target.text, "prod-staging");
}

#[test]
fn suggestion_navigation_preserves_the_original_filter() {
    let mut picker = picker_with_suggestions("prod");

    picker.navigate_suggestions(false);

    assert_eq!(picker.target.text, "production");
    assert_eq!(
        picker.suggestion_navigation.as_ref().unwrap().filter,
        "prod"
    );
    assert_eq!(
        picker.visible_suggestions(),
        vec!["production", "prod-west", "prod-staging"]
    );
}

#[test]
fn editing_the_target_resets_suggestion_navigation() {
    let mut picker = picker_with_suggestions("prod");
    picker.navigate_suggestions(false);
    picker
        .suggestion_scroll
        .scroll_to_item(3, ScrollStrategy::Top);

    picker.target = TextField::new("productionx");
    picker.reset_suggestion_navigation();

    assert!(picker.suggestion_navigation.is_none());
    assert!(picker.visible_suggestions().is_empty());
    assert_eq!(picker.suggestion_scroll.logical_scroll_top_index(), 0);
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

#[test]
fn target_navigation_keeps_the_current_value_for_enter_to_load() {
    let mut picker = picker_with_suggestions("prod");
    picker.navigate_suggestions(false);

    let target = Zetta::remote_target_from_picker(&picker).unwrap();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(target.destination(), "production");
}

#[test]
fn remote_picker_parses_optional_ports_and_rejects_invalid_values() {
    let mut picker = RemoteSessionPicker {
        target: TextField::new("dev.example"),
        ..Default::default()
    };

    let target = Zetta::remote_target_from_picker(&picker).unwrap();
    assert_eq!(target.destination(), "dev.example");
    assert_eq!(target.port(), None);

    picker.port = TextField::new("2200");
    assert_eq!(
        Zetta::remote_target_from_picker(&picker).unwrap().port(),
        Some(2200)
    );

    picker.port = TextField::new("0");
    assert!(Zetta::remote_target_from_picker(&picker).is_err());
    picker.port = TextField::new("not-a-port");
    assert!(Zetta::remote_target_from_picker(&picker).is_err());
}

/// The keep-alive field only exists while Zosh is carrying the panes, so the
/// tab order has to change with the protocol rather than land on a field the
/// picker is not showing.
#[test]
fn the_keep_alive_field_is_only_in_the_tab_order_under_zosh() {
    let mut picker = RemoteSessionPicker::default();

    let ssh_order = collect_tab_order(&mut picker);
    assert_eq!(
        ssh_order,
        vec![
            RemoteSessionField::Port,
            RemoteSessionField::Protocol,
            RemoteSessionField::Profile,
            RemoteSessionField::Template,
            RemoteSessionField::List,
            RemoteSessionField::Cancel,
            RemoteSessionField::Load,
            RemoteSessionField::Target,
        ]
    );

    picker.field = RemoteSessionField::Protocol;
    picker.toggle_transport();
    assert!(picker.transport.is_zosh());
    picker.field = RemoteSessionField::Target;
    assert_eq!(
        collect_tab_order(&mut picker),
        vec![
            RemoteSessionField::Port,
            RemoteSessionField::Protocol,
            RemoteSessionField::KeepAlive,
            RemoteSessionField::Profile,
            RemoteSessionField::Template,
            RemoteSessionField::List,
            RemoteSessionField::Cancel,
            RemoteSessionField::Load,
            RemoteSessionField::Target,
        ]
    );
}

#[test]
fn action_tab_order_is_dynamic_and_reverse_wraps() {
    let mut picker = RemoteSessionPicker {
        profiles: vec!["System".to_owned()],
        sessions: vec![remote_session_summary(4, false)],
        ..Default::default()
    };

    assert_eq!(
        collect_tab_order(&mut picker),
        vec![
            RemoteSessionField::Port,
            RemoteSessionField::Protocol,
            RemoteSessionField::Profile,
            RemoteSessionField::Template,
            RemoteSessionField::List,
            RemoteSessionField::Cancel,
            RemoteSessionField::Load,
            RemoteSessionField::Create,
            RemoteSessionField::Attach,
            RemoteSessionField::Target,
        ]
    );

    picker.field = RemoteSessionField::Target;
    picker.cycle_field(true);
    assert_eq!(picker.field, RemoteSessionField::Attach);
    picker.cycle_field(true);
    assert_eq!(picker.field, RemoteSessionField::Create);

    picker.loading = true;
    picker.field = RemoteSessionField::Load;
    picker.move_unavailable_focus_to_cancel();
    assert_eq!(picker.field, RemoteSessionField::Cancel);
    assert!(!picker.field_order().contains(&RemoteSessionField::Load));
    assert!(!picker.field_order().contains(&RemoteSessionField::Create));
    assert!(!picker.field_order().contains(&RemoteSessionField::Attach));

    picker.loading = false;
    picker.sessions.clear();
    picker.field = RemoteSessionField::Attach;
    picker.move_unavailable_focus_to_cancel();
    assert_eq!(picker.field, RemoteSessionField::Cancel);
}

/// Turning Zosh back off while the keep-alive field has the focus would leave
/// the focus on a field nothing draws.
#[test]
fn leaving_zosh_moves_the_focus_off_the_keep_alive_field() {
    let mut picker = RemoteSessionPicker::default();
    picker.toggle_transport();
    picker.field = RemoteSessionField::KeepAlive;

    picker.toggle_transport();

    assert!(!picker.transport.is_zosh());
    assert_eq!(picker.field, RemoteSessionField::Protocol);
}

/// Choosing Zosh with an empty field asks for the default interval, the way
/// `-k` with no value does; emptying the field afterwards is how a session is
/// left on Mosh's own heartbeat.
#[test]
fn the_protocol_decides_what_the_picker_asks_for() {
    let mut picker = RemoteSessionPicker {
        target: TextField::new("dev.example"),
        ..Default::default()
    };
    assert_eq!(
        Zetta::remote_transport_from_picker(&picker).unwrap(),
        RemotePaneTransport::Ssh
    );

    picker.toggle_transport();
    assert_eq!(
        Zetta::remote_transport_from_picker(&picker).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: Some(REMOTE_KEEP_ALIVE_DEFAULT_MS)
        }
    );

    picker.keep_alive = TextField::default();
    assert_eq!(
        Zetta::remote_transport_from_picker(&picker).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: None
        },
        "an empty interval leaves the link on Mosh's own heartbeat"
    );

    picker.keep_alive = TextField::new("5");
    assert!(
        Zetta::remote_transport_from_picker(&picker).is_err(),
        "an interval below Mosh's frame interval cannot be held to"
    );

    // An unusable interval belongs to Zosh alone: switching back to SSH must
    // not keep reporting it.
    picker.toggle_transport();
    assert_eq!(
        Zetta::remote_transport_from_picker(&picker).unwrap(),
        RemotePaneTransport::Ssh
    );
}

/// Walks the tab order from wherever the picker is, once round.
fn collect_tab_order(picker: &mut RemoteSessionPicker) -> Vec<RemoteSessionField> {
    let started = picker.field;
    let expected_len = picker.field_order().len();
    let mut visited = Vec::new();
    loop {
        picker.cycle_field(false);
        visited.push(picker.field);
        if picker.field == started || visited.len() > expected_len {
            return visited;
        }
    }
}
