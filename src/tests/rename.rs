use super::*;
use crate::process_control::{TabNameRequest, WorktreeNameRequest};
use std::collections::HashMap;

fn tab(attention_id: u64, custom_title: Option<&str>) -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = TerminalPane {
        id: 1,
        label_number: 1,
        generated_label: None,
        custom_label: None,
        overlay_text: None,
        overlay_font_size: None,
        overlay_opacity: None,
        overlay_color: None,
        profile,
        environment_overrides: HashMap::new(),
        terminal: None,
        view: None,
        error: None,
        base_exited: false,
        wsl_cwd_file: None,
        pending_command: None,
        detected_worktree_title: None,
        worktree_detection_directory: None,
        worktree_detection_generation: 0,
        stack: PaneStack::default(),
    };
    Tab {
        id: attention_id,
        attention_id,
        attention: None,
        panes: vec![pane],
        pane_indices: HashMap::from([(1, 0)]),
        next_pane_label: 2,
        layout: PaneLayout::Pane(1),
        active_pane: 1,
        focus_history: vec![1],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        custom_title: custom_title.map(str::to_owned),
        pinned_worktree_title: None,
        process_title: None,
        icon: Some(IconName::Terminal),
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    }
}

#[test]
fn tab_name_updates_the_exact_nested_name_without_touching_another_tab() {
    let mut tabs = [tab(1, Some("active")), tab(42, None)];
    let request = TabNameRequest {
        attention_id: 42,
        name: Some("feature/api".to_owned()),
    };

    assert!(set_tab_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(tabs[0].custom_title.as_deref(), Some("active"));
    assert_eq!(tabs[1].process_title.as_deref(), Some("feature/api"));
}

#[test]
fn tab_name_clear_removes_only_a_previous_process_title() {
    let mut tabs = [tab(42, Some("manually changed"))];
    tabs[0].process_title = Some("process title".to_owned());
    let request = TabNameRequest {
        attention_id: 42,
        name: None,
    };

    assert!(set_tab_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(tabs[0].custom_title.as_deref(), Some("manually changed"));
    assert_eq!(tabs[0].process_title, None);
}

#[test]
fn tab_name_does_not_fall_back_to_the_active_tab() {
    let mut tabs = [tab(1, Some("active"))];
    let request = TabNameRequest {
        attention_id: 99,
        name: Some("feature/api".to_owned()),
    };

    assert!(!set_tab_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(tabs[0].custom_title.as_deref(), Some("active"));
}

#[test]
fn tab_name_targets_detached_tabs_too() {
    let mut sessions = BackgroundSessionRunner::default();
    sessions.detach(tab(42, None), None);
    let request = TabNameRequest {
        attention_id: 42,
        name: Some("feature/api".to_owned()),
    };

    assert!(set_tab_name_on_tabs(sessions.iter_mut(), &request));
    assert_eq!(
        sessions.iter().next().unwrap().process_title.as_deref(),
        Some("feature/api")
    );
}

#[test]
fn worktree_name_request_sets_the_pinned_title() {
    let mut tabs = [tab(42, None)];
    let request = WorktreeNameRequest {
        attention_id: 42,
        name: Some("custom-tab-name".to_owned()),
    };

    assert!(set_worktree_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(
        tabs[0].pinned_worktree_title.as_deref(),
        Some("custom-tab-name")
    );
}

#[test]
fn pinned_worktree_title_wins_over_detected_and_process_titles() {
    let mut tab = tab(42, None);
    set_tab_worktree_title(&mut tab, Some("custom-tab-name".to_owned()));
    tab.panes[0].detected_worktree_title = Some("switched-source".to_owned());
    set_tab_process_title(&mut tab, Some("switched-source".to_owned()));

    let title = resolve_tab_title(&tab, || "terminal".to_owned().into());

    assert_eq!(title.as_ref(), "custom-tab-name");
}

#[test]
fn manual_rename_wins_over_worktree_and_process_titles() {
    let mut tab = tab(42, None);
    set_tab_worktree_title(&mut tab, Some("feature/api".to_owned()));
    set_tab_process_title(&mut tab, Some("switched-source".to_owned()));
    set_tab_title(&mut tab, Some("Pinned".to_owned()));

    assert_eq!(
        resolve_tab_title(&tab, || "terminal".to_owned().into()).as_ref(),
        "Pinned"
    );
}

#[test]
fn clearing_manual_rename_reveals_the_pinned_worktree_title() {
    let mut tab = tab(42, None);
    set_tab_worktree_title(&mut tab, Some("feature/api".to_owned()));
    set_tab_title(&mut tab, Some("Pinned".to_owned()));
    set_tab_title(&mut tab, None);

    assert_eq!(
        resolve_tab_title(&tab, || "terminal".to_owned().into()).as_ref(),
        "feature/api"
    );
}

#[test]
fn active_pane_selects_its_own_detected_worktree_title() {
    let mut tab = tab(42, None);
    tab.panes[0].detected_worktree_title = Some("feature/one".to_owned());
    let mut second = TerminalPane::new(2, tab.panes[0].profile.clone()).with_label_number(2);
    second.detected_worktree_title = Some("feature/two".to_owned());
    tab.push_pane(second);

    assert_eq!(
        resolve_tab_title(&tab, || "terminal".to_owned().into()).as_ref(),
        "feature/one"
    );
    tab.activate_pane(2);
    assert_eq!(
        resolve_tab_title(&tab, || "terminal".to_owned().into()).as_ref(),
        "feature/two"
    );
}

#[test]
fn clearing_worktree_title_invalidates_detection_and_preserves_manual_title() {
    let mut tabs = [tab(42, Some("Pinned"))];
    tabs[0].panes[0].detected_worktree_title = Some("stale".to_owned());
    tabs[0].panes[0].worktree_detection_directory = Some("/old-worktree".into());
    let initial_generation = tabs[0].panes[0].worktree_detection_generation;
    set_tab_worktree_title(&mut tabs[0], Some("feature/api".to_owned()));
    let request = WorktreeNameRequest {
        attention_id: 42,
        name: None,
    };

    assert!(set_worktree_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(tabs[0].pinned_worktree_title, None);
    assert_eq!(tabs[0].custom_title.as_deref(), Some("Pinned"));
    assert_eq!(tabs[0].panes[0].detected_worktree_title, None);
    assert_eq!(tabs[0].panes[0].worktree_detection_directory, None);
    assert_eq!(
        tabs[0].panes[0].worktree_detection_generation,
        initial_generation.wrapping_add(2)
    );
}

#[test]
fn clearing_process_title_does_not_affect_worktree_title() {
    let mut tabs = [tab(42, None)];
    set_tab_worktree_title(&mut tabs[0], Some("feature/api".to_owned()));
    set_tab_process_title(&mut tabs[0], Some("switched-source".to_owned()));
    let request = TabNameRequest {
        attention_id: 42,
        name: None,
    };

    assert!(set_tab_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(tabs[0].process_title, None);
    assert_eq!(
        tabs[0].pinned_worktree_title.as_deref(),
        Some("feature/api")
    );
}
