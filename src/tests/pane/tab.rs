use super::*;

#[test]
fn tab_pane_index_resolves_panes_without_scanning() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let panes = [1, 2, 3]
        .into_iter()
        .map(|id| TerminalPane::new(id, profile.clone()).with_label_number(id as usize))
        .collect::<Vec<_>>();
    let mut tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes,
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
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
    };
    for pane in &tab.panes {
        assert!(std::ptr::eq(tab.pane(pane.id).unwrap(), pane));
    }
    assert!(tab.pane(99).is_none());

    tab.remove_pane(1);
    assert_eq!(tab.pane(2).map(|pane| pane.id), Some(2));
    assert_eq!(tab.pane(3).map(|pane| pane.id), Some(3));
    tab.push_pane(TerminalPane::new(4, profile));
    assert_eq!(tab.pane(4).map(|pane| pane.id), Some(4));
}

#[test]
fn split_profile_comes_from_the_active_pane() {
    let system = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let zsh = Profile {
        name: "Zsh".to_owned(),
        command: task::Shell::Program("zsh".to_owned()),
        theme: Some("One Light".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zsh,
    };
    let tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![
            TerminalPane::new(1, system).with_label_number(1),
            TerminalPane::new(2, zsh).with_label_number(2),
        ],
        pane_indices: HashMap::from([(1, 0), (2, 1)]),
        next_pane_label: 3,
        theme_override: None,
        layout: PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(2)),
        },
        active_pane: 2,
        focus_history: vec![1, 2],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
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
    };

    let profile = tab.active_profile().unwrap();
    assert_eq!(profile.name, "Zsh");
    assert_eq!(profile.theme.as_deref(), Some("One Light"));
}

#[test]
fn closing_active_pane_restores_previous_focus() {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = |id| TerminalPane::new(id, profile.clone()).with_label_number(id as usize);
    let mut tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![pane(1), pane(2), pane(3)],
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        theme_override: None,
        layout: PaneLayout::Pane(1),
        active_pane: 3,
        focus_history: vec![1, 2, 3],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
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
    };

    tab.remove_pane(3);
    tab.restore_focus_after_close(3, 1);

    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.focus_history, vec![1, 2]);
}

#[test]
fn closing_inactive_pane_preserves_focus() {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = |id| TerminalPane::new(id, profile.clone()).with_label_number(id as usize);
    let mut tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![pane(1), pane(2), pane(3)],
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        theme_override: None,
        layout: PaneLayout::Pane(1),
        active_pane: 3,
        focus_history: vec![1, 2, 3],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
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
    };

    tab.remove_pane(1);
    tab.restore_focus_after_close(1, 2);

    assert_eq!(tab.active_pane, 3);
    assert_eq!(tab.focus_history, vec![2, 3]);
}

