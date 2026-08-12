use super::*;

#[cfg(target_os = "macos")]
pub(crate) mod macos_menu_keybindings {
    pub(crate) const MACOS_NEW_TAB_KEYBINDING: &str = "cmd-t";
    pub(crate) const MACOS_NEW_WINDOW_KEYBINDING: &str = "cmd-n";
    pub(crate) const MACOS_SETTINGS_KEYBINDING: &str = "cmd-,";
    pub(crate) const MACOS_CLOSE_TAB_KEYBINDING: &str = "cmd-w";
    pub(crate) const MACOS_CLOSE_WINDOW_KEYBINDING: &str = "cmd-q";
    pub(crate) const MACOS_CLOSE_ALL_WINDOWS_KEYBINDING: &str = "cmd-x";
    pub(crate) const MACOS_COPY_KEYBINDING: &str = "cmd-c";
    pub(crate) const MACOS_CLEAR_KEYBINDING: &str = "cmd-l";
    pub(crate) const MACOS_PASTE_KEYBINDING: &str = "cmd-v";
}

#[cfg(target_os = "macos")]
use macos_menu_keybindings::*;

pub(crate) fn profile_keybindings(
    slot: usize,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> [KeyBinding; 1] {
    let action = OpenProfile { slot };
    let context = Some(
        KeyBindingContextPredicate::parse("Zetta > Terminal")
            .expect("built-in keybinding context must be valid")
            .into(),
    );
    let binding = |keystroke: &str, action: OpenProfile| {
        KeyBinding::load(
            keystroke,
            Box::new(action),
            context.clone(),
            true,
            None,
            keyboard_mapper,
        )
        .expect("built-in profile keystroke must be valid")
    };
    [binding(
        &format!("ctrl-{}", PROFILE_SHORTCUT_SYMBOLS[slot - 1]),
        action,
    )]
}

pub(crate) const PROFILE_SHORTCUT_SYMBOLS: [&str; 10] =
    ["!", "@", "#", "$", "%", "^", "&", "*", "(", ")"];

pub(crate) const PROFILE_SHORTCUT_KEYS: [&str; 10] =
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];

/// Converts GPUI's normalized number-row key names to the user-facing
/// physical-key aliases used by menus and the keymap editor.
#[allow(dead_code)]
pub(crate) fn keymap_keystroke_alias(keystroke: &str) -> Option<String> {
    let keystroke = keystroke.trim().to_ascii_lowercase();
    let key_index = keystroke
        .strip_prefix("ctrl-shift-")
        .and_then(|key| {
            PROFILE_SHORTCUT_KEYS
                .iter()
                .position(|candidate| *candidate == key)
        })
        .or_else(|| {
            keystroke.strip_prefix("ctrl-").and_then(|symbol| {
                PROFILE_SHORTCUT_SYMBOLS
                    .iter()
                    .position(|candidate| *candidate == symbol)
            })
        })?;
    Some(format!("Ctrl+Shift+{}", PROFILE_SHORTCUT_KEYS[key_index]))
}

/// Converts a user-facing number-row alias back to GPUI's normalized keymap
/// spelling for storage and keymap loading.
pub(crate) fn keymap_keystroke_storage(keystroke: &str) -> String {
    let normalized = keystroke.trim().to_ascii_lowercase();
    // A trailing `+` is the literal plus key in GPUI's `ctrl-+` spelling,
    // not a separator in the alternate `ctrl+shift+1` spelling. Replacing it
    // with `-` would make `ctrl-+` collide with `ctrl--` in default lookups.
    let normalized = if normalized.ends_with('+') {
        normalized
    } else {
        normalized.replace('+', "-")
    };
    let key_index = normalized
        .strip_prefix("ctrl+shift+")
        .or_else(|| normalized.strip_prefix("ctrl-shift-"))
        .and_then(|key| {
            PROFILE_SHORTCUT_KEYS
                .iter()
                .position(|candidate| *candidate == key)
        })
        .or_else(|| {
            normalized.strip_prefix("ctrl-").and_then(|symbol| {
                PROFILE_SHORTCUT_SYMBOLS
                    .iter()
                    .position(|candidate| *candidate == symbol)
            })
        });
    if let Some(key_index) = key_index {
        return format!("ctrl-shift-{}", PROFILE_SHORTCUT_KEYS[key_index]);
    }
    normalized
}

