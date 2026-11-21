//! The title bar's profile and application menus, and the shortcut each
//! profile entry advertises.

use super::*;

#[derive(Clone)]
pub(crate) enum ProfileMenuShortcut {
    #[cfg(not(target_os = "macos"))]
    Alias(String),
    Binding(ui::KeyBinding),
}

impl ProfileMenuShortcut {
    pub(crate) fn render(&self) -> AnyElement {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::Alias(label) => Label::new(label.clone())
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
            Self::Binding(binding) => binding.clone().into_any_element(),
        }
    }
}

pub(crate) fn profile_menu_shortcut(
    slot: usize,
    terminal_focus: Option<&gpui::FocusHandle>,
    window: &Window,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> Option<ProfileMenuShortcut> {
    let action = OpenProfile { slot };
    let binding = terminal_focus
        .and_then(|focus| window.highest_precedence_binding_for_action_in(&action, focus))
        .or_else(|| window.highest_precedence_binding_for_action(&action));
    let binding = binding?;

    if slot <= PROFILE_SHORTCUT_KEYS.len() {
        let expected_binding = profile_keybindings(slot, keyboard_mapper)[0].clone();
        if let Some(label) = profile_shortcut_label(slot, &binding, &expected_binding) {
            return Some(profile_shortcut_alias(slot, label));
        }
    }

    Some(ProfileMenuShortcut::Binding(
        ui::KeyBinding::from_keystrokes(binding.keystrokes().to_vec().into(), false),
    ))
}

#[cfg(target_os = "macos")]
pub(crate) fn profile_shortcut_alias_keystroke(slot: usize) -> gpui::KeybindingKeystroke {
    let key = PROFILE_SHORTCUT_KEYS[slot - 1];
    let keystroke = gpui::Keystroke::parse(&format!("ctrl-shift-{key}"))
        .expect("profile shortcut alias must be a valid keystroke");
    gpui::KeybindingKeystroke::from_keystroke(keystroke)
}

#[cfg(target_os = "macos")]
pub(crate) fn profile_shortcut_alias(slot: usize, _label: String) -> ProfileMenuShortcut {
    ProfileMenuShortcut::Binding(ui::KeyBinding::from_keystrokes(
        vec![profile_shortcut_alias_keystroke(slot)].into(),
        false,
    ))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn profile_shortcut_alias(_slot: usize, label: String) -> ProfileMenuShortcut {
    ProfileMenuShortcut::Alias(label)
}

/// Returns focus to the active pane when `menu` is dismissed while focused.
///
/// Registered before `PopoverMenu`'s own dismissal listener so a menu reached
/// through left/right navigation cannot restore focus to the menu it replaced.
pub(crate) fn restore_focus_on_dismiss(
    menu: &Entity<ui::ContextMenu>,
    handle: WeakEntity<Zetta>,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .subscribe(menu, cx, move |menu, _: &DismissEvent, window, cx| {
            if menu.focus_handle(cx).is_focused(window) {
                handle
                    .update(cx, |this, cx| this.focus_active(window, cx))
                    .ok();
            }
        })
        .detach();
}

impl Zetta {
    /// The "Profile" menu: one entry per visible profile, each opening a new tab.
    pub(super) fn render_profile_menu(
        &self,
        show_label: bool,
        handle: &WeakEntity<Self>,
        active_terminal_focus: Option<gpui::FocusHandle>,
        cx: &App,
    ) -> AnyElement {
        let profiles = self.profiles.clone();
        let hidden_profiles = self.effective_config().hidden_profiles.clone();
        let default_profile = self.effective_config().default_profile;
        let menu_handle = handle.clone();
        let dismiss_handle = handle.clone();
        let keyboard_mapper = cx.keyboard_mapper().clone();
        PopoverMenu::new("new-tab-profile-menu")
            .with_handle(self.profile_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new(
                    "new-tab-profile-menu-trigger",
                    if show_label { "Profile" } else { "" },
                )
                .start_icon(Icon::new(IconName::ChevronDown).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("New tab profile"),
                Tooltip::text("New tab profile"),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let profiles = profiles.clone();
                let hidden_profiles = hidden_profiles.clone();
                let handle = menu_handle.clone();
                let dismiss_handle = dismiss_handle.clone();
                let terminal_focus = active_terminal_focus.clone();
                let keyboard_mapper = keyboard_mapper.clone();
                let menu = ui::ContextMenu::build(window, cx, move |mut menu, window, _| {
                    for (visible_index, (index, profile)) in profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, profile)| !profile_is_hidden(profile, &hidden_profiles))
                        .enumerate()
                    {
                        let is_default = index == default_profile;
                        let label_for_row = profile.name.clone();
                        let profile_icon = profile.icon.clone();
                        let shortcut = profile_menu_shortcut(
                            visible_index + 1,
                            terminal_focus.as_ref(),
                            window,
                            keyboard_mapper.as_ref(),
                        );
                        let handle = handle.clone();
                        menu = menu.custom_entry(
                            move |_, _| {
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .when(is_default, |row| {
                                                row.child(
                                                    Icon::new(IconName::Check)
                                                        .size(IconSize::Small)
                                                        .color(Color::Accent),
                                                )
                                            })
                                            .when(!is_default, |row| row.child(div().w_4()))
                                            .child(profile_icon.render(IconSize::Small))
                                            .child(Label::new(label_for_row.clone()).color(
                                                if is_default {
                                                    Color::Accent
                                                } else {
                                                    Color::Default
                                                },
                                            )),
                                    )
                                    .when_some(shortcut.clone(), |row, shortcut| {
                                        row.child(shortcut.render())
                                    })
                                    .into_any_element()
                            },
                            move |window, cx| {
                                handle
                                    .update(cx, |this, cx| {
                                        let profile = this.profiles[index].clone();
                                        this.open_tab_with_profile(profile, window, cx);
                                    })
                                    .ok();
                            },
                        );
                    }
                    menu
                });
                restore_focus_on_dismiss(&menu, dismiss_handle, window, cx);
                Some(menu)
            })
            .into_any_element()
    }

    /// The "Menu" button's application menu.
    pub(super) fn render_application_menu(
        &self,
        show_label: bool,
        handle: &WeakEntity<Self>,
        // The popover receives focus while it is open. Retain the active
        // terminal's context so actions continue to resolve their shortcuts
        // when the user cycles here from the Profile menu.
        action_context: Option<gpui::FocusHandle>,
    ) -> AnyElement {
        let dismiss_handle = handle.clone();
        PopoverMenu::new("application-menu")
            .with_handle(self.application_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new(
                    "application-menu-trigger",
                    if show_label { "Menu" } else { "" },
                )
                .start_icon(Icon::new(IconName::Menu).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("Application menu"),
                Tooltip::for_action_title("Open application menu", &OpenApplicationMenu),
            )
            .anchor(Anchor::TopLeft)
            .menu(move |window, cx| {
                let dismiss_handle = dismiss_handle.clone();
                let action_context = action_context.clone();
                let is_fullscreen = window.is_fullscreen();
                let is_default_terminal = crate::default_terminal::is_default_terminal();
                let menu = ui::ContextMenu::build(window, cx, move |menu, _, _| {
                    let menu =
                        menu.when_some(action_context.clone(), |menu, focus| menu.context(focus));
                    menu.action("New Tab", Box::new(NewTab))
                        .action("New Window", Box::new(NewWindow))
                        .separator()
                        .action("Command Palette", Box::new(ToggleCommandPalette))
                        .action_checked(
                            "Set as Default Terminal",
                            Box::new(SetDefaultTerminal),
                            is_default_terminal,
                        )
                        .action("Open Settings", Box::new(ToggleSettings))
                        .action("Open Themes", Box::new(OpenThemes))
                        .action("Open Keymap", Box::new(OpenKeymap))
                        .action("Open Templates", Box::new(OpenTemplates))
                        .action("Open Projects", Box::new(OpenProjects))
                        .when(cfg!(feature = "zmux"), |menu| {
                            menu.action("Open Remote Session", Box::new(OpenRemoteSession))
                        })
                        .separator()
                        .action_checked(
                            "Toggle Fullscreen",
                            Box::new(ToggleFullscreen),
                            is_fullscreen,
                        )
                        .action("Close Tab", Box::new(CloseTab))
                        .action("Close Window", Box::new(CloseWindow))
                        .action("Close All Windows", Box::new(CloseAllWindows))
                });
                restore_focus_on_dismiss(&menu, dismiss_handle, window, cx);
                Some(menu)
            })
            .into_any_element()
    }
}