fn pane_management_tab() -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = |id| TerminalPane::new(id, profile.clone()).with_label_number(id as usize);
    let layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: DEFAULT_PANE_SPLIT_RATIO,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };
    Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![pane(1), pane(2), pane(3)],
        pane_indices: HashMap::from([(1, 0), (2, 1), (3, 2)]),
        next_pane_label: 4,
        theme_override: None,
        layout,
        active_pane: 2,
        focus_history: vec![1, 3, 2],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
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
fn transferred_tabs_receive_target_window_ids_consistently() {
    let mut tab = pane_management_tab();
    let profile = tab.pane(2).unwrap().profile.clone();
    let pane = tab.pane_mut(2).unwrap();
    assert!(pane.stack.push(StackedPane::new(
        7,
        "first".to_owned(),
        profile.clone(),
        None,
        None
    )));
    assert!(pane.stack.push(StackedPane::new(
        8,
        "second".to_owned(),
        profile,
        None,
        None
    )));
    tab.attention_id = 99;
    tab.attention = Some(TabAttention {
        summary: "Build finished".to_owned(),
        body: Some("All tests passed".to_owned()),
    });
    tab.maximized_pane = Some(2);
    tab.minimized_panes = vec![1, 3];
    tab.selected_minimized_pane = Some(3);
    let mut next_pane_id = 20;

    let pane_ids = tab.reassign_ids(10, &mut next_pane_id);

    // The map is what lets a caller re-key anything it holds under the old ids.
    // A tab attached from the multiplexer arrives with the *publishing* window's
    // pane ids, and this window's registries — the multiplexer pane map, the
    // shared-pane registry, pane controls — are keyed by pane id alone, so two
    // tabs sharing an id share those entries and closing either one takes the
    // other's bookkeeping with it.
    assert_eq!(pane_ids, HashMap::from([(1, 20), (2, 21), (3, 22)]));
    assert_eq!(tab.id, 10);
    assert_eq!(tab.attention_id, 99);
    assert_eq!(
        tab.attention.as_ref().unwrap().tooltip_text(),
        "Build finished\nAll tests passed"
    );
    assert_eq!(next_pane_id, 25);
    assert_eq!(
        tab.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(),
        [20, 21, 22]
    );
    assert_eq!(
        tab.panes
            .iter()
            .map(|pane| pane.routing_id)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(tab.active_pane, 21);
    assert_eq!(tab.focus_history, [20, 22, 21]);
    assert_eq!(tab.maximized_pane, Some(21));
    assert_eq!(tab.minimized_panes, [20, 22]);
    assert_eq!(tab.selected_minimized_pane, Some(22));
    assert_eq!(
        tab.pane(21)
            .unwrap()
            .stack
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        [23, 24]
    );
    assert_eq!(
        tab.pane(21)
            .unwrap()
            .stack
            .entries
            .iter()
            .map(|entry| entry.routing_id)
            .collect::<Vec<_>>(),
        [7, 8]
    );
    assert_eq!(
        tab.pane(21).unwrap().stack.selected,
        PaneStackSelection::Stacked(24)
    );
    assert_eq!(
        tab.layout,
        PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(20)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(PaneLayout::Pane(21)),
                second: Box::new(PaneLayout::Pane(22)),
            }),
        }
    );
}

#[test]
fn tab_close_policy_distinguishes_close_from_unprotected_backgrounding() {
    assert!(TabClosePolicy::Close.background_authentication().is_none());
    assert!(
        TabClosePolicy::Background {
            authentication: None,
        }
        .background_authentication()
        .is_some_and(|authentication| authentication.is_none())
    );

    let authentication = SessionAuthentication::create("correct horse battery staple").unwrap();
    let selected = TabClosePolicy::Background {
        authentication: Some(authentication),
    }
    .background_authentication()
    .flatten()
    .unwrap();
    assert!(selected.verify("correct horse battery staple").is_some());
    assert!(selected.verify("wrong").is_none());
}

#[test]
fn maximizing_and_restoring_preserves_the_original_layout() {
    let mut tab = pane_management_tab();
    let original = tab.layout.clone();

    assert!(tab.toggle_maximize(2));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(2)));
    assert_eq!(tab.layout, original);

    assert!(tab.toggle_maximize(2));
    assert_eq!(tab.visible_layout(), Some(original.clone()));
    assert_eq!(tab.layout, original);
}

#[test]
fn maximizing_the_only_visible_pane_is_a_no_op() {
    let mut tab = pane_management_tab();

    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));

    assert!(!tab.toggle_maximize(1));
    assert_eq!(tab.maximized_pane, None);
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));
}

#[test]
fn pane_labels_remain_stable_and_are_not_reused() {
    let mut tab = pane_management_tab();

    assert_eq!(tab.pane(1).unwrap().label(), "Pane 1");
    assert_eq!(tab.pane(2).unwrap().label(), "Pane 2");
    assert_eq!(tab.pane(3).unwrap().label(), "Pane 3");

    let profile = tab.pane(1).unwrap().profile.clone();
    tab.remove_pane(2);
    tab.push_pane(TerminalPane::new(4, profile));

    assert_eq!(tab.pane(1).unwrap().label(), "Pane 1");
    assert_eq!(tab.pane(3).unwrap().label(), "Pane 3");
    assert_eq!(tab.pane(4).unwrap().label(), "Pane 4");
}

