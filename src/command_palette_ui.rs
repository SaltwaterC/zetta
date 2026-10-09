use super::*;
use crate::command_palette::action_available_in_launch_mode;
use crate::rename::set_tab_title;

/// The shortcut the effective keymap binds `action` to where the terminal has
/// focus, resolved when the palette opens so a remapped action shows its new
/// keys.
fn palette_shortcut(
    window: &Window,
    focus: Option<&gpui::FocusHandle>,
    action: &dyn Action,
) -> Option<std::rc::Rc<[gpui::KeybindingKeystroke]>> {
    focus
        .and_then(|focus| window.highest_precedence_binding_for_action_in(action, focus))
        .map(|binding| binding.keystrokes().to_vec().into())
}

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
        if self.is_picking_overlay_style() {
            self.cancel_overlay_style_picker(window, cx);
        }
        if self.command_palette.is_some() {
            self.dismiss_command_palette(window, cx);
            return;
        }

        let terminal_focus = self.active_terminal_focus(cx);
        let shortcut = |window: &Window, action: &dyn Action| {
            palette_shortcut(window, terminal_focus.as_ref(), action)
        };
        let actions = window
            .available_actions(cx)
            .into_iter()
            .filter(|action| action_is_enabled_in_build(action.name()))
            .filter(|action| action_available_in_launch_mode(action.name(), self.no_mux))
            .filter(|action| action.name() != ToggleCommandPalette.name())
            .filter(|action| action.name() != ApplyPaneSplitTemplate::name_for_type())
            // Actions the palette offers even where the focused element does
            // not register them, because they act on the tab or pane rather
            // than on what has focus.
            .chain([
                Box::new(ChangeTabIcon) as Box<dyn Action>,
                Box::new(ToggleTabPinning),
                Box::new(ChangePaneTheme),
                Box::new(ChangeTabTheme),
                Box::new(SetPaneOverlay),
            ])
            .collect::<Vec<_>>();
        let mut commands = actions
            .into_iter()
            .map(|action| PaletteCommand {
                name: humanize_action_name(action.name()),
                shortcut: shortcut(window, action.as_ref()),
                action,
            })
            .collect::<Vec<_>>();
        commands.extend(
            self.effective_config()
                .pane_split_templates
                .keys()
                .map(|name| {
                    let action = ApplyPaneSplitTemplate { name: name.clone() };
                    PaletteCommand {
                        name: format!("zetta: apply pane split template: {name}"),
                        shortcut: shortcut(window, &action),
                        action: Box::new(action),
                    }
                }),
        );
        commands.extend(project_palette_commands(self.projects.registry.roots()));
        self.command_palette = Some(CommandPalette::new(commands));
        self.command_palette_focus.focus(window, cx);
        cx.notify();
    }

    /// Rebuilds a visible palette after its project context changes. The
    /// action catalog is context-sensitive, but the user's in-progress search
    /// is not: retain it and keep the same command selected when it still
    /// exists.
    pub(crate) fn refresh_open_command_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(previous) = self.command_palette.take() else {
            return;
        };
        let selected_name = previous
            .matches()
            .get(previous.selected)
            .and_then(|index| previous.commands.get(*index))
            .map(|command| command.name.clone());
        let mut query = previous.query;
        query.cursor = query.cursor.min(query.text.len());

        self.toggle_command_palette(&ToggleCommandPalette, window, cx);
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };
        palette.query = query;
        palette.refresh_matches();
        if let Some(selected_name) = selected_name
            && let Some(selected) = palette
                .matches()
                .iter()
                .position(|index| palette.commands[*index].name == selected_name)
        {
            palette.selected = selected;
        }
        palette.scroll_to_selected();
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

    /// Routes a key in the capture phase whenever a focus-managed surface is
    /// visible. Terminal views can retain focus briefly while an overlay is
    /// opening or after a window is reactivated, so the modal must claim the
    /// event before the terminal's own capture handler sees it.
    pub(crate) fn modal_key_down_capture(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_active_surface(window, cx) {
            return;
        }
        self.command_palette_key_down(event, window, cx);
        cx.stop_propagation();
    }

    pub(crate) fn command_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.queue_overlay_clipboard(event, window, cx) {
            self.dispatch_overlay_key(event, window, cx);
        }
    }

    pub(crate) fn dispatch_overlay_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_trust_key_down(event, window, cx) {
            return;
        }
        if self.close_confirmation_key_down(event, window, cx) {
            return;
        }
        if self.session_authentication_key_down(event, window, cx) {
            return;
        }
        if self.remote_session_key_down(event, window, cx) {
            return;
        }
        #[cfg(feature = "serial-console")]
        if self.serial_console_key_down(event, window, cx) {
            return;
        }
        if self.is_picking_overlay_style() {
            self.overlay_style_key_down(event, window, cx);
            return;
        }
        if self.theme_picker.is_some() {
            self.theme_picker_key_down(event, window, cx);
            return;
        }
        if self.tab_icon_picker.is_some() {
            self.tab_icon_picker_key_down(event, window, cx);
            return;
        }
        if self.settings_editor.is_some() {
            self.settings_key_down(event, window, cx);
            return;
        }
        if self.tab_search.is_some() {
            self.tab_search_key_down(event, window, cx);
            return;
        }
        if self.multi_command.is_some() {
            self.multi_command_key_down(event, window, cx);
            return;
        }
        if self.command_palette.is_none() {
            if self.is_editing_pane_overlay() {
                self.overlay_key_down(event, window, cx);
            } else {
                self.rename_key_down(event, window, cx);
            }
            return;
        }
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };
        // Settled before the palette's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        match apply_clipboard_shortcut(&mut palette.query, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => {}
            ClipboardOutcome::Unchanged => {
                cx.notify();
                return;
            }
            ClipboardOutcome::Edited => {
                palette.query_edited();
                cx.notify();
                return;
            }
        }
        if event.keystroke.key == "escape" {
            self.dismiss_command_palette(window, cx);
            return;
        }
        match palette.apply_key(&event.keystroke) {
            PaletteKey::Ignored => {}
            PaletteKey::Redraw => cx.notify(),
            PaletteKey::Accept(command) => self.run_palette_command(command, window, cx),
        }
    }

    pub(crate) fn rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(buffer) = tab.rename_buffer.as_mut() else {
            return;
        };
        // Settled before this prompt's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        match apply_clipboard_shortcut(buffer, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => {}
            ClipboardOutcome::Unchanged | ClipboardOutcome::Edited => {
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        let mut committed = false;
        match event.keystroke.key.as_str() {
            "enter" => {
                let title = buffer.text.trim().to_owned();
                let title = (!title.is_empty()).then_some(title);
                if let Some(pane_id) = tab.renaming_pane.take() {
                    if let Some(pane) = tab.pane_mut(pane_id) {
                        pane.custom_label = title;
                    }
                } else {
                    set_tab_title(tab, title);
                }
                tab.rename_buffer = None;
                committed = true;
                self.focus_active(window, cx);
            }
            "escape" => {
                tab.renaming_pane = None;
                tab.rename_buffer = None;
                self.focus_active(window, cx);
            }
            _ => {
                if apply_text_field_key(buffer, &event.keystroke) != TextFieldEdit::Ignored {
                    cx.notify();
                }
            }
        }
        if committed && let Some(tab_id) = tab_id {
            self.sync_shared_tab_state(tab_id, cx);
        }
        cx.stop_propagation();
    }

    pub(crate) fn overlay_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(buffer) = tab.overlay_buffer.as_mut() else {
            return;
        };
        // Settled before this prompt's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        match apply_clipboard_shortcut(buffer, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => {}
            ClipboardOutcome::Unchanged | ClipboardOutcome::Edited => {
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        match event.keystroke.key.as_str() {
            "enter" => {
                let text = buffer.text.trim().to_owned();
                let text = (!text.is_empty()).then_some(text);
                if let Some(pane_id) = tab.editing_overlay_pane.take() {
                    self.commit_overlay_text_then_pick_style(pane_id, text, window, cx);
                    return;
                }
                tab.overlay_buffer = None;
                self.focus_active(window, cx);
            }
            "escape" => {
                tab.editing_overlay_pane = None;
                tab.overlay_buffer = None;
                self.focus_active(window, cx);
            }
            _ => {
                if apply_text_field_key(buffer, &event.keystroke) != TextFieldEdit::Ignored {
                    cx.notify();
                }
            }
        }
        cx.stop_propagation();
    }

    /// Keyboard input for the overlay-style selector. Tab (and shift-Tab)
    /// cycle the section being adjusted; arrow keys adjust within the
    /// section (font size, hue/saturation/brightness, hex digits, colour
    /// presets, and opacity); Enter commits the picker and Escape restores
    /// the previous values.
    pub(crate) fn overlay_style_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(section) = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_ref())
            .map(|picker| picker.section)
        else {
            return;
        };
        let shift = event.keystroke.modifiers.shift;
        match event.keystroke.key.as_str() {
            "escape" => self.cancel_overlay_style_picker(window, cx),
            "enter" => self.apply_overlay_style_picker(window, cx),
            "tab" => self.adjust_overlay_picker_section(if shift { -1 } else { 1 }, cx),
            _ => match section {
                OverlayPickerSection::FontSize => match event.keystroke.key.as_str() {
                    "left" => self.adjust_overlay_font_size(-1, cx),
                    "right" => self.adjust_overlay_font_size(1, cx),
                    "home" => self.set_overlay_font_size(OverlayFontSize::Small, cx),
                    "end" => self.set_overlay_font_size(OverlayFontSize::ExtraExtraExtraLarge, cx),
                    _ => {}
                },
                OverlayPickerSection::Color => match event.keystroke.key.as_str() {
                    "left" if event.keystroke.modifiers.shift => {
                        self.adjust_overlay_hue(-1. / 36., cx);
                    }
                    "right" if event.keystroke.modifiers.shift => {
                        self.adjust_overlay_hue(1. / 36., cx);
                    }
                    "left" => self.adjust_overlay_saturation(-0.05, cx),
                    "right" => self.adjust_overlay_saturation(0.05, cx),
                    "up" => self.adjust_overlay_value(0.05, cx),
                    "down" => self.adjust_overlay_value(-0.05, cx),
                    "backspace" => self.overlay_hex_backspace(cx),
                    _ if event.keystroke.modifiers.control
                        || event.keystroke.modifiers.platform
                        || event.keystroke.modifiers.alt => {}
                    _ => {
                        if let Some(ch) = event.keystroke.key_char.as_ref() {
                            for ch in ch.chars().take(2) {
                                self.overlay_hex_input(ch, cx);
                            }
                        }
                    }
                },
                OverlayPickerSection::ColorPresets => match event.keystroke.key.as_str() {
                    "left" => self.adjust_overlay_color_preset(0, -1, cx),
                    "right" => self.adjust_overlay_color_preset(0, 1, cx),
                    "up" => self.adjust_overlay_color_preset(-1, 0, cx),
                    "down" => self.adjust_overlay_color_preset(1, 0, cx),
                    "home" => self.set_overlay_color_preset_index(0, cx),
                    "end" => self.set_overlay_color_preset_index(
                        OVERLAY_COLOR_PRESETS.len().saturating_sub(1),
                        cx,
                    ),
                    _ => {}
                },
                OverlayPickerSection::Opacity => match event.keystroke.key.as_str() {
                    "left" | "down" => self.adjust_overlay_opacity_percent(-5, cx),
                    "right" | "up" => self.adjust_overlay_opacity_percent(5, cx),
                    "home" => self.set_overlay_opacity_percent(0, cx),
                    "end" => self.set_overlay_opacity_percent(100, cx),
                    _ => {}
                },
            },
        }
        cx.stop_propagation();
    }

    /// The centred command palette, backdrop included.
    pub(crate) fn render_command_palette_overlay(
        &self,
        colors: &ThemeColors,
        top_inset: Pixels,
        handle: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let palette = self.command_palette.as_ref()?;
        let query = field_query_run(&palette.query, Some("Search commands…"), colors);
        let result_count = palette.matches().len();
        let row_handle = handle.clone();
        let row_colors = colors.clone();
        let rows = uniform_list(
            "command-palette-list",
            result_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                let Some(palette) = this.command_palette.as_ref() else {
                    return Vec::new();
                };
                let query_text = palette.query.text.clone();
                range
                    .map(|position| {
                        let command_index = palette.matches()[position];
                        let command = &palette.commands[command_index];
                        let command_name = command.name.clone();
                        let shortcut = command.shortcut.clone();
                        let row_handle = row_handle.clone();
                        let (row, name) = picker_row(
                            ("command-palette-row", command_index),
                            command_name.into(),
                            &query_text,
                            position == palette.selected,
                            &row_colors,
                        );
                        row.on_click(move |_, window, cx| {
                            row_handle
                                .update(cx, |this, cx| {
                                    this.run_palette_command(command_index, window, cx);
                                })
                                .ok();
                        })
                        .child(name)
                        // Drawn as the menus draw theirs, from the binding
                        // itself rather than as the raw `ctrl-shift-p` text.
                        .when_some(shortcut, |row, keystrokes| {
                            row.child(
                                div()
                                    .flex_none()
                                    .child(ui::KeyBinding::from_keystrokes(keystrokes, false)),
                            )
                        })
                    })
                    .collect()
            }),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .max_h(PALETTE_LIST_MAX_HEIGHT)
        .track_scroll(&palette.scroll)
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        let dismiss_handle = handle.clone();
        let backdrop = modal_backdrop(
            "command-palette-backdrop",
            Placement::UnderChrome(top_inset),
            BackdropClick::dismiss(move |window, cx| {
                dismiss_handle
                    .update(cx, |this, cx| this.dismiss_command_palette(window, cx))
                    .ok();
            }),
        );
        let panel = modal_panel("command-palette", colors)
            .track_focus(&self.command_palette_focus)
            .max_w(PALETTE_WIDTH)
            .child(palette_header(">", query, colors))
            .child(
                palette_section(colors)
                    .py_1()
                    .when(result_count == 0, |list| {
                        list.child(empty_list_row("No commands match", colors))
                    })
                    .when(result_count > 0, |list| list.child(rows)),
            )
            .child(palette_footer(colors).child(format!(
                "{result_count} command{}",
                if result_count == 1 { "" } else { "s" }
            )));
        Some(modal(backdrop, panel))
    }
}

#[cfg(test)]
#[path = "tests/command_palette_ui.rs"]
mod tests;