pub(crate) fn keymap_keystroke_display(keystroke: &str) -> String {
    keymap_keystroke_storage(keystroke)
}

/// Returns the user-facing alias for a built-in number-row profile shortcut.
///
/// GPUI represents a shifted number key by its produced symbol (for example,
/// `ctrl-!`), while users type the more familiar `Ctrl+Shift+1` chord. A
/// remapped binding has no alias, so callers display the effective binding.
pub(crate) fn profile_shortcut_label(
    slot: usize,
    binding: &KeyBinding,
    expected_binding: &KeyBinding,
) -> Option<String> {
    let key = PROFILE_SHORTCUT_KEYS.get(slot.checked_sub(1)?)?;
    (binding.keystrokes() == expected_binding.keystrokes()).then(|| format!("Ctrl+Shift+{key}"))
}

pub(crate) fn pane_template_keybindings() -> [KeyBinding; 2] {
    [
        platform_keybinding(
            "alt-shift-o",
            ApplyPaneSplitTemplate {
                name: "three-right".to_owned(),
            },
            Some("Zetta > Terminal"),
        ),
        platform_keybinding(
            "alt-shift-e",
            ApplyPaneSplitTemplate {
                name: "quarters".to_owned(),
            },
            Some("Zetta > Terminal"),
        ),
    ]
}

pub(crate) fn pane_font_size_keybindings() -> [KeyBinding; 4] {
    [
        platform_keybinding(
            "alt-shift-=",
            IncreasePaneFontSize,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding(
            "alt-shift-+",
            IncreasePaneFontSize,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding(
            "alt-shift--",
            DecreasePaneFontSize,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding("alt-shift-0", ResetPaneFontSize, Some("Zetta > Terminal")),
    ]
}

#[cfg(target_os = "macos")]
fn macos_keybindings() -> [KeyBinding; 12] {
    [
        // Keep application bindings unscoped so the native application menu
        // can resolve their key equivalents, including while a Zetta overlay
        // is focused. The existing Ctrl bindings remain available too.
        KeyBinding::new(MACOS_NEW_TAB_KEYBINDING, NewTab, None),
        KeyBinding::new(MACOS_NEW_WINDOW_KEYBINDING, NewWindow, None),
        KeyBinding::new(MACOS_SETTINGS_KEYBINDING, ToggleSettings, None),
        KeyBinding::new(MACOS_CLOSE_TAB_KEYBINDING, CloseTab, None),
        KeyBinding::new(MACOS_CLOSE_WINDOW_KEYBINDING, CloseWindow, None),
        KeyBinding::new(MACOS_CLOSE_ALL_WINDOWS_KEYBINDING, CloseAllWindows, None),
        KeyBinding::new("cmd-h", HideWindow, None),
        KeyBinding::new(HIDE_WINDOW_KEYBINDING, HideWindow, None),
        KeyBinding::new("ctrl-shift-h", MinimizeWindow, None),
        // Terminal actions stay scoped so they do not override unrelated
        // macOS editor bindings outside a terminal pane.
        KeyBinding::new(
            MACOS_COPY_KEYBINDING,
            CopyAndClearSelection,
            Some("Zetta > Terminal && selection"),
        ),
        KeyBinding::new(MACOS_CLEAR_KEYBINDING, Clear, Some("Zetta > Terminal")),
        KeyBinding::new(MACOS_PASTE_KEYBINDING, Paste, Some("Zetta > Terminal")),
    ]
}

pub(crate) fn terminal_clear_keybinding() -> KeyBinding {
    KeyBinding::new("ctrl-shift-l", Clear, Some("Zetta > Terminal"))
}

#[cfg(target_os = "macos")]
fn macos_terminal_clear_unbinding() -> KeyBinding {
    KeyBinding::new("cmd-k", Unbind("terminal::Clear".into()), None)
}

fn platform_keystroke(keystroke: &str) -> String {
    if cfg!(target_os = "macos") && keystroke != APPLICATION_MENU_KEYBINDING {
        keystroke.replace("alt-", "cmd-")
    } else {
        keystroke.to_owned()
    }
}

fn platform_keybinding<A: Action>(keystroke: &str, action: A, context: Option<&str>) -> KeyBinding {
    let keystroke = platform_keystroke(keystroke);
    KeyBinding::new(&keystroke, action, context)
}

pub(crate) const RENAME_TAB_KEYBINDING: &str = "ctrl-shift-r";

pub(crate) const CHANGE_TAB_ICON_KEYBINDING: &str = "ctrl-shift-y";

pub(crate) const RELOAD_CONFIGURATION_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "ctrl-cmd-r"
} else {
    "ctrl-alt-r"
};

pub(crate) const RENAME_PANE_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-r"
} else {
    "alt-shift-r"
};

pub(crate) const HIDE_WINDOW_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-h"
} else {
    "ctrl-shift-h"
};

