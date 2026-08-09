use super::*;

#[test]
fn profile_shortcuts_match_the_shifted_number_row() {
    const SHIFTED_DIGITS: [&str; 10] = ["!", "@", "#", "$", "%", "^", "&", "*", "(", ")"];
    let keyboard_mapper = gpui::DummyKeyboardMapper;
    for (index, symbol) in SHIFTED_DIGITS.into_iter().enumerate() {
        let slot = index + 1;
        let bindings = profile_keybindings(slot, &keyboard_mapper);
        let shifted = gpui::Keystroke::parse(&format!("ctrl-{symbol}")).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].match_keystrokes(&[shifted]), Some(false));
    }
}

#[test]
fn profile_shortcut_labels_use_number_row_aliases() {
    let keyboard_mapper = gpui::DummyKeyboardMapper;
    let slot_one = profile_keybindings(1, &keyboard_mapper)[0].clone();
    let slot_nine = profile_keybindings(9, &keyboard_mapper)[0].clone();
    let slot_ten = profile_keybindings(10, &keyboard_mapper)[0].clone();
    let remapped = KeyBinding::new("alt-p", OpenProfile { slot: 1 }, Some("Zetta > Terminal"));

    assert_eq!(
        profile_shortcut_label(1, &slot_one, &slot_one).as_deref(),
        Some("Ctrl+Shift+1")
    );
    assert_eq!(
        profile_shortcut_label(9, &slot_nine, &slot_nine).as_deref(),
        Some("Ctrl+Shift+9")
    );
    assert_eq!(
        profile_shortcut_label(10, &slot_ten, &slot_ten).as_deref(),
        Some("Ctrl+Shift+0")
    );
    assert_eq!(profile_shortcut_label(11, &slot_ten, &slot_ten), None);
    assert_eq!(profile_shortcut_label(1, &remapped, &slot_one), None);
}

#[test]
fn profile_shortcut_labels_survive_keyboard_layout_mapping() {
    // On the British layout, GPUI's mapper turns the shifted `#` source into
    // `£` before it reaches the menu renderer.
    let expected = KeyBinding::new("ctrl-£", OpenProfile { slot: 3 }, Some("Zetta > Terminal"));
    let mapped = KeyBinding::new("ctrl-£", OpenProfile { slot: 3 }, Some("Zetta > Terminal"));
    let raw = KeyBinding::new("ctrl-#", OpenProfile { slot: 3 }, Some("Zetta > Terminal"));

    assert_eq!(
        profile_shortcut_label(3, &mapped, &expected).as_deref(),
        Some("Ctrl+Shift+3")
    );
    assert_eq!(profile_shortcut_label(3, &raw, &expected), None);
}

#[test]
fn pane_template_shortcuts_are_built_in() {
    let [three_right, quarters] = pane_template_keybindings();
    let three_right_key = gpui::Keystroke::parse(&platform_keystroke("alt-shift-o")).unwrap();
    let quarters_key = gpui::Keystroke::parse(&platform_keystroke("alt-shift-e")).unwrap();

    assert_eq!(
        three_right.match_keystrokes(&[three_right_key]),
        Some(false)
    );
    assert_eq!(quarters.match_keystrokes(&[quarters_key]), Some(false));
}

#[test]
fn tab_rename_and_configuration_reload_shortcuts_are_swapped() {
    assert_eq!(RENAME_TAB_KEYBINDING, "ctrl-shift-r");
    assert_eq!(CHANGE_TAB_ICON_KEYBINDING, "ctrl-shift-y");
    assert_eq!(
        RELOAD_CONFIGURATION_KEYBINDING,
        platform_keystroke("ctrl-alt-r")
    );
    assert_ne!(RENAME_TAB_KEYBINDING, RELOAD_CONFIGURATION_KEYBINDING);
}

#[test]
fn pane_label_uses_the_documented_shortcut() {
    assert_eq!(RENAME_PANE_KEYBINDING, platform_keystroke("alt-shift-r"));
}

