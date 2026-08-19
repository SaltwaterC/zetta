use super::*;
use crate::{OverlayFontSize, PaneStackSelection, SplitPosition};

fn profile(name: &str) -> Profile {
    Profile {
        name: name.to_owned(),
        command: task::Shell::System,
        theme: None,
        icon: crate::ProfileIcon::default(),
    }
}

/// A tab with two split panes and most of its durable state set to something
/// other than the default, so a field that fails to round-trip shows up.
fn populated_tab() -> Tab {
    let mut tab = Tab {
        id: 1,
        attention_id: 42,
        attention: None,
        panes: vec![
            TerminalPane::new(10, profile("System")).with_label_number(1),
            TerminalPane::new(11, profile("Fish")).with_label_number(2),
        ],
        pane_indices: HashMap::from([(10, 0), (11, 1)]),
        next_pane_label: 3,
        layout: PaneLayout::Pane(10),
        active_pane: 11,
        focus_history: vec![10, 11],
        maximized_pane: None,
        minimized_panes: vec![10],
        selected_minimized_pane: Some(10),
        broadcast_input: true,
        silent_mode: true,
        close_policy: TabClosePolicy::Background {
            authentication: None,
        },
        shared: true,
        custom_title: Some("release build".to_owned()),
        worktree_seed_title: Some("feature".to_owned()),
        process_title: Some("cargo".to_owned()),
        icon: crate::tab_icon_picker::parse_tab_icon_name("Terminal"),
        pinned: true,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    };
    tab.layout
        .split(10, SplitAxis::Vertical, 11, SplitPosition::After);

    let pane = tab.pane_mut(10).unwrap();
    pane.custom_label = Some("compiler".to_owned());
    pane.generated_label = Some("HTTP: 8080".to_owned());
    pane.environment_overrides = HashMap::from([("RUST_LOG".to_owned(), "debug".to_owned())]);
    pane.overlay_text = Some("staging".to_owned());
    pane.overlay_font_size = Some(OverlayFontSize::Large);
    pane.overlay_opacity = Some(0.5);
    pane.overlay_color = Some(gpui::Hsla {
        h: 0.1,
        s: 0.2,
        l: 0.3,
        a: 0.4,
    });
    pane.pending_command = Some("cargo test".to_owned());
    pane.detected_worktree_title = Some("worktree".to_owned());
    tab
}

fn round_trip(tab: &Tab) -> Tab {
    let state = TabState::from_tab(tab, &HashMap::from([(10, 900), (11, 901)]));
    // Through JSON, because that is how it reaches the multiplexer: a field
    // that serializes but does not deserialize would otherwise pass.
    let encoded = serde_json::to_string(&state).unwrap();
    let decoded: TabState = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, state);
    decoded.into_tab(tab.id, profile).unwrap()
}

#[test]
fn a_tab_survives_the_round_trip_to_the_multiplexer_and_back() {
    let original = populated_tab();
    let restored = round_trip(&original);

    assert_eq!(restored.id, original.id);
    assert_eq!(restored.attention_id, original.attention_id);
    assert_eq!(restored.layout, original.layout);
    assert_eq!(restored.active_pane, original.active_pane);
    assert_eq!(restored.focus_history, original.focus_history);
    assert_eq!(restored.minimized_panes, original.minimized_panes);
    assert_eq!(
        restored.selected_minimized_pane,
        original.selected_minimized_pane
    );
    assert_eq!(restored.broadcast_input, original.broadcast_input);
    assert_eq!(restored.silent_mode, original.silent_mode);
    assert_eq!(restored.custom_title, original.custom_title);
    assert_eq!(restored.worktree_seed_title, original.worktree_seed_title);
    assert_eq!(restored.process_title, original.process_title);
    assert_eq!(restored.icon, original.icon);
    assert!(restored.icon.is_some(), "the icon must not be lost");
    assert_eq!(restored.pinned, original.pinned);
    assert_eq!(restored.next_pane_label, original.next_pane_label);
    assert!(matches!(
        restored.close_policy,
        TabClosePolicy::Background { .. }
    ));
    // Without this a window joining a shared session would show sharing as
    // switched off, and switching it on again would be the only way to make the
    // menu agree with what the multiplexer is already doing.
    assert_eq!(restored.shared, original.shared);
    assert!(restored.shared, "the sharing must not be lost");
    assert_eq!(restored.pane_indices, original.pane_indices);
}