pub(crate) const CLOSE_PANE_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-x"
} else {
    "alt-shift-x"
};

pub(crate) const SAVE_PANE_OUTPUT_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-s"
} else {
    "alt-shift-s"
};

pub(crate) const SAVE_SETTINGS_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-s"
} else {
    "ctrl-s"
};

pub(crate) const EDIT_SCROLLBACK_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-v"
} else {
    "alt-shift-v"
};

pub(crate) const SELECT_ALL_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-a"
} else {
    "alt-shift-a"
};

pub(crate) const RECONNECT_SESSION_KEYBINDING: &str = "ctrl-shift-a";

pub(crate) const DETACH_TAB_KEYBINDING: &str = "ctrl-shift-d";

pub(crate) const CLOSE_WINDOW_KEYBINDING: &str = "ctrl-shift-q";

pub(crate) const CLOSE_ALL_WINDOWS_KEYBINDING: &str = "ctrl-shift-x";

#[cfg(feature = "serial-console")]
pub(crate) const SERIAL_CONSOLE_KEYBINDING: &str = "ctrl-shift-s";

pub(crate) const AUTO_BACKGROUND_TAB_KEYBINDING: &str = "ctrl-shift-b";

pub(crate) const ROTATE_PANE_LAYOUT_KEYBINDING: &str = if cfg!(target_os = "macos") {
    "cmd-shift-l"
} else {
    "alt-shift-l"
};

pub(crate) const ROTATE_PANE_LAYOUT_COUNTER_CLOCKWISE_KEYBINDING: &str =
    if cfg!(target_os = "macos") {
        "cmd-shift-k"
    } else {
        "alt-shift-k"
    };

pub(crate) const TOGGLE_PANE_RESIZE_MODE_KEYBINDING: &str = "ctrl-shift-j";

pub(crate) const TOGGLE_PANE_MOVE_MODE_KEYBINDING: &str = "alt-shift-m";

pub(crate) const TOGGLE_TAB_MOVE_MODE_KEYBINDING: &str = "ctrl-shift-g";

pub(crate) const APPLICATION_MENU_KEYBINDING: &str = "alt-space";

