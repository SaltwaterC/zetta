use super::*;

impl Zetta {
    pub(crate) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.multi_command.is_some() {
            self.dismiss_multi_command(window, cx);
        }
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        if self.command_palette.is_some() {
            self.dismiss_command_palette(window, cx);
            return;
        }

        let terminal_focus = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.as_ref())
            .map(|view| view.focus_handle(cx));
        let mut commands = window
            .available_actions(cx)
            .into_iter()
            .filter(|action| action.name() != ToggleCommandPalette.name())
            .filter(|action| action.name() != ApplyPaneSplitTemplate::name_for_type())
            .map(|action| {
                let shortcut = terminal_focus
                    .as_ref()
                    .and_then(|focus| {
                        window.highest_precedence_binding_for_action_in(action.as_ref(), focus)
                    })
                    .map(|binding| {
                        binding
                            .keystrokes()
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                PaletteCommand {
                    name: humanize_action_name(action.name()),
                    shortcut,
                    action,
                }
            })
            .collect::<Vec<_>>();
        commands.extend(self.launch_config.pane_split_templates.keys().map(|name| {
            let action = ApplyPaneSplitTemplate { name: name.clone() };
            let shortcut = terminal_focus
                .as_ref()
                .and_then(|focus| window.highest_precedence_binding_for_action_in(&action, focus))
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                });
            PaletteCommand {
                name: format!("zetta: apply pane split template: {name}"),
                shortcut,
                action: Box::new(action),
            }
        }));
        self.command_palette = Some(CommandPalette::new(commands));
        self.command_palette_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn run_palette_command(
        &mut self,
        command_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = self
            .command_palette
            .as_ref()
            .and_then(|palette| palette.commands.get(command_index))
            .map(|command| command.action.boxed_clone());
        self.command_palette = None;
        self.focus_active(window, cx);
        if let Some(action) = action {
            window.dispatch_action(action, cx);
        }
        cx.notify();
    }

    pub(crate) fn command_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.serial_console_key_down(event, window, cx) {
            return;
        }
        if self.settings_editor.is_some() {
            self.settings_key_down(event, window, cx);
            return;
        }
        if self.multi_command.is_some() {
            self.multi_command_key_down(event, window, cx);
            return;
        }
        if self.tab_search.is_some() {
            self.tab_search_key_down(event, window, cx);
            return;
        }
        let Some(palette) = self.command_palette.as_mut() else {
            self.rename_key_down(event, window, cx);
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_command_palette(window, cx),
            "up" => {
                palette.selected = palette.selected.saturating_sub(1);
                cx.notify();
            }
            "down" => {
                palette.selected =
                    (palette.selected + 1).min(palette.matches().len().saturating_sub(1));
                cx.notify();
            }
            "enter" => {
                let command = palette.matches().get(palette.selected).copied();
                if let Some(command) = command {
                    self.run_palette_command(command, window, cx);
                }
            }
            "backspace" => {
                if palette.select_all {
                    palette.query.clear();
                    palette.cursor = 0;
                    palette.refresh_matches();
                } else if palette.cursor > 0 {
                    let previous = previous_char_boundary(&palette.query, palette.cursor);
                    palette.query.replace_range(previous..palette.cursor, "");
                    palette.cursor = previous;
                    palette.refresh_matches();
                }
                palette.select_all = false;
                palette.selected = 0;
                cx.notify();
            }
            "delete" => {
                if palette.select_all {
                    palette.query.clear();
                    palette.cursor = 0;
                    palette.refresh_matches();
                } else if palette.cursor < palette.query.len() {
                    let next = next_char_boundary(&palette.query, palette.cursor);
                    palette.query.replace_range(palette.cursor..next, "");
                    palette.refresh_matches();
                }
                palette.select_all = false;
                palette.selected = 0;
                cx.notify();
            }
            "left" => {
                palette.cursor = if palette.select_all {
                    0
                } else {
                    previous_char_boundary(&palette.query, palette.cursor)
                };
                palette.select_all = false;
                cx.notify();
            }
            "right" => {
                palette.cursor = if palette.select_all {
                    palette.query.len()
                } else {
                    next_char_boundary(&palette.query, palette.cursor)
                };
                palette.select_all = false;
                cx.notify();
            }
            "home" => {
                palette.cursor = 0;
                palette.select_all = false;
                cx.notify();
            }
            "end" => {
                palette.cursor = palette.query.len();
                palette.select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                palette.select_all = !palette.query.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if palette.select_all {
                        palette.query.clear();
                        palette.cursor = 0;
                        palette.select_all = false;
                    }
                    palette.query.insert_str(palette.cursor, text);
                    palette.cursor += text.len();
                    palette.refresh_matches();
                    palette.selected = 0;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(buffer) = tab.rename_buffer.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "enter" => {
                let title = buffer.trim().to_string();
                let title = (!title.is_empty()).then_some(title);
                if let Some(pane_id) = tab.renaming_pane.take() {
                    if let Some(pane) = tab.pane_mut(pane_id) {
                        pane.custom_label = title;
                    }
                } else {
                    tab.custom_title = title;
                }
                tab.rename_buffer = None;
                tab.rename_select_all = false;
                self.focus_active(window, cx);
            }
            "escape" => {
                tab.renaming_pane = None;
                tab.rename_buffer = None;
                tab.rename_select_all = false;
                self.focus_active(window, cx);
            }
            "backspace" => {
                if tab.rename_select_all {
                    buffer.clear();
                    tab.rename_cursor = 0;
                    tab.rename_select_all = false;
                } else if tab.rename_cursor > 0 {
                    let previous = previous_char_boundary(buffer, tab.rename_cursor);
                    buffer.replace_range(previous..tab.rename_cursor, "");
                    tab.rename_cursor = previous;
                }
                cx.notify();
            }
            "delete" => {
                if tab.rename_select_all {
                    buffer.clear();
                    tab.rename_cursor = 0;
                    tab.rename_select_all = false;
                } else if tab.rename_cursor < buffer.len() {
                    let next = next_char_boundary(buffer, tab.rename_cursor);
                    buffer.replace_range(tab.rename_cursor..next, "");
                }
                cx.notify();
            }
            "left" => {
                tab.rename_cursor = if tab.rename_select_all {
                    0
                } else {
                    previous_char_boundary(buffer, tab.rename_cursor)
                };
                tab.rename_select_all = false;
                cx.notify();
            }
            "right" => {
                tab.rename_cursor = if tab.rename_select_all {
                    buffer.len()
                } else {
                    next_char_boundary(buffer, tab.rename_cursor)
                };
                tab.rename_select_all = false;
                cx.notify();
            }
            "home" => {
                tab.rename_cursor = 0;
                tab.rename_select_all = false;
                cx.notify();
            }
            "end" => {
                tab.rename_cursor = buffer.len();
                tab.rename_select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                tab.rename_select_all = !buffer.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if tab.rename_select_all {
                        buffer.clear();
                        tab.rename_cursor = 0;
                        tab.rename_select_all = false;
                    }
                    buffer.insert_str(tab.rename_cursor, text);
                    tab.rename_cursor += text.len();
                    cx.notify();
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }
}
