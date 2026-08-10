use super::*;
use crate::process_control::TabNameRequest;
use std::collections::HashMap;

fn tab(attention_id: u64, custom_title: Option<&str>) -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
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
        terminal: None,
        view: None,
        error: None,
        wsl_cwd_file: None,
        pending_command: None,
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
        close_policy: TabClosePolicy::Close,
        custom_title: custom_title.map(str::to_owned),
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
    assert_eq!(tabs[1].custom_title.as_deref(), Some("feature/api"));
}

#[test]
fn tab_name_clear_removes_a_previous_custom_title() {
    let mut tabs = [tab(42, Some("manually changed"))];
    let request = TabNameRequest {
        attention_id: 42,
        name: None,
    };

    assert!(set_tab_name_on_tabs(tabs.iter_mut(), &request));
    assert_eq!(tabs[0].custom_title, None);
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
        sessions.iter().next().unwrap().custom_title.as_deref(),
        Some("feature/api")
    );
}
