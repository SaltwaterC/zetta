use super::*;

use gpui::{
    Context, FocusHandle, KeyDownEvent, Keystroke, Modifiers, TestAppContext,
    UniformListScrollHandle, px, red, size,
};

use std::{cell::Cell, rc::Rc};

#[path = "remote_session_ui/windowed.rs"]
mod windowed;

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
        "Remote zmux was not found. Install it on the remote host or add it to the remote login shell's PATH."
    );
    assert!(!message.contains('\x1b'));
    assert!(!message.contains("gitstatus"));
}

#[test]
fn missing_remote_daemon_is_reported_instead_of_shell_startup_noise() {
    let error = anyhow::anyhow!(
        "listing remote sessions: SSH endpoint query failed with exit status: 1: \
         (anon):setopt:7: can't change option: monitor\n\n\
         \x1b[31mERROR\x1b[39m: gitstatus failed to initialize.\n\n\
         Add GITSTATUS_LOG_LEVEL=DEBUG to ~/.zshrc for extra diagnostics.\n\n\
         zmux: reading multiplexer endpoint \
         /home/user/.config/zetta/sessions/zmux.json: \
         No such file or directory (os error 2)"
    );

    let message = remote_error_message(&error);

    assert_eq!(
        message,
        "No remote session service is running on this host. Create a remote session to start one."
    );
    assert!(!message.contains("gitstatus"));
    assert!(!message.contains("setopt"));
}

#[test]
fn stale_remote_endpoint_is_reported_as_a_missing_daemon() {
    let error = anyhow::anyhow!(
        "listing remote sessions: checking the SSH stdio proxy for pi: \
         SSH stderr: zmux: connecting the SSH stdio proxy to the local daemon: \
         Connection refused (os error 111)"
    );
    assert_eq!(
        remote_error_message(&error),
        "No remote session service is running on this host. Create a remote session to start one."
    );
}

#[test]
fn remote_zmux_diagnostic_takes_precedence_over_shell_startup_noise() {
    let error = anyhow::anyhow!(
        "SSH endpoint query failed with exit status: 1: \
         shell prompt failed to initialize\n\
         zmux: endpoint belongs to an unsupported version"
    );

    assert_eq!(
        remote_error_message(&error),
        "zmux: endpoint belongs to an unsupported version"
    );
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

    picker.forward_agent = true;
    assert_eq!(
        Zetta::remote_target_from_picker(&picker).unwrap(),
        zmux::remote::RemoteTarget::new("dev.example").with_forward_agent(true)
    );
    picker.toggle_transport();
    assert_eq!(
        Zetta::remote_target_from_picker(&picker).unwrap(),
        zmux::remote::RemoteTarget::new("dev.example").with_forward_agent(false),
        "Zosh uses its pane bridge instead of native OpenSSH forwarding"
    );
    picker.toggle_transport();

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
            RemoteSessionField::ForwardAgent,
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
            RemoteSessionField::ForwardAgent,
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
            RemoteSessionField::ForwardAgent,
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
            keep_alive_ms: Some(REMOTE_KEEP_ALIVE_DEFAULT_MS),
            forward_agent: false,
        }
    );

    picker.keep_alive = TextField::default();
    assert_eq!(
        Zetta::remote_transport_from_picker(&picker).unwrap(),
        RemotePaneTransport::Zosh {
            keep_alive_ms: None,
            forward_agent: false,
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