#[test]
fn custom_pane_labels_replace_the_fallback_and_render_while_editing() {
    let mut tab = pane_management_tab();

    tab.pane_mut(2).unwrap().generated_label = Some("dev · eu-west".to_owned());
    assert_eq!(tab.pane(2).unwrap().label(), "dev · eu-west");

    tab.pane_mut(2).unwrap().custom_label = Some("API server".to_owned());
    assert_eq!(tab.pane(2).unwrap().label(), "API server");

    tab.renaming_pane = Some(2);
    tab.rename_buffer = Some(TextField::new("Database"));
    tab.rename_buffer.as_mut().unwrap().cursor = 4;
    assert_eq!(tab.displayed_pane_label(2).as_deref(), Some("Data|base"));

    tab.pane_mut(2).unwrap().custom_label = None;
    tab.renaming_pane = None;
    tab.rename_buffer = None;
    assert_eq!(tab.pane(2).unwrap().label(), "dev · eu-west");
}

#[test]
fn template_labels_replace_generated_labels_but_preserve_manual_labels() {
    let mut tab = pane_management_tab();
    tab.pane_mut(1).unwrap().generated_label = Some("old-left".to_owned());
    tab.pane_mut(2).unwrap().generated_label = Some("old-middle".to_owned());
    tab.pane_mut(2).unwrap().custom_label = Some("Manual".to_owned());
    tab.pane_mut(3).unwrap().generated_label = Some("old-right".to_owned());

    tab.apply_generated_labels([
        (1, Some("top-left".to_owned())),
        (2, Some("top-right".to_owned())),
        (3, None),
    ]);

    assert_eq!(tab.pane(1).unwrap().label(), "top-left");
    assert_eq!(tab.pane(2).unwrap().label(), "Manual");
    assert_eq!(
        tab.pane(2).unwrap().generated_label.as_deref(),
        Some("top-right")
    );
    assert_eq!(tab.pane(3).unwrap().label(), "Pane 3");
    assert_eq!(tab.pane(3).unwrap().generated_label, None);
}

#[test]
fn pane_overlay_is_hidden_by_default_and_renders_while_editing() {
    let mut tab = pane_management_tab();

    assert_eq!(tab.displayed_pane_overlay(2), None);

    tab.pane_mut(2).unwrap().overlay_text = Some("Prod".to_owned());
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Prod"));

    tab.editing_overlay_pane = Some(2);
    tab.overlay_buffer = Some(TextField::new("Staging"));
    tab.overlay_buffer.as_mut().unwrap().cursor = 4;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Stag|ing"));

    tab.overlay_buffer.as_mut().unwrap().select_all = true;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Staging"));

    tab.overlay_buffer = Some(TextField::selected(""));
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("|"));

    tab.editing_overlay_pane = None;
    tab.overlay_buffer = None;
    assert_eq!(tab.displayed_pane_overlay(2).as_deref(), Some("Prod"));

    tab.pane_mut(2).unwrap().overlay_text = None;
    assert_eq!(tab.displayed_pane_overlay(2), None);
}

#[test]
fn overlay_style_picker_preserves_committed_text_and_holds_values() {
    let mut tab = pane_management_tab();
    let pane_id = 2;
    tab.pane_mut(pane_id).unwrap().overlay_text = Some("Prod".to_owned());
    tab.editing_overlay_pane = None;
    tab.overlay_buffer = None;

    tab.overlay_style_picker = Some(OverlayStylePicker {
        pane_id,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::Large,
        original_font_size: None,
        hue: 0.6,
        saturation: 0.7,
        value: 0.4,
        original_color: None,
        preset_index: 0,
        opacity_percent: 60,
        original_opacity: Some(0.6),
        hex_buffer: String::new(),
    });
    let picker = tab.overlay_style_picker.as_ref().unwrap();

    assert_eq!(picker.opacity_percent, 60);
    assert_eq!(picker.section, OverlayPickerSection::Color);
    assert_eq!(picker.font_size, OverlayFontSize::Large);
    assert_eq!(tab.displayed_pane_overlay(pane_id).as_deref(), Some("Prod"));
    assert_eq!(tab.editing_overlay_pane, None);
}

#[test]
fn tabs_use_the_terminal_icon_by_default() {
    assert_eq!(pane_management_tab().icon, Some(IconName::Terminal));
}