#[test]
fn a_panes_appearance_and_configuration_survive() {
    let original = populated_tab();
    let restored = round_trip(&original);

    let before = original.pane(10).unwrap();
    let after = restored.pane(10).unwrap();
    assert_eq!(after.custom_label, before.custom_label);
    assert_eq!(after.generated_label, before.generated_label);
    assert_eq!(after.label_number, before.label_number);
    assert_eq!(after.environment_overrides, before.environment_overrides);
    assert_eq!(after.overlay_text, before.overlay_text);
    assert_eq!(after.overlay_font_size, before.overlay_font_size);
    assert_eq!(after.overlay_opacity, before.overlay_opacity);
    assert_eq!(after.overlay_color, before.overlay_color);
    assert_eq!(after.pending_command, before.pending_command);
    assert_eq!(
        after.detected_worktree_title,
        before.detected_worktree_title
    );
    assert_eq!(after.profile.name, before.profile.name);
    assert_eq!(restored.pane(11).unwrap().profile.name, "Fish");
}

#[test]
fn a_profile_is_resolved_afresh_rather_than_frozen() {
    // A profile is user configuration. A session restored after the user
    // edited one should follow the edit.
    let state = TabState::from_tab(&populated_tab(), &HashMap::new());
    let restored = state
        .into_tab(1, |name| Profile {
            theme: Some("Edited Theme".to_owned()),
            ..profile(name)
        })
        .unwrap();

    assert_eq!(
        restored.pane(10).unwrap().profile.theme.as_deref(),
        Some("Edited Theme")
    );
}

#[test]
fn the_multiplexers_pane_identifiers_are_carried_per_pane() {
    // These are how the two sides agree on which terminal to hand over.
    let state = TabState::from_tab(&populated_tab(), &HashMap::from([(11, 901)]));

    assert_eq!(state.panes[0].mux_pane_id, None);
    assert_eq!(state.panes[1].mux_pane_id, Some(901));
}

#[test]
fn transient_editing_state_is_not_restored() {
    let mut original = populated_tab();
    original.renaming_pane = Some(10);
    original.rename_buffer = Some("half typed".to_owned());
    original.editing_overlay_pane = Some(11);
    original.overlay_buffer = Some("unfinished".to_owned());

    let restored = round_trip(&original);

    // Restoring a session into the middle of an interaction the user has since
    // forgotten about is worse than restoring it settled.
    assert_eq!(restored.renaming_pane, None);
    assert_eq!(restored.rename_buffer, None);
    assert_eq!(restored.editing_overlay_pane, None);
    assert_eq!(restored.overlay_buffer, None);
}

