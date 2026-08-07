use super::*;

impl Zetta {
    pub(crate) fn change_pane_theme(
        &mut self,
        _: &ChangePaneTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pane_theme_picker(window, cx);
    }

    pub(crate) fn open_pane_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Falls back to the global theme when the pane has neither an
        // override nor a profile theme, so the tick mark always lands on
        // whichever theme is actually showing, not just an active override.
        let current_theme_name = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| Some(cx.theme().clone()))
            .map(|theme| theme.name.to_string());
        let mut theme_names = ThemeRegistry::global(cx)
            .list()
            .into_iter()
            .map(|theme| theme.name.to_string())
            .collect::<Vec<_>>();
        theme_names.sort();
        theme_names.dedup();
        let reset_command = PaletteCommand {
            name: "Reset to profile default".to_owned(),
            shortcut: None,
            action: Box::new(ResetPaneTheme),
        };
        let theme_commands = theme_names
            .into_iter()
            .map(|name| PaletteCommand {
                name: name.clone(),
                shortcut: None,
                action: Box::new(ApplyPaneTheme { name }),
            })
            .collect();
        let mut picker = CommandPalette::with_pinned_first(reset_command, theme_commands);
        if let Some(index) = current_theme_name.as_deref().and_then(|current| {
            picker
                .commands
                .iter()
                .position(|command| command.name == current)
        }) {
            picker.selected = index;
        }
        picker.scroll_to_selected();
        self.theme_picker_current = current_theme_name;
        self.theme_picker = Some(picker);
        self.theme_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_pane_theme_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.theme_picker = None;
        self.theme_picker_current = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn run_pane_theme_picker_command(
        &mut self,
        command_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = self
            .theme_picker
            .as_ref()
            .and_then(|palette| palette.commands.get(command_index))
            .map(|command| command.action.boxed_clone());
        self.theme_picker = None;
        self.focus_active(window, cx);
        if let Some(action) = action {
            window.dispatch_action(action, cx);
        }
        cx.notify();
    }

    pub(crate) fn theme_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.theme_picker.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_pane_theme_picker(window, cx),
            "up" => {
                picker.selected = picker.selected.saturating_sub(1);
                picker.scroll_to_selected();
                cx.notify();
            }
            "down" => {
                picker.selected =
                    (picker.selected + 1).min(picker.matches().len().saturating_sub(1));
                picker.scroll_to_selected();
                cx.notify();
            }
            "enter" => {
                let command = picker.matches().get(picker.selected).copied();
                if let Some(command) = command {
                    self.run_pane_theme_picker_command(command, window, cx);
                }
            }
            "backspace" => {
                if picker.select_all {
                    picker.query.clear();
                    picker.cursor = 0;
                    picker.refresh_matches();
                } else if picker.cursor > 0 {
                    let previous = previous_char_boundary(&picker.query, picker.cursor);
                    picker.query.replace_range(previous..picker.cursor, "");
                    picker.cursor = previous;
                    picker.refresh_matches();
                }
                picker.select_all = false;
                picker.selected = 0;
                picker.scroll_to_selected();
                cx.notify();
            }
            "delete" => {
                if picker.select_all {
                    picker.query.clear();
                    picker.cursor = 0;
                    picker.refresh_matches();
                } else if picker.cursor < picker.query.len() {
                    let next = next_char_boundary(&picker.query, picker.cursor);
                    picker.query.replace_range(picker.cursor..next, "");
                    picker.refresh_matches();
                }
                picker.select_all = false;
                picker.selected = 0;
                picker.scroll_to_selected();
                cx.notify();
            }
            "left" => {
                picker.cursor = if picker.select_all {
                    0
                } else {
                    previous_char_boundary(&picker.query, picker.cursor)
                };
                picker.select_all = false;
                cx.notify();
            }
            "right" => {
                picker.cursor = if picker.select_all {
                    picker.query.len()
                } else {
                    next_char_boundary(&picker.query, picker.cursor)
                };
                picker.select_all = false;
                cx.notify();
            }
            "home" => {
                picker.cursor = 0;
                picker.select_all = false;
                cx.notify();
            }
            "end" => {
                picker.cursor = picker.query.len();
                picker.select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                picker.select_all = !picker.query.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if picker.select_all {
                        picker.query.clear();
                        picker.cursor = 0;
                        picker.select_all = false;
                    }
                    picker.query.insert_str(picker.cursor, text);
                    picker.cursor += text.len();
                    picker.refresh_matches();
                    picker.selected = 0;
                    picker.scroll_to_selected();
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn apply_pane_theme(
        &mut self,
        action: &ApplyPaneTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active_pane_theme(Some(action.name.clone()), cx);
    }

    pub(crate) fn reset_pane_theme(
        &mut self,
        _: &ResetPaneTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active_pane_theme(None, cx);
    }

    /// Applies `theme_name` to the active pane's terminal view only: it never
    /// touches `pane.profile`, `self.profiles`, or the configuration file, so
    /// the change is lost on tab close or the next configuration reload.
    /// `None` restores whatever theme the pane's profile is configured with.
    /// Shared by the interactive picker and the `panetheme` CLI command.
    pub(crate) fn set_active_pane_theme(
        &mut self,
        theme_name: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((profile, view)) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone().map(|view| (pane.profile.clone(), view)))
        else {
            return false;
        };
        let theme = match theme_name {
            Some(name) => match ThemeRegistry::global(cx).get(&name) {
                Ok(theme) => Some(with_zetta_theme_overrides(theme)),
                Err(_) => return false,
            },
            None => resolve_profile_theme(&profile, cx).ok().flatten(),
        };
        view.update(cx, |view, cx| view.set_theme(theme, cx));
        cx.notify();
        true
    }
}