#[test]
fn hide_window_uses_the_platform_shortcuts() {
    let bindings = default_keybindings(0, &gpui::DummyKeyboardMapper);
    let hide_bindings = bindings
        .iter()
        .filter(|binding| binding.action().name() == HideWindow.name())
        .collect::<Vec<_>>();

    let expected_shortcuts = if cfg!(target_os = "macos") {
        vec!["cmd-h", "cmd-shift-h"]
    } else {
        vec![HIDE_WINDOW_KEYBINDING]
    };
    assert_eq!(hide_bindings.len(), expected_shortcuts.len());
    for shortcut in expected_shortcuts {
        let shortcut = gpui::Keystroke::parse(shortcut).unwrap();
        assert!(hide_bindings.iter().any(|binding| {
            binding.match_keystrokes(std::slice::from_ref(&shortcut)) == Some(false)
        }));
    }
}

#[test]
fn default_binding_index_keeps_all_shortcuts_for_an_action() {
    let bindings = default_keybindings(0, &gpui::DummyKeyboardMapper);
    let indexed = index_default_bindings(&bindings);
    let terminal_context = Some("Zetta > Terminal".to_owned());

    assert_eq!(
        indexed.get(&(
            IncreaseTerminalFontSize.name().to_owned(),
            terminal_context.clone()
        )),
        Some(&vec!["^=".to_owned(), "^+".to_owned()])
    );

    let window_context = None;
    if cfg!(target_os = "macos") {
        assert_eq!(
            indexed.get(&(HideWindow.name().to_owned(), window_context)),
            Some(&vec!["⌘H".to_owned(), "⌘⇧H".to_owned()])
        );
    } else {
        assert_eq!(
            indexed.get(&(HideWindow.name().to_owned(), Some("Zetta".to_owned()))),
            Some(&vec!["ctrl-shift-h".to_owned()])
        );
    }
}

#[test]
fn pane_control_actions_have_no_built_in_keyboard_bindings() {
    let bindings = default_keybindings(0, &gpui::DummyKeyboardMapper);
    let pane_control_actions = [TogglePaneControls.name(), ToggleTabPaneControls.name()];
    assert!(
        bindings
            .iter()
            .all(|binding| !pane_control_actions.contains(&binding.action().name()))
    );
}

