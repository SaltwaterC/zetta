use super::*;

#[test]
fn icon_search_is_case_insensitive_and_matches_icon_names() {
    assert!(matching_tab_icons("TERMINAL").contains(&IconName::Terminal));
    assert!(matching_tab_icons("arrow").contains(&IconName::ArrowLeft));
    assert!(!matching_tab_icons("not-an-icon").contains(&IconName::Terminal));
}

#[test]
fn empty_icon_search_returns_every_icon() {
    assert_eq!(
        matching_tab_icons("").len(),
        <IconName as strum::IntoEnumIterator>::iter().count()
    );
}

#[test]
fn icon_options_include_none_and_filter_it_by_name() {
    assert_eq!(matching_tab_icon_options("none"), vec![None]);
    assert!(matching_tab_icon_options("").contains(&None));
    assert!(!matching_tab_icon_options("terminal").contains(&None));
}

#[test]
fn virtualized_grid_uses_row_indices_at_column_boundaries() {
    assert_eq!(tab_icon_row(0), 0);
    assert_eq!(tab_icon_row(TAB_ICON_COLUMNS - 1), 0);
    assert_eq!(tab_icon_row(TAB_ICON_COLUMNS), 1);
    assert_eq!(tab_icon_row(TAB_ICON_COLUMNS * 3 + 2), 3);
}

#[test]
fn picker_filters_cached_entries_without_rebuilding_labels() {
    let entries = build_icon_entries(&[IconName::Terminal, IconName::Folder]).into();
    let mut picker =
        TabIconPicker::new(TabIconPickerTarget::Tab(0), Some(IconName::Folder), entries);

    assert_eq!(picker.selected, 2);
    assert_eq!(picker.entries[0].label.as_ref(), "Terminal");
    assert_eq!(picker.entries[0].search_label, "terminal");

    let options = picker.options();
    assert_eq!(options.as_ref(), &[None, Some(0), Some(1)]);
    assert_eq!(picker.icon_for_option(options[0]), None);
    assert_eq!(picker.icon_for_option(options[2]), Some(IconName::Folder));

    picker.query.text = "folder".to_owned();
    let filtered = picker.options();
    assert_eq!(filtered.as_ref(), &[Some(1)]);
}

#[test]
fn cli_icon_names_are_snake_case_and_include_none() {
    let names = tab_icon_completion_names().collect::<Vec<_>>();
    assert_eq!(names.first(), Some(&"none"));
    assert!(names.contains(&"terminal"));
    assert_eq!(parse_tab_icon_name("terminal"), Some(IconName::Terminal));
    assert_eq!(parse_tab_icon_name("Terminal"), Some(IconName::Terminal));
    assert_eq!(parse_tab_icon_name("not-an-icon"), None);
}

fn icon_test_tab(attention_id: u64) -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = TerminalPane::new(1, profile).with_label_number(1);
    Tab {
        id: attention_id,
        attention_id,
        attention: None,
        panes: vec![pane],
        pane_indices: HashMap::from([(1, 0)]),
        next_pane_label: 2,
        theme_override: None,
        layout: PaneLayout::Pane(1),
        active_pane: 1,
        focus_history: vec![1],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        protected: false,
        shared: false,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: Some(IconName::Terminal),
        icon_override: TabIconOverride::None,
        pinned: false,
        renaming_pane: None,
        rename_buffer: None,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_style_picker: None,
    }
}

#[test]
fn tab_icon_hooks_target_the_originating_tab_without_changing_focus() {
    let mut tabs = [icon_test_tab(1), icon_test_tab(42)];
    let active_tab = 0;
    let target = tab_icon_target_index(&tabs, active_tab, Some(42)).unwrap();
    tabs[target].set_icon_override(Some(IconName::AiOpenAi));
    assert_eq!(tabs[0].icon_override, TabIconOverride::None);
    assert_eq!(
        tabs[1].icon_override,
        TabIconOverride::Icon(IconName::AiOpenAi)
    );
    assert_eq!(tab_icon_target_index(&tabs, active_tab, Some(99)), None);
    assert_eq!(tab_icon_target_index(&tabs, active_tab, None), Some(0));
    assert_eq!(tab_icon_target_index(&[], active_tab, None), None);
}

#[test]
fn ending_another_codex_session_restores_only_its_own_tab_icon() {
    let mut tabs = [icon_test_tab(1), icon_test_tab(42)];
    for tab in &mut tabs {
        tab.set_icon_override(Some(IconName::AiOpenAi));
    }
    let active_tab = 0;
    let target = tab_icon_target_index(&tabs, active_tab, Some(42)).unwrap();
    let tab = &mut tabs[target];
    crate::project_context::reset_project_tab_icon(
        tab.id,
        &mut tab.icon,
        &mut tab.icon_override,
        Some(IconName::Folder),
        None,
        &mut HashMap::new(),
    );
    assert_eq!(
        tabs[0].icon_override,
        TabIconOverride::Icon(IconName::AiOpenAi)
    );
    assert_eq!(tabs[1].icon_override, TabIconOverride::None);
    assert_eq!(tabs[1].icon, Some(IconName::Folder));
}