#[test]
fn minimizing_and_restoring_preserves_the_nested_split_position() {
    let mut tab = pane_management_tab();
    let original = tab.layout.clone();

    assert!(tab.minimize(2));
    assert_eq!(tab.minimized_panes, vec![2]);
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert_eq!(tab.active_pane, 3);
    assert_eq!(
        tab.visible_layout(),
        Some(PaneLayout::Split {
            axis: SplitAxis::Vertical,
            first_ratio: DEFAULT_PANE_SPLIT_RATIO,
            first: Box::new(PaneLayout::Pane(1)),
            second: Box::new(PaneLayout::Pane(3)),
        })
    );
    assert_eq!(tab.layout, original);

    assert!(tab.restore_minimized(2));
    assert_eq!(tab.selected_minimized_pane, None);
    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.visible_layout(), Some(original.clone()));
    assert_eq!(tab.layout, original);
}

#[test]
fn minimized_pane_selection_wraps_and_restore_uses_the_selection() {
    let mut tab = pane_management_tab();

    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert_eq!(tab.selected_minimized_pane, Some(3));

    assert!(tab.select_previous_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert!(tab.select_previous_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(3));
    assert!(tab.select_next_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(2));

    assert!(tab.restore_last_minimized());
    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.minimized_panes, vec![3]);
    assert_eq!(tab.selected_minimized_pane, Some(3));
}

#[test]
fn closing_the_selected_minimized_pane_selects_a_surviving_item() {
    let mut tab = pane_management_tab();
    assert!(tab.minimize(2));
    assert!(tab.minimize(3));

    tab.remove_pane(3);
    let layout = tab.layout.clone().without(3).unwrap();
    tab.restore_focus_after_close(3, layout.first_pane());
    tab.layout = layout;

    assert_eq!(tab.minimized_panes, vec![2]);
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));
}

#[test]
fn closing_the_only_visible_pane_restores_the_most_recently_minimized_pane() {
    let mut tab = pane_management_tab();
    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert!(tab.select_previous_minimized());
    assert_eq!(tab.selected_minimized_pane, Some(2));

    tab.remove_pane(1);
    tab.layout = tab.layout.clone().without(1).unwrap();
    tab.restore_focus_after_close(1, tab.layout.first_pane());

    assert_eq!(tab.minimized_panes, vec![2]);
    assert_eq!(tab.selected_minimized_pane, Some(2));
    assert_eq!(tab.active_pane, 3);
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(3)));
}

#[test]
fn closing_the_only_visible_pane_restores_a_sole_minimized_pane() {
    let mut tab = pane_management_tab();
    tab.remove_pane(3);
    tab.layout = tab.layout.clone().without(3).unwrap();
    tab.restore_focus_after_close(3, tab.layout.first_pane());
    assert!(tab.minimize(2));

    tab.remove_pane(1);
    tab.layout = tab.layout.clone().without(1).unwrap();
    tab.restore_focus_after_close(1, tab.layout.first_pane());

    assert!(tab.minimized_panes.is_empty());
    assert_eq!(tab.selected_minimized_pane, None);
    assert_eq!(tab.active_pane, 2);
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(2)));
    assert!(!tab.restore_last_minimized());
}

#[test]
fn at_least_one_pane_must_remain_visible() {
    let mut tab = pane_management_tab();

    assert!(tab.minimize(2));
    assert!(tab.minimize(3));
    assert!(!tab.minimize(1));
    assert_eq!(tab.visible_layout(), Some(PaneLayout::Pane(1)));
    assert!(tab.restore_last_minimized());
    assert_eq!(tab.active_pane, 3);
}

#[test]
fn setting_a_tab_icon_updates_the_effective_value_and_override_marker() {
    let mut tab = pane_management_tab();

    tab.set_icon_override(Some(IconName::Folder));
    assert_eq!(tab.icon, Some(IconName::Folder));
    assert_eq!(tab.icon_override, TabIconOverride::Icon(IconName::Folder));

    tab.set_icon_override(None);
    assert_eq!(tab.icon, None);
    assert_eq!(tab.icon_override, TabIconOverride::Hidden);
}