#[test]
fn a_layout_that_does_not_match_its_panes_is_refused() {
    let mut state = TabState::from_tab(&populated_tab(), &HashMap::new());
    state.panes.retain(|pane| pane.id != 11);

    // Silently dropping the difference would lose a terminal from the user's
    // view while leaving it running in the multiplexer.
    let error = match state.clone().into_tab(1, profile) {
        Ok(_) => panic!("a layout naming a missing pane must be refused"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("pane 11"), "{error}");

    let mut extra = TabState::from_tab(&populated_tab(), &HashMap::new());
    extra.panes.push(PaneState {
        id: 12,
        mux_pane_id: None,
        label_number: 3,
        generated_label: None,
        custom_label: None,
        profile: "System".to_owned(),
        environment_overrides: HashMap::new(),
        overlay: None,
        exit: None,
        base_exited: false,
        pending_command: None,
        detected_worktree_title: None,
        stack: Vec::new(),
        selected_stacked: None,
    });
    assert!(extra.into_tab(1, profile).is_err());
}

#[test]
fn an_active_pane_outside_the_layout_is_refused() {
    let mut state = TabState::from_tab(&populated_tab(), &HashMap::new());
    state.active_pane = 99;

    assert!(state.into_tab(1, profile).is_err());
}

#[test]
fn a_restored_pane_starts_without_a_terminal() {
    // The terminal arrives from the multiplexer afterwards; a restored pane
    // that claimed to have one would be rendered before it exists.
    let restored = round_trip(&populated_tab());
    let pane = restored.pane(10).unwrap();

    assert!(pane.terminal.is_none());
    assert!(pane.view.is_none());
    assert!(pane.stack.is_empty());
    assert_eq!(pane.stack.selected, PaneStackSelection::Base);
}

#[test]
fn a_panes_stacked_commands_survive_a_detach() {
    // The stack used to be dropped outright while `base_exited` was kept, so a
    // pane detached with its shell gone and a command in front came back with
    // nothing to show and no way to get anything.
    let mut tab = populated_tab();
    let pane = tab.pane_mut(10).unwrap();
    pane.base_exited = true;
    let mut finished = StackedPane::new(
        70,
        "cargo build".to_owned(),
        profile("System"),
        Some(std::path::PathBuf::from("/tmp")),
        None,
    );
    finished.state = crate::pane::StackedPaneState::Completed;
    finished.exit_code = Some(0);
    pane.stack.entries.push(finished);
    pane.stack.selected = PaneStackSelection::Stacked(70);

    let restored = round_trip(&tab);
    let pane = restored.pane(10).unwrap();

    assert_eq!(pane.stack.entries.len(), 1, "the stacked command was lost");
    let entry = &pane.stack.entries[0];
    assert_eq!(entry.id, 70);
    assert_eq!(entry.command, "cargo build");
    assert_eq!(entry.state, crate::pane::StackedPaneState::Completed);
    assert_eq!(entry.exit_code, Some(0));
    assert_eq!(
        entry.working_directory,
        Some(std::path::PathBuf::from("/tmp"))
    );
    // Which command was in front is part of the pane's shape, and with
    // `base_exited` set there is no base terminal to fall back to.
    assert_eq!(pane.stack.selected, PaneStackSelection::Stacked(70));
    assert!(pane.base_exited);
}

#[test]
fn a_command_still_running_at_detach_comes_back_saying_so() {
    let mut tab = populated_tab();
    let pane = tab.pane_mut(10).unwrap();
    let mut running = StackedPane::new(71, "sleep 60".to_owned(), profile("System"), None, None);
    running.state = crate::pane::StackedPaneState::Running;
    pane.stack.entries.push(running);

    let restored = round_trip(&tab);
    let entry = &restored.pane(10).unwrap().stack.entries[0];

    // A stacked command's terminal is a task terminal, which cannot be
    // reattached, so restoring it as "running" would leave a command that never
    // finishes. Saying it could not be restored is the honest answer.
    assert_eq!(entry.state, crate::pane::StackedPaneState::Failed);
    assert!(
        entry
            .error
            .as_deref()
            .is_some_and(|error| error.contains("still running")),
        "{:?}",
        entry.error
    );
}

#[test]
fn a_stack_selection_naming_a_missing_command_falls_back_to_the_shell() {
    let mut state = TabState::from_tab(&populated_tab(), &HashMap::new());
    state
        .panes
        .iter_mut()
        .find(|pane| pane.id == 10)
        .unwrap()
        .selected_stacked = Some(999);

    let restored = state.into_tab(1, profile).unwrap();

    // Leaving the selection would render a pane that has nothing selected.
    assert_eq!(
        restored.pane(10).unwrap().stack.selected,
        PaneStackSelection::Base
    );
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    let mut encoded: serde_json::Value =
        serde_json::to_value(TabState::from_tab(&populated_tab(), &HashMap::new())).unwrap();
    encoded["unexpected"] = serde_json::json!(true);

    // State written by a newer Zetta may mean something this one would get
    // wrong; refusing is better than restoring a tab that is subtly not the
    // one that was detached.
    assert!(serde_json::from_value::<TabState>(encoded).is_err());
}