pub(crate) fn pane_output_keybinding() -> KeyBinding {
    KeyBinding::new(
        SAVE_PANE_OUTPUT_KEYBINDING,
        SavePaneOutput,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn save_settings_keybinding() -> KeyBinding {
    KeyBinding::new(
        SAVE_SETTINGS_KEYBINDING,
        SaveSettings,
        Some("Zetta > Settings"),
    )
}

pub(crate) fn edit_scrollback_keybinding() -> KeyBinding {
    KeyBinding::new(
        EDIT_SCROLLBACK_KEYBINDING,
        EditScrollback,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn select_all_keybinding() -> KeyBinding {
    KeyBinding::new(SELECT_ALL_KEYBINDING, SelectAll, Some("Zetta > Terminal"))
}

pub(crate) fn reconnect_session_keybinding() -> KeyBinding {
    KeyBinding::new(
        RECONNECT_SESSION_KEYBINDING,
        ReconnectSession,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn application_menu_keybinding() -> Option<KeyBinding> {
    Some(KeyBinding::new(
        APPLICATION_MENU_KEYBINDING,
        OpenApplicationMenu,
        // The menu is an application-level control. Binding it at the Zetta
        // context keeps it available even when the focused terminal view does
        // not contribute its `Terminal` key context.
        Some("Zetta"),
    ))
}

pub(crate) fn application_menu_navigation_keybindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("left", ActivateApplicationMenuLeft, Some("Zetta > menu")),
        KeyBinding::new("right", ActivateApplicationMenuRight, Some("Zetta > menu")),
    ]
}

pub(crate) fn tab_menu_navigation_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new("ctrl-tab", NextTab, Some("Zetta > menu")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Zetta > menu")),
        KeyBinding::new("ctrl-pageup", NextTab, Some("Zetta > menu")),
        KeyBinding::new("ctrl-pagedown", PreviousTab, Some("Zetta > menu")),
    ]
}

pub(crate) fn detach_tab_keybinding() -> KeyBinding {
    KeyBinding::new(DETACH_TAB_KEYBINDING, DetachTab, Some("Zetta > Terminal"))
}

pub(crate) fn close_window_keybinding() -> KeyBinding {
    KeyBinding::new(
        CLOSE_WINDOW_KEYBINDING,
        CloseWindow,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn close_all_windows_keybinding() -> KeyBinding {
    KeyBinding::new(
        CLOSE_ALL_WINDOWS_KEYBINDING,
        CloseAllWindows,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn close_pane_keybinding() -> KeyBinding {
    KeyBinding::new(CLOSE_PANE_KEYBINDING, ClosePane, Some("Zetta > Terminal"))
}

#[cfg(feature = "serial-console")]
pub(crate) fn serial_console_keybinding() -> KeyBinding {
    KeyBinding::new(
        SERIAL_CONSOLE_KEYBINDING,
        ToggleSerialConsole,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn auto_background_tab_keybinding() -> KeyBinding {
    KeyBinding::new(
        AUTO_BACKGROUND_TAB_KEYBINDING,
        ToggleAutoBackgroundTab,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn rotate_pane_layout_keybinding() -> KeyBinding {
    KeyBinding::new(
        ROTATE_PANE_LAYOUT_KEYBINDING,
        RotatePaneLayout,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn rotate_pane_layout_counter_clockwise_keybinding() -> KeyBinding {
    KeyBinding::new(
        ROTATE_PANE_LAYOUT_COUNTER_CLOCKWISE_KEYBINDING,
        RotatePaneLayoutCounterClockwise,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn pane_resize_mode_keybinding() -> KeyBinding {
    KeyBinding::new(
        TOGGLE_PANE_RESIZE_MODE_KEYBINDING,
        TogglePaneResizeMode,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn pane_resize_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new(
            "left",
            ResizePaneLeft,
            Some("Zetta > PaneResize > Terminal"),
        ),
        KeyBinding::new(
            "right",
            ResizePaneRight,
            Some("Zetta > PaneResize > Terminal"),
        ),
        KeyBinding::new("up", ResizePaneUp, Some("Zetta > PaneResize > Terminal")),
        KeyBinding::new(
            "down",
            ResizePaneDown,
            Some("Zetta > PaneResize > Terminal"),
        ),
    ]
}

pub(crate) fn pane_move_mode_keybinding() -> KeyBinding {
    platform_keybinding(
        TOGGLE_PANE_MOVE_MODE_KEYBINDING,
        TogglePaneMoveMode,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn tab_move_mode_keybinding() -> KeyBinding {
    KeyBinding::new(
        TOGGLE_TAB_MOVE_MODE_KEYBINDING,
        ToggleTabMoveMode,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn pane_move_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new("left", MovePaneLeft, Some("Zetta > PaneMove > Terminal")),
        KeyBinding::new("right", MovePaneRight, Some("Zetta > PaneMove > Terminal")),
        KeyBinding::new("up", MovePaneUp, Some("Zetta > PaneMove > Terminal")),
        KeyBinding::new("down", MovePaneDown, Some("Zetta > PaneMove > Terminal")),
    ]
}

pub(crate) fn tab_move_keybindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("left", MoveTabLeft, Some("Zetta > TabMove > Terminal")),
        KeyBinding::new("right", MoveTabRight, Some("Zetta > TabMove > Terminal")),
    ]
}

pub(crate) fn focus_pane_keybindings() -> [KeyBinding; 4] {
    let shortcuts = ["alt-left", "alt-right", "alt-up", "alt-down"];

    [
        platform_keybinding(shortcuts[0], FocusPaneLeft, Some("Zetta > Terminal")),
        platform_keybinding(shortcuts[1], FocusPaneRight, Some("Zetta > Terminal")),
        platform_keybinding(shortcuts[2], FocusPaneUp, Some("Zetta > Terminal")),
        platform_keybinding(shortcuts[3], FocusPaneDown, Some("Zetta > Terminal")),
    ]
}

pub(crate) fn minimized_pane_keybindings() -> [KeyBinding; 4] {
    [
        platform_keybinding("alt-shift-down", MinimizePane, Some("Zetta > Terminal")),
        platform_keybinding(
            "alt-shift-up",
            RestoreMinimizedPane,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding(
            "alt-shift-left",
            SelectPreviousMinimizedPane,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding(
            "alt-shift-right",
            SelectNextMinimizedPane,
            Some("Zetta > Terminal"),
        ),
    ]
}

pub(crate) fn stacked_pane_keybindings() -> [KeyBinding; 5] {
    [
        platform_keybinding(
            "alt-shift-n",
            ToggleStackedCommand,
            Some("Zetta > Terminal"),
        ),
        // GPUI's Linux keyboard event uses the produced symbol for shifted
        // punctuation. On a US layout, Alt+Shift+[ therefore arrives as
        // Alt+{ (and the closing bracket as Alt+}). Keep the documented
        // physical-key spellings below as well so the bindings remain
        // portable to layouts/platforms that preserve the bracket key.
        platform_keybinding("alt-{", SelectPreviousStackedPane, Some("Zetta > Terminal")),
        platform_keybinding("alt-}", SelectNextStackedPane, Some("Zetta > Terminal")),
        platform_keybinding(
            "alt-shift-[",
            SelectPreviousStackedPane,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding(
            "alt-shift-]",
            SelectNextStackedPane,
            Some("Zetta > Terminal"),
        ),
    ]
}

fn default_keybindings(
    profile_count: usize,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new("ctrl-shift-t", NewTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-n", NewWindow, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Zetta > Terminal")),
        close_window_keybinding(),
        close_all_windows_keybinding(),
        detach_tab_keybinding(),
        reconnect_session_keybinding(),
        auto_background_tab_keybinding(),
        close_pane_keybinding(),
        KeyBinding::new(
            "ctrl-shift-o",
            SplitHorizontalDown,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-shift-e", SplitVerticalRight, Some("Zetta > Terminal")),
        rotate_pane_layout_keybinding(),
        rotate_pane_layout_counter_clockwise_keybinding(),
        pane_resize_mode_keybinding(),
        pane_move_mode_keybinding(),
        tab_move_mode_keybinding(),
        select_all_keybinding(),
        edit_scrollback_keybinding(),
        KeyBinding::new(
            "ctrl-shift-backspace",
            ClearClipboard,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("shift-escape", ToggleMaximizePane, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-i",
            ToggleBroadcastInput,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-shift-m", ToggleMultiCommand, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-tab", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pageup", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pagedown", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-c",
            CopyAndClearSelection,
            Some("Zetta > Terminal && selection"),
        ),
        KeyBinding::new(
            "ctrl-insert",
            CopyAndClearSelection,
            Some("Zetta > Terminal && selection"),
        ),
        terminal_clear_keybinding(),
        KeyBinding::new("ctrl-v", Paste, Some("Zetta > Terminal")),
        KeyBinding::new("shift-insert", Paste, Some("Zetta > Terminal")),
        platform_keybinding("alt-shift-f", SearchScrollback, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-f",
            SearchTabScrollback,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "enter",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-enter",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "f3",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-f3",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "escape",
            DismissSearch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "ctrl-a",
            SelectAllSearchText,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        platform_keybinding("ctrl-alt-v", PasteTrimmed, Some("Zetta > Terminal")),
        pane_output_keybinding(),
        save_settings_keybinding(),
        KeyBinding::new(
            "ctrl-shift-p",
            ToggleCommandPalette,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-,", ToggleSettings, Some("Zetta > Terminal")),
        KeyBinding::new(RENAME_TAB_KEYBINDING, RenameTab, Some("Zetta > Terminal")),
        KeyBinding::new(
            CHANGE_TAB_ICON_KEYBINDING,
            ChangeTabIcon,
            Some("Zetta > Terminal"),
        ),
        platform_keybinding("alt-shift-t", ChangePaneTheme, Some("Zetta > Terminal")),
        KeyBinding::new(RENAME_PANE_KEYBINDING, RenamePane, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-=", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-+", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl--", DecreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-0", ResetTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new(
            RELOAD_CONFIGURATION_KEYBINDING,
            ReloadConfiguration,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "ctrl-shift-f12",
            TogglePerformanceOverlay,
            Some("Zetta > Terminal"),
        ),
        // Override Zed's inherited `pane::CloseActiveItem` binding in terminal focus.
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Terminal")),
    ];
    #[cfg(not(target_os = "macos"))]
    bindings.push(KeyBinding::new(
        HIDE_WINDOW_KEYBINDING,
        HideWindow,
        Some("Zetta"),
    ));
    #[cfg(feature = "serial-console")]
    bindings.push(serial_console_keybinding());
    bindings.extend(application_menu_keybinding());
    bindings.extend(application_menu_navigation_keybindings());
    bindings.extend(tab_menu_navigation_keybindings());
    bindings.extend(focus_pane_keybindings());
    bindings.extend(minimized_pane_keybindings());
    bindings.extend(stacked_pane_keybindings());
    bindings.extend(pane_resize_keybindings());
    bindings.extend(pane_move_keybindings());
    bindings.extend(tab_move_keybindings());
    bindings.extend(pane_template_keybindings());
    bindings.extend(pane_font_size_keybindings());
    #[cfg(target_os = "macos")]
    bindings.push(macos_terminal_clear_unbinding());
    #[cfg(target_os = "macos")]
    bindings.extend(macos_keybindings());
    bindings.extend(
        (1..=profile_count.min(PROFILE_SHORTCUT_SYMBOLS.len()))
            .flat_map(|slot| profile_keybindings(slot, keyboard_mapper)),
    );

    bindings
}

type DefaultBindingKey = (String, Option<String>);

fn index_default_bindings(bindings: &[KeyBinding]) -> HashMap<DefaultBindingKey, Vec<String>> {
    let mut indexed: HashMap<DefaultBindingKey, Vec<String>> = HashMap::new();
    for binding in bindings {
        let action_name = binding.action().name().to_string();
        let context = binding.predicate().map(|predicate| predicate.to_string());
        if let Some(keystroke) = binding.keystrokes().first() {
            indexed
                .entry((action_name, context))
                .or_default()
                .push(keystroke.to_string());
        }
    }
    indexed
}

pub(crate) fn load_keybindings(path: &PathBuf, profile_count: usize, cx: &mut App) {
    cx.clear_key_bindings();
    match KeymapFile::load_asset_allow_partial_failure(settings::DEFAULT_KEYMAP_PATH, cx) {
        Ok(bindings) => cx.bind_keys(bindings),
        Err(error) => eprintln!("Could not load the default terminal keymap: {error:#}"),
    }

    // Build default bindings and collect a map of (action_name, context) -> keystrokes
    // for rebinding detection
    let keyboard_mapper = cx.keyboard_mapper().clone();
    let bindings = default_keybindings(profile_count, keyboard_mapper.as_ref());
    let default_bindings_map = index_default_bindings(&bindings);

    cx.bind_keys(bindings);

    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let content = normalize_keymap_key_names(&content);

    // Parse user keymap to detect rebindings and create unbind key bindings
    let user_keymap = match KeymapFile::parse(&content) {
        Ok(keymap) => keymap,
        Err(error) => {
            eprintln!("Could not parse {}: {error:#}", path.display());
            return;
        }
    };

    // Create unbind key bindings for rebindings
    let mut unbind_keys = Vec::new();
    for section in user_keymap.sections() {
        let context_str = if section.context.is_empty() {
            None
        } else {
            Some(section.context.clone())
        };
        let context_predicate = context_str
            .as_ref()
            .and_then(|s| KeyBindingContextPredicate::parse(s).ok().map(Rc::new));
        for (_keystrokes, action) in section.bindings() {
            if let Ok(Some((action_name, _))) = KeymapFile::parse_action(action)
                && let Some(default_keystrokes) =
                    default_bindings_map.get(&(action_name.clone(), context_str.clone()))
            {
                for default_keystroke in default_keystrokes {
                    // Create unbind key bindings for every default keystroke
                    // belonging to this action and context.
                    let unbind_action = Unbind(action_name.clone().into());
                    if let Ok(key_binding) = KeyBinding::load(
                        default_keystroke,
                        Box::new(unbind_action),
                        context_predicate.clone(),
                        false,
                        None,
                        cx.keyboard_mapper().as_ref(),
                    ) {
                        unbind_keys.push(key_binding);
                    }
                }
            }
        }
    }

    // Bind unbind keys first to remove default bindings
    if !unbind_keys.is_empty() {
        cx.bind_keys(unbind_keys);
    }

    // Now load user keymap normally
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { key_bindings } => cx.bind_keys(key_bindings),
        KeymapFileLoadResult::SomeFailedToLoad {
            key_bindings,
            error_message,
        } => {
            eprintln!(
                "Some key bindings in {} were ignored: {error_message}",
                path.display()
            );
            cx.bind_keys(key_bindings);
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            eprintln!("Could not load {}: {error:#}", path.display());
        }
    }
}

#[cfg(target_os = "macos")]
fn native_macos_menus(
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
    default_profile: usize,
) -> [Menu; 3] {
    let profile_menu = Menu::new("Profile").items(
        profiles
            .iter()
            .enumerate()
            .filter(|(_, profile)| !profile_is_hidden(profile, hidden_profiles))
            .enumerate()
            .map(|(visible_index, (index, profile))| {
                MenuItem::action(
                    profile.name.clone(),
                    OpenProfile {
                        slot: visible_index + 1,
                    },
                )
                .checked(index == default_profile)
            }),
    );

    // Keep Window separate from the first, application-owned menu and preserve
    // the standard Minimize/Zoom/separator shape that AppKit augments with its
    // native Move & Resize commands.
    [
        Menu::new("Zetta").items([
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("New Window", NewWindow),
            MenuItem::separator(),
            MenuItem::action("Command Palette", ToggleCommandPalette),
            MenuItem::action("Open Settings", ToggleSettings),
            MenuItem::action("Open Themes", OpenThemes),
            MenuItem::action("Open Keymap", OpenKeymap),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::action("Close All Windows", CloseAllWindows),
        ]),
        profile_menu,
        Menu::new("Window").items([
            MenuItem::action("Minimize", MinimizeWindow),
            MenuItem::action("Zoom", ZoomWindow),
            MenuItem::separator(),
        ]),
    ]
}

#[cfg(target_os = "macos")]
pub(crate) fn update_native_macos_menus(
    cx: &mut App,
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
    default_profile: usize,
) {
    cx.set_menus(native_macos_menus(
        profiles,
        hidden_profiles,
        default_profile,
    ));
}

#[cfg(target_os = "macos")]
pub(crate) fn install_native_macos_menus(
    cx: &mut App,
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
    default_profile: usize,
) {
    update_native_macos_menus(cx, profiles, hidden_profiles, default_profile);
    install_native_macos_window_menu_key_equivalents();
}

#[cfg(target_os = "macos")]
fn install_native_macos_window_menu_key_equivalents() {
    // GPUI's content view sees key equivalents before AppKit searches the main
    // menu. A terminal consumes Control+Function combinations as terminal
    // input, so macOS never gets to invoke the tiling items that it injects
    // into the registered Window menu. Give that menu first refusal; exact
    // modifier matching in NSMenu keeps ordinary terminal shortcuts intact.
    unsafe {
        let main_thread =
            MainThreadMarker::new().expect("menu monitor must be installed on AppKit");
        let application = NSApplication::sharedApplication(main_thread);
        let handler = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
            let event_ref = event.as_ref();
            let modifiers = event_ref.modifierFlags();
            if !modifiers.contains(NSEventModifierFlags::Control)
                || !modifiers.contains(NSEventModifierFlags::Function)
            {
                return event.as_ptr();
            }
            let handled = application.mainMenu().is_some_and(|menu| {
                // `performKeyEquivalent` performs the menu validation it needs.
                // Calling `update` first re-enters AppKit while it is dispatching
                // the key event and can race the input method's deactivation.
                menu.performKeyEquivalent(event_ref)
            });
            if handled {
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });

        let monitor =
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
                .expect("failed to install native macOS menu key-equivalent monitor");
        // The monitor is intentionally process-scoped and removed when AppKit exits.
        std::mem::forget(monitor);
    }
}

#[cfg(test)]
#[path = "../tests/startup/keybindings.rs"]
mod tests;
