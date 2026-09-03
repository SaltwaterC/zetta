use super::*;

fn pin_test_tab(id: u64, pinned: bool) -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = TerminalPane::new(id, profile).with_label_number(1);
    Tab {
        id,
        attention_id: id,
        attention: None,
        panes: vec![pane],
        pane_indices: HashMap::from([(id, 0)]),
        next_pane_label: 2,
        theme_override: None,
        layout: PaneLayout::Pane(id),
        active_pane: id,
        focus_history: vec![id],
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
        pinned,
        renaming_pane: None,
        rename_buffer: None,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_style_picker: None,
    }
}

fn tab_ids(tabs: &[Tab]) -> Vec<u64> {
    tabs.iter().map(|tab| tab.id).collect()
}

#[test]
fn launch_theme_override_applies_case_insensitively_by_name_only() {
    let mut profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    apply_launch_theme_override(
        &mut profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(profile.theme, Some("Override Theme".to_owned()));
    assert_eq!(profile.dark_theme, Some("Override Theme".to_owned()));

    let mut other_profile = Profile {
        name: "Other".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    apply_launch_theme_override(
        &mut other_profile,
        Some(&("system".to_owned(), "Override Theme".to_owned())),
    );
    assert_eq!(other_profile.theme, Some("Configured Theme".to_owned()));
    assert_eq!(other_profile.dark_theme, None);

    let mut unaffected_profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: Some("Configured Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    apply_launch_theme_override(&mut unaffected_profile, None);
    assert_eq!(
        unaffected_profile.theme,
        Some("Configured Theme".to_owned())
    );
}

#[test]
fn cli_replacement_profile_resolution_is_case_insensitive_and_preserves_split_defaults() {
    let profiles = [
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: Some("Configured Theme".to_owned()),
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Alternate".to_owned(),
            command: Shell::Program("alternate-shell".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
    ];

    let selected =
        resolve_cli_replacement_profile(&profiles, Some("sYsTeM"), Some("Dracula"), None)
            .unwrap()
            .unwrap();
    assert_eq!(selected.name, "System");
    assert_eq!(selected.theme.as_deref(), Some("Dracula"));
    assert_eq!(selected.dark_theme.as_deref(), Some("Dracula"));
    assert_eq!(profiles[0].theme.as_deref(), Some("Configured Theme"));

    assert_eq!(
        resolve_cli_replacement_profile(&profiles, None, None, None),
        Some(None)
    );
    assert!(resolve_cli_replacement_profile(&profiles, Some("missing"), None, None).is_none());
    assert!(resolve_cli_replacement_profile(&profiles, None, Some("Dracula"), None).is_none());
    assert!(resolve_cli_replacement_profile(&profiles, Some("System"), Some(""), None).is_none());
}

#[test]
fn cli_replacement_profile_resolution_requires_the_exact_homebrew_name() {
    let homebrew_profile = Profile {
        name: "Fish (Homebrew)".to_owned(),
        command: Shell::Program("/opt/homebrew/bin/fish".to_owned()),
        theme: Some("Homebrew Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Fish,
    };
    let launch_theme_override = ("fish (homebrew)".to_owned(), "Launch Theme".to_owned());

    assert!(
        resolve_cli_replacement_profile(
            std::slice::from_ref(&homebrew_profile),
            Some("fIsH"),
            None,
            None,
        )
        .is_none()
    );

    let selected = resolve_cli_replacement_profile(
        std::slice::from_ref(&homebrew_profile),
        Some("fIsH (hOmEbReW)"),
        None,
        Some(&launch_theme_override),
    )
    .unwrap()
    .unwrap();
    assert_eq!(selected.name, "Fish (Homebrew)");
    assert_eq!(selected.theme.as_deref(), Some("Launch Theme"));
    assert_eq!(selected.dark_theme.as_deref(), Some("Launch Theme"));

    let exact_profile = Profile {
        name: "Fish".to_owned(),
        command: Shell::Program("fish".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Fish,
    };
    let profiles = [homebrew_profile, exact_profile];
    let selected = resolve_cli_replacement_profile(&profiles, Some("fIsH"), None, None)
        .unwrap()
        .unwrap();
    assert_eq!(selected.name, "Fish");

    let selected = resolve_cli_replacement_profile(&profiles, Some("fIsH (hOmEbReW)"), None, None)
        .unwrap()
        .unwrap();
    assert_eq!(selected.name, "Fish (Homebrew)");
}

#[test]
fn pane_template_leaf_resolution_applies_profile_command_theme_environment_and_overlay() {
    let config = Config::parse(
        r##"{
            "profiles": [
                { "name": "Worker", "program": "worker-shell", "theme": "One Dark" }
            ],
            "pane_split_templates": {
                "custom": {
                    "env": { "SHARED": "yes", "ROLE": "default" },
                    "layout": {
                        "vertical": [
                            {
                                "label": "worker",
                                "profile": "wOrKeR",
                                "theme": "One Light",
                                "env": { "ROLE": "worker" },
                                "overlay": {
                                    "text": "WORKER",
                                    "size": "2xl",
                                    "opacity": 40,
                                    "color": "#ff00ff"
                                }
                            },
                            {
                                "label": "ssh",
                                "command": { "program": "ssh", "args": ["host", "-p", "2200"] }
                            }
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();
    let active = config
        .profiles
        .iter()
        .find(|profile| profile.name == "System")
        .cloned()
        .unwrap();
    let leaves =
        resolve_pane_split_leaves(&config.pane_split_templates["custom"], &active, None).unwrap();

    assert_eq!(leaves[0].label.as_deref(), Some("worker"));
    assert_eq!(leaves[0].profile.name, "Worker");
    assert_eq!(leaves[0].profile.theme.as_deref(), Some("One Light"));
    assert_eq!(leaves[0].environment["ROLE"], "worker");
    assert_eq!(leaves[0].environment["SHARED"], "yes");
    assert_eq!(leaves[0].overlay_text.as_deref(), Some("WORKER"));
    assert_eq!(
        leaves[0].overlay_font_size,
        Some(OverlayFontSize::ExtraExtraLarge)
    );
    assert_eq!(leaves[0].overlay_opacity, Some(0.4));
    assert_eq!(leaves[0].overlay_color, overlay_color_from_value("#ff00ff"));

    assert_eq!(leaves[1].label.as_deref(), Some("ssh"));
    assert_eq!(leaves[1].environment["ROLE"], "default");
    assert_eq!(leaves[1].environment["SHARED"], "yes");
    assert_eq!(leaves[1].profile.name, "System");
    assert_eq!(
        leaves[1].profile.command,
        Shell::WithArguments {
            program: "ssh".to_owned(),
            args: vec!["host".to_owned(), "-p".to_owned(), "2200".to_owned()],
            title_override: None,
        }
    );
}

#[test]
fn pane_template_leaf_resolution_quotes_stacked_commands_for_the_resolved_shell() {
    let config = Config::parse(
        r##"{
            "pane_split_templates": {
                "custom": {
                    "layout": {
                        "vertical": [
                            {
                                "label": "server",
                                "stack": [
                                    { "program": "cargo", "args": ["watch", "-x", "test"] },
                                    { "program": "tail", "args": ["-f", "logs/my app.log"] }
                                ]
                            },
                            { "label": "client" }
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();
    let active = config
        .profiles
        .iter()
        .find(|profile| profile.name == "System")
        .cloned()
        .unwrap();

    let leaves =
        resolve_pane_split_leaves(&config.pane_split_templates["custom"], &active, None).unwrap();

    assert_eq!(leaves[0].stack.len(), 2);
    assert_eq!(leaves[0].stack[0], "cargo watch -x test");
    // The argument with a space has to arrive quoted for the host shell, the way
    // `zetta pane --stack` quotes it.
    assert_eq!(
        leaves[0].stack[1],
        quote_pane_command_for_shell(
            &active.command,
            &[
                "tail".to_owned(),
                "-f".to_owned(),
                "logs/my app.log".to_owned(),
            ],
        )
        .unwrap()
    );
    assert_ne!(leaves[0].stack[1], "tail -f logs/my app.log");
    assert!(leaves[1].stack.is_empty());
}

#[test]
fn pane_template_labels_and_overlays_do_not_require_a_terminal_restart() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = TerminalPane::new(1, profile.clone());
    let leaf = ResolvedPaneSplitLeaf {
        label: Some("new-label".to_owned()),
        profile,
        environment: HashMap::new(),
        overlay_text: Some("overlay".to_owned()),
        overlay_font_size: Some(OverlayFontSize::Large),
        overlay_opacity: Some(0.5),
        overlay_color: overlay_color_from_value("cyan"),
        stack: Vec::new(),
    };

    assert!(!pane_split_leaf_requires_restart(&pane, &leaf));
    let mut changed = leaf.clone();
    changed
        .environment
        .insert("ROLE".to_owned(), "worker".to_owned());
    assert!(pane_split_leaf_requires_restart(&pane, &changed));

    // A declared stack has to be rebuilt from scratch, or re-applying the
    // template would append its commands to the stack the pane already has.
    let mut stacked = leaf.clone();
    stacked.stack = vec!["cargo watch".to_owned()];
    assert!(pane_split_leaf_requires_restart(&pane, &stacked));
}

#[test]
fn mouse_window_resize_clamps_each_dimension_to_the_minimum() {
    assert_eq!(
        clamp_window_size_to_minimum(size(px(400.), px(500.))),
        size(px(520.), px(500.))
    );
    assert_eq!(
        clamp_window_size_to_minimum(size(px(600.), px(200.))),
        size(px(600.), px(320.))
    );
}

#[test]
fn pane_resize_mode_pauses_terminal_input() {
    assert!(pane_input_enabled(false));
    assert!(!pane_input_enabled(true));
}

#[test]
fn tab_move_mode_moves_adjacent_tabs_without_wrapping() {
    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        move_item_by_id(&mut ids, 3, TabMoveDirection::Left, 3, true, |id| *id),
        Some(1)
    );
    assert_eq!(ids, vec![1, 3, 2, 4]);

    assert_eq!(
        move_item_by_id(&mut ids, 3, TabMoveDirection::Right, 3, true, |id| *id),
        Some(2)
    );
    assert_eq!(ids, vec![1, 2, 3, 4]);
}

#[test]
fn tab_move_mode_stops_at_boundaries_and_when_disabled() {
    let mut ids = vec![1, 2, 3];
    assert_eq!(
        move_item_by_id(&mut ids, 1, TabMoveDirection::Left, 1, true, |id| *id),
        None
    );
    assert_eq!(
        move_item_by_id(&mut ids, 3, TabMoveDirection::Right, 3, true, |id| *id),
        None
    );
    assert_eq!(
        move_item_by_id(&mut ids, 2, TabMoveDirection::Right, 2, false, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn visual_pinning_toggles_at_the_pinned_prefix_without_losing_the_active_tab() {
    let mut tabs = vec![
        pin_test_tab(1, true),
        pin_test_tab(2, false),
        pin_test_tab(3, false),
    ];

    let active_index = toggle_tab_pinning_in_order(&mut tabs, 2);
    assert_eq!(active_index, Some(1));
    assert_eq!(tab_ids(&tabs), vec![1, 3, 2]);
    assert!(tabs[1].pinned);

    let active_index = toggle_tab_pinning_in_order(&mut tabs, 0);
    assert_eq!(active_index, Some(1));
    assert_eq!(tab_ids(&tabs), vec![3, 1, 2]);
    assert!(!tabs[1].pinned);
    assert_eq!(pinned_tab_count(&tabs), 1);
}

#[test]
fn reconnected_tabs_reenter_the_pinned_prefix() {
    let mut tabs = vec![pin_test_tab(1, true), pin_test_tab(2, false)];
    let pinned_index = insert_tab_in_pin_order(&mut tabs, pin_test_tab(3, true));
    assert_eq!(pinned_index, 1);
    assert_eq!(tab_ids(&tabs), vec![1, 3, 2]);

    let unpinned_index = insert_tab_in_pin_order(&mut tabs, pin_test_tab(4, false));
    assert_eq!(unpinned_index, 3);
    assert_eq!(tab_ids(&tabs), vec![1, 3, 2, 4]);
}

#[test]
fn tab_move_and_drag_boundaries_preserve_the_pinned_prefix() {
    let tabs = vec![
        pin_test_tab(1, true),
        pin_test_tab(2, true),
        pin_test_tab(3, false),
        pin_test_tab(4, false),
    ];
    assert!(tab_move_preserves_pinning(&tabs, 1, TabMoveDirection::Left));
    assert!(!tab_move_preserves_pinning(
        &tabs,
        1,
        TabMoveDirection::Right
    ));
    assert!(!tab_move_preserves_pinning(
        &tabs,
        2,
        TabMoveDirection::Left
    ));
    assert!(tab_move_preserves_pinning(
        &tabs,
        2,
        TabMoveDirection::Right
    ));
    assert!(tab_drop_preserves_pinning(
        &tabs,
        1,
        TabDropPosition::Before(2)
    ));
    assert!(!tab_drop_preserves_pinning(
        &tabs,
        1,
        TabDropPosition::After(3)
    ));
    assert!(tab_drop_preserves_pinning(
        &tabs,
        3,
        TabDropPosition::After(4)
    ));
    assert!(!tab_drop_preserves_pinning(
        &tabs,
        3,
        TabDropPosition::Before(2)
    ));
}

#[test]
fn tab_move_mode_preserves_the_logical_active_tab() {
    let mut ids = vec![1, 2, 3, 4];
    let active_index = move_item_by_id(&mut ids, 2, TabMoveDirection::Right, 2, true, |id| *id);
    assert_eq!(ids, vec![1, 3, 2, 4]);
    assert_eq!(active_index, Some(2));
}

#[test]
fn application_menu_navigation_wraps_in_both_directions() {
    assert_eq!(
        adjacent_application_menu_index(2, 0, ApplicationMenuDirection::Left),
        1
    );
    assert_eq!(
        adjacent_application_menu_index(2, 1, ApplicationMenuDirection::Right),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Left),
        0
    );
    assert_eq!(
        adjacent_application_menu_index(3, 1, ApplicationMenuDirection::Right),
        2
    );
}

#[test]
fn exited_terminal_is_not_backgrounded_by_the_tab_pin() {
    let pinned = TabClosePolicy::Background {
        authentication: None,
    };

    assert!(background_authentication_for_close(&pinned, false, true, false).is_some());
    assert!(background_authentication_for_close(&pinned, false, false, false).is_none());
    assert!(background_authentication_for_close(&pinned, false, true, true).is_none());
}

#[test]
fn a_shared_tab_is_backgrounded_when_it_closes() {
    assert!(matches!(
        background_authentication_for_close(&TabClosePolicy::Close, true, true, false),
        Some(None)
    ));
    assert!(
        background_authentication_for_close(&TabClosePolicy::Close, false, true, false).is_none()
    );
    assert!(
        background_authentication_for_close(&TabClosePolicy::Close, true, true, true).is_none()
    );
}

#[test]
fn new_tab_inherits_the_active_profile_after_an_explicit_profile_tab_closes() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };

    let profile = new_tab_profile(
        Some(&system),
        &[system.clone(), alternate],
        0,
        NewTabProfile::Inherit,
    )
    .unwrap();

    assert_eq!(profile.name, "System");
}

#[test]
fn first_tab_uses_the_configured_default_profile() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };

    let profile = new_tab_profile(None, &[system, alternate], 1, NewTabProfile::Default).unwrap();

    assert_eq!(profile.name, "Alternate");
}

#[test]
fn default_new_tabs_ignore_the_active_profile() {
    let system = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let alternate = Profile {
        name: "Alternate".to_owned(),
        command: Shell::Program("alternate-shell".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };

    let profile = new_tab_profile(
        Some(&alternate),
        &[system, alternate.clone()],
        0,
        NewTabProfile::Default,
    )
    .unwrap();

    assert_eq!(profile.name, "System");
}

#[test]
fn tab_reorder_moves_tabs_before_and_after_targets_in_both_directions() {
    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 3, TabDropPosition::Before(1), 2, |id| *id),
        Some(2)
    );
    assert_eq!(ids, vec![3, 1, 2, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 3, TabDropPosition::After(1), 2, |id| *id),
        Some(2)
    );
    assert_eq!(ids, vec![1, 3, 2, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 1, TabDropPosition::Before(3), 2, |id| *id),
        Some(0)
    );
    assert_eq!(ids, vec![2, 1, 3, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 1, TabDropPosition::After(3), 2, |id| *id),
        Some(0)
    );
    assert_eq!(ids, vec![2, 3, 1, 4]);
}

#[test]
fn tab_reorder_adjusts_the_target_index_after_removing_the_source() {
    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 1, TabDropPosition::After(3), 4, |id| *id),
        Some(3)
    );
    assert_eq!(ids, vec![2, 3, 1, 4]);

    let mut ids = vec![1, 2, 3, 4];
    assert_eq!(
        reorder_items_by_id(&mut ids, 4, TabDropPosition::Before(2), 1, |id| *id),
        Some(0)
    );
    assert_eq!(ids, vec![1, 4, 2, 3]);
}

#[test]
fn tab_reorder_preserves_the_active_tab_when_moving_active_or_inactive_tabs() {
    let mut ids = vec![1, 2, 3, 4];
    let active_index = reorder_items_by_id(&mut ids, 3, TabDropPosition::After(1), 3, |id| *id);
    assert_eq!(ids, vec![1, 3, 2, 4]);
    assert_eq!(active_index, Some(1));

    let mut ids = vec![1, 2, 3, 4];
    let active_index = reorder_items_by_id(&mut ids, 1, TabDropPosition::After(4), 3, |id| *id);
    assert_eq!(ids, vec![2, 3, 4, 1]);
    assert_eq!(active_index, Some(1));
}

#[test]
fn tab_reorder_ignores_same_tab_invalid_target_empty_and_outside_drops() {
    let mut ids = vec![1, 2, 3];
    assert_eq!(
        reorder_items_by_id(&mut ids, 2, TabDropPosition::Before(2), 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);

    assert_eq!(
        reorder_items_by_id(&mut ids, 2, TabDropPosition::After(99), 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);

    assert_eq!(
        reorder_items_by_id(&mut ids, 99, TabDropPosition::After(1), 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);

    assert_eq!(
        reorder_items_by_id(
            &mut Vec::<u64>::new(),
            1,
            TabDropPosition::After(2),
            1,
            |id| *id
        ),
        None
    );
    assert_eq!(
        reorder_items_by_id(&mut ids, 2, TabDropPosition::Outside, 1, |id| *id),
        None
    );
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn overflow_selection_side_distinguishes_left_and_right_entries() {
    assert_eq!(tab_overflow_selection_side(1, 4), Some(false));
    assert_eq!(tab_overflow_selection_side(7, 4), Some(true));
    assert_eq!(tab_overflow_selection_side(4, 4), None);
}

#[test]
fn opening_a_project_starts_in_the_project_directory_regardless_of_scope() {
    for scope in [
        WorkingDirectoryScope::None,
        WorkingDirectoryScope::Pane,
        WorkingDirectoryScope::Tab,
    ] {
        assert!(
            !NewTabOrigin::ProjectEntry.inherits_working_directory(scope),
            "entering a project must not inherit the session directory with scope {scope:?}"
        );
    }
}

#[test]
fn a_new_tab_in_the_current_session_follows_the_configured_scope() {
    assert!(!NewTabOrigin::CurrentSession.inherits_working_directory(WorkingDirectoryScope::None));
    assert!(NewTabOrigin::CurrentSession.inherits_working_directory(WorkingDirectoryScope::Tab));
}

#[test]
fn a_transient_notice_is_taken_away_by_its_own_timer_only() {
    // The banner it replaces stayed on screen until something else overwrote
    // it, which made a sentence of advice read as an unresolved error with no
    // way to clear it.
    let mut notice = TransientNotice::default();
    let first = notice.show("attach it from another window".to_owned());
    assert_eq!(notice.message(), Some("attach it from another window"));

    // A later notice takes over, and the earlier one's timer must not then
    // remove it: the generation is what keeps the two apart.
    let second = notice.show("this tab can now be joined".to_owned());
    assert!(!notice.dismiss_if_current(first));
    assert_eq!(notice.message(), Some("this tab can now be joined"));

    assert!(notice.dismiss_if_current(second));
    assert_eq!(notice.message(), None);
    // Dismissing twice is not an error, and does not report a second change.
    assert!(!notice.dismiss_if_current(second));
}