#[test]
fn pane_layout_rotation_uses_the_requested_shortcut() {
    assert_eq!(
        ROTATE_PANE_LAYOUT_KEYBINDING,
        platform_keystroke("alt-shift-l")
    );
    let shortcut = gpui::Keystroke::parse(ROTATE_PANE_LAYOUT_KEYBINDING).unwrap();
    assert_eq!(
        rotate_pane_layout_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn counter_clockwise_pane_layout_rotation_uses_the_requested_shortcut() {
    assert_eq!(
        ROTATE_PANE_LAYOUT_COUNTER_CLOCKWISE_KEYBINDING,
        platform_keystroke("alt-shift-k")
    );
    let shortcut = gpui::Keystroke::parse(ROTATE_PANE_LAYOUT_COUNTER_CLOCKWISE_KEYBINDING).unwrap();
    assert_eq!(
        rotate_pane_layout_counter_clockwise_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn pane_resize_mode_uses_a_dedicated_ctrl_shift_shortcut() {
    assert_eq!(TOGGLE_PANE_RESIZE_MODE_KEYBINDING, "ctrl-shift-j");
    let shortcut = gpui::Keystroke::parse(TOGGLE_PANE_RESIZE_MODE_KEYBINDING).unwrap();
    assert_eq!(
        pane_resize_mode_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
    for (binding, shortcut) in pane_resize_keybindings()
        .into_iter()
        .zip(["left", "right", "up", "down"])
    {
        let shortcut = gpui::Keystroke::parse(shortcut).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
        assert!(
            binding
                .predicate()
                .expect("pane resize shortcut should be scoped to a terminal")
                .depth_of(&[
                    gpui::KeyContext::parse("Zetta").unwrap(),
                    gpui::KeyContext::parse("PaneResize").unwrap(),
                    gpui::KeyContext::parse("Terminal").unwrap(),
                ])
                .is_some()
        );
    }
}

#[test]
fn pane_focus_shortcuts_use_the_platform_modifier() {
    let shortcuts = ["alt-left", "alt-right", "alt-up", "alt-down"];

    for (binding, shortcut) in focus_pane_keybindings().into_iter().zip(shortcuts) {
        let shortcut = gpui::Keystroke::parse(&platform_keystroke(shortcut)).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}

#[test]
fn alt_shortcuts_use_the_platform_equivalent() {
    for (shortcut, expected) in [
        ("alt-left", "cmd-left"),
        ("alt-shift-l", "cmd-shift-l"),
        ("alt-shift-k", "cmd-shift-k"),
        ("alt-shift-o", "cmd-shift-o"),
        ("alt-shift-e", "cmd-shift-e"),
        ("alt-shift-a", "cmd-shift-a"),
        ("alt-shift-down", "cmd-shift-down"),
        ("alt-shift-up", "cmd-shift-up"),
        ("alt-shift-left", "cmd-shift-left"),
        ("alt-shift-right", "cmd-shift-right"),
        ("alt-shift-f", "cmd-shift-f"),
        ("ctrl-alt-v", "ctrl-cmd-v"),
        ("alt-shift-s", "cmd-shift-s"),
        ("alt-shift-v", "cmd-shift-v"),
        ("alt-shift-r", "cmd-shift-r"),
        ("alt-shift-x", "cmd-shift-x"),
        ("alt-shift-=", "cmd-shift-="),
        ("alt-shift-+", "cmd-shift-+"),
        ("alt-shift--", "cmd-shift--"),
        ("alt-shift-0", "cmd-shift-0"),
        ("ctrl-alt-r", "ctrl-cmd-r"),
        ("alt-space", "alt-space"),
    ] {
        let expected = if cfg!(target_os = "macos") {
            expected
        } else {
            shortcut
        };
        assert_eq!(platform_keystroke(shortcut), expected);
    }
}

#[test]
fn close_pane_uses_the_pane_control_modifiers() {
    assert_eq!(CLOSE_PANE_KEYBINDING, platform_keystroke("alt-shift-x"));
    let shortcut = gpui::Keystroke::parse(CLOSE_PANE_KEYBINDING).unwrap();
    assert_eq!(
        close_pane_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn close_all_windows_uses_the_documented_shortcut() {
    assert_eq!(CLOSE_ALL_WINDOWS_KEYBINDING, "ctrl-shift-x");
    let shortcut = gpui::Keystroke::parse(CLOSE_ALL_WINDOWS_KEYBINDING).unwrap();
    assert_eq!(
        close_all_windows_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn close_window_uses_the_documented_shortcut() {
    assert_eq!(CLOSE_WINDOW_KEYBINDING, "ctrl-shift-q");
    let shortcut = gpui::Keystroke::parse(CLOSE_WINDOW_KEYBINDING).unwrap();
    assert_eq!(
        close_window_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn terminal_clear_uses_ctrl_shift_l() {
    let shortcut = gpui::Keystroke::parse("ctrl-shift-l").unwrap();
    assert_eq!(
        terminal_clear_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
    assert_eq!(terminal_clear_keybinding().action().name(), Clear.name());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_shortcuts_are_additional_application_bindings() {
    let expected = [
        ("cmd-t", NewTab.name()),
        ("cmd-n", NewWindow.name()),
        ("cmd-,", ToggleSettings.name()),
        ("cmd-w", CloseTab.name()),
        ("cmd-q", CloseWindow.name()),
        ("cmd-x", CloseAllWindows.name()),
        ("cmd-h", HideWindow.name()),
        ("cmd-shift-h", HideWindow.name()),
        ("ctrl-shift-h", MinimizeWindow.name()),
        ("cmd-c", CopyAndClearSelection.name()),
        ("cmd-l", Clear.name()),
        ("cmd-v", Paste.name()),
    ];

    for (binding, (shortcut, action)) in macos_keybindings().into_iter().zip(expected) {
        let shortcut = gpui::Keystroke::parse(shortcut).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
        assert_eq!(binding.action().name(), action);
    }

    assert_eq!(CLOSE_WINDOW_KEYBINDING, "ctrl-shift-q");
    assert_eq!(CLOSE_ALL_WINDOWS_KEYBINDING, "ctrl-shift-x");

    let unbinding = macos_terminal_clear_unbinding();
    let shortcut = gpui::Keystroke::parse("cmd-k").unwrap();
    assert_eq!(unbinding.match_keystrokes(&[shortcut]), Some(false));
    assert_eq!(
        unbinding
            .action()
            .as_any()
            .downcast_ref::<Unbind>()
            .expect("Cmd+K should use an unbind marker")
            .0
            .as_ref(),
        "terminal::Clear"
    );
}

#[test]
fn pane_output_uses_the_standard_save_shortcut() {
    assert_eq!(
        SAVE_PANE_OUTPUT_KEYBINDING,
        platform_keystroke("alt-shift-s")
    );
    let shortcut = gpui::Keystroke::parse(SAVE_PANE_OUTPUT_KEYBINDING).unwrap();
    assert_eq!(
        pane_output_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn edit_scrollback_uses_a_pane_scoped_shortcut() {
    assert_eq!(
        EDIT_SCROLLBACK_KEYBINDING,
        platform_keystroke("alt-shift-v")
    );
    let shortcut = gpui::Keystroke::parse(EDIT_SCROLLBACK_KEYBINDING).unwrap();
    assert_eq!(
        edit_scrollback_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
    assert!(
        edit_scrollback_keybinding()
            .predicate()
            .expect("edit scrollback should be scoped to a terminal")
            .to_string()
            .contains("Zetta > Terminal")
    );
}

#[test]
fn select_all_and_reconnect_use_scope_based_shortcuts() {
    assert_eq!(SELECT_ALL_KEYBINDING, platform_keystroke("alt-shift-a"));
    assert_eq!(RECONNECT_SESSION_KEYBINDING, "ctrl-shift-a");
    assert_ne!(SELECT_ALL_KEYBINDING, RECONNECT_SESSION_KEYBINDING);

    let select_all = gpui::Keystroke::parse(SELECT_ALL_KEYBINDING).unwrap();
    assert_eq!(
        select_all_keybinding().match_keystrokes(&[select_all]),
        Some(false)
    );
    let reconnect = gpui::Keystroke::parse(RECONNECT_SESSION_KEYBINDING).unwrap();
    assert_eq!(
        reconnect_session_keybinding().match_keystrokes(&[reconnect]),
        Some(false)
    );
}

#[test]
fn application_menu_shortcut_uses_the_platform_modifier() {
    let binding = application_menu_keybinding();
    assert_eq!(APPLICATION_MENU_KEYBINDING, platform_keystroke("alt-space"));
    let shortcut = gpui::Keystroke::parse(APPLICATION_MENU_KEYBINDING).unwrap();
    let binding = binding.expect("all platforms should bind the application menu");
    assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    assert!(
        binding
            .predicate()
            .expect("application menu shortcut should be scoped to Zetta")
            .depth_of(&[
                gpui::KeyContext::parse("Zetta").unwrap(),
                gpui::KeyContext::parse("Terminal").unwrap(),
            ])
            .is_some()
    );
}

#[test]
fn application_menu_navigation_shortcuts_apply_while_a_menu_is_focused() {
    let shortcuts = ["left", "right"];
    for (binding, shortcut) in application_menu_navigation_keybindings()
        .into_iter()
        .zip(shortcuts)
    {
        assert_eq!(
            binding.match_keystrokes(&[gpui::Keystroke::parse(shortcut).unwrap()]),
            Some(false)
        );
        assert!(
            binding
                .predicate()
                .expect("application menu navigation should be scoped to menus")
                .depth_of(&[
                    gpui::KeyContext::parse("Zetta").unwrap(),
                    gpui::KeyContext::parse("Terminal").unwrap(),
                    gpui::KeyContext::parse("menu").unwrap(),
                ])
                .is_some()
        );
    }
}

#[test]
fn tab_navigation_shortcuts_apply_while_a_menu_is_focused() {
    let shortcuts = ["ctrl-tab", "ctrl-shift-tab", "ctrl-pageup", "ctrl-pagedown"];
    for (binding, shortcut) in tab_menu_navigation_keybindings().into_iter().zip(shortcuts) {
        assert_eq!(
            binding.match_keystrokes(&[gpui::Keystroke::parse(shortcut).unwrap()]),
            Some(false)
        );
        assert!(
            binding
                .predicate()
                .expect("tab navigation should be scoped to menus")
                .depth_of(&[
                    gpui::KeyContext::parse("Zetta").unwrap(),
                    gpui::KeyContext::parse("menu").unwrap(),
                ])
                .is_some()
        );
    }
}

#[test]
fn pane_font_size_shortcuts_use_pane_control_modifiers() {
    let bindings = pane_font_size_keybindings();
    for (binding, shortcut) in
        bindings
            .into_iter()
            .zip(["alt-shift-=", "alt-shift-+", "alt-shift--", "alt-shift-0"])
    {
        let shortcut = gpui::Keystroke::parse(&platform_keystroke(shortcut)).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}

#[test]
fn keymap_storage_preserves_literal_plus_keys() {
    assert_eq!(keymap_keystroke_storage("ctrl-+"), "ctrl-+");
    assert_eq!(keymap_keystroke_storage("alt-shift-+"), "alt-shift-+");
    assert_eq!(keymap_keystroke_storage("ctrl+shift+1"), "ctrl-shift-1");
}

#[cfg(feature = "serial-console")]
#[test]
fn serial_console_avoids_the_linux_unicode_input_shortcut() {
    assert_eq!(SERIAL_CONSOLE_KEYBINDING, "ctrl-shift-s");
    assert_ne!(SERIAL_CONSOLE_KEYBINDING, "ctrl-shift-u");
    let shortcut = gpui::Keystroke::parse(SERIAL_CONSOLE_KEYBINDING).unwrap();
    assert_eq!(
        serial_console_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn auto_background_tab_uses_the_documented_shortcut() {
    assert_eq!(AUTO_BACKGROUND_TAB_KEYBINDING, "ctrl-shift-b");
    let shortcut = gpui::Keystroke::parse(AUTO_BACKGROUND_TAB_KEYBINDING).unwrap();
    assert_eq!(
        auto_background_tab_keybinding().match_keystrokes(std::slice::from_ref(&shortcut)),
        Some(false)
    );
}

#[test]
fn detach_tab_uses_the_tab_scoped_shortcut() {
    assert_eq!(DETACH_TAB_KEYBINDING, "ctrl-shift-d");
    let shortcut = gpui::Keystroke::parse(DETACH_TAB_KEYBINDING).unwrap();
    assert_eq!(
        detach_tab_keybinding().match_keystrokes(&[shortcut]),
        Some(false)
    );
}

#[test]
fn minimized_pane_shortcuts_are_built_in() {
    let bindings = minimized_pane_keybindings();
    for (binding, shortcut) in bindings.into_iter().zip([
        "alt-shift-down",
        "alt-shift-up",
        "alt-shift-left",
        "alt-shift-right",
    ]) {
        let shortcut = gpui::Keystroke::parse(&platform_keystroke(shortcut)).unwrap();
        assert_eq!(binding.match_keystrokes(&[shortcut]), Some(false));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn native_macos_menus_duplicate_the_title_bar_menus() {
    let profiles = vec![
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
        },
        Profile {
            name: "Alternate".to_owned(),
            command: Shell::Program("alternate-shell".to_owned()),
            theme: None,
        },
    ];
    let [application_menu, profile_menu, window_menu] =
        native_macos_menus(&profiles, &HashSet::new(), 1);
    assert_eq!(application_menu.name, "Zetta");
    assert_eq!(profile_menu.name, "Profile");
    assert_eq!(window_menu.name, "Window");

    let application_action_names = application_menu
        .items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { action, .. } => Some(action.name()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        application_action_names,
        [
            NewTab.name(),
            NewWindow.name(),
            ToggleSettings.name(),
            OpenThemes.name(),
            OpenKeymap.name(),
            CloseTab.name(),
            CloseWindow.name(),
            CloseAllWindows.name(),
        ]
    );

    let profile_items = profile_menu
        .items
        .into_iter()
        .map(|item| match item {
            MenuItem::Action {
                name,
                action,
                checked,
                ..
            } => (name.to_string(), action.name(), checked),
            _ => panic!("profile menu contains a non-action item"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        profile_items,
        vec![
            ("System".to_owned(), OpenProfile::name_for_type(), false),
            ("Alternate".to_owned(), OpenProfile::name_for_type(), true)
        ]
    );

    let action_names = window_menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Action { action, .. } => Some(action.name()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(action_names, [MinimizeWindow.name(), ZoomWindow.name()]);
}
