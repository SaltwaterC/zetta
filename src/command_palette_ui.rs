use super::*;
use crate::rename::set_tab_title;

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
        let mut commands = window
            .available_actions(cx)
            .into_iter()
            .filter(|action| action_is_enabled_in_build(action.name()))
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
        let change_tab_icon = ChangeTabIcon;
        let change_tab_icon_shortcut = terminal_focus
            .as_ref()
            .and_then(|focus| {
                window.highest_precedence_binding_for_action_in(&change_tab_icon, focus)
            })
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        commands.push(PaletteCommand {
            name: humanize_action_name(change_tab_icon.name()),
            shortcut: change_tab_icon_shortcut,
            action: Box::new(change_tab_icon),
        });
        let toggle_tab_pinning = ToggleTabPinning;
        let toggle_tab_pinning_shortcut = terminal_focus
            .as_ref()
            .and_then(|focus| {
                window.highest_precedence_binding_for_action_in(&toggle_tab_pinning, focus)
            })
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        commands.push(PaletteCommand {
            name: humanize_action_name(toggle_tab_pinning.name()),
            shortcut: toggle_tab_pinning_shortcut,
            action: Box::new(toggle_tab_pinning),
        });
        let change_pane_theme = ChangePaneTheme;
        let change_pane_theme_shortcut = terminal_focus
            .as_ref()
            .and_then(|focus| {
                window.highest_precedence_binding_for_action_in(&change_pane_theme, focus)
            })
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        commands.push(PaletteCommand {
            name: humanize_action_name(change_pane_theme.name()),
            shortcut: change_pane_theme_shortcut,
            action: Box::new(change_pane_theme),
        });
        let set_pane_overlay = SetPaneOverlay;
        let set_pane_overlay_shortcut = terminal_focus
            .as_ref()
            .and_then(|focus| {
                window.highest_precedence_binding_for_action_in(&set_pane_overlay, focus)
            })
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        commands.push(PaletteCommand {
            name: humanize_action_name(set_pane_overlay.name()),
            shortcut: set_pane_overlay_shortcut,
            action: Box::new(set_pane_overlay),
        });
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
        if self.close_confirmation_key_down(event, window, cx) {
            return;
        }
        if self.session_authentication_key_down(event, window, cx) {
            return;
        }
        #[cfg(feature = "serial-console")]
        if self.serial_console_key_down(event, window, cx) {
            return;
        }
        if self.tab_icon_picker.is_some() {
            self.tab_icon_picker_key_down(event, window, cx);
            return;
        }
        if self.theme_picker.is_some() {
            self.theme_picker_key_down(event, window, cx);
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
            if self.is_editing_pane_overlay() {
                self.overlay_key_down(event, window, cx);
            } else if self.is_picking_overlay_style() {
                self.overlay_style_key_down(event, window, cx);
            } else {
                self.rename_key_down(event, window, cx);
            }
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_command_palette(window, cx),
            "up" => {
                palette.selected = palette.selected.saturating_sub(1);
                palette.scroll_to_selected();
                cx.notify();
            }
            "down" => {
                palette.selected =
                    (palette.selected + 1).min(palette.matches().len().saturating_sub(1));
                palette.scroll_to_selected();
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
                palette.scroll_to_selected();
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
                palette.scroll_to_selected();
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
                    palette.scroll_to_selected();
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
                    set_tab_title(tab, title);
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
        match event.keystroke.key.as_str() {
            "enter" => {
                let text = buffer.trim().to_string();
                let text = (!text.is_empty()).then_some(text);
                if let Some(pane_id) = tab.editing_overlay_pane.take() {
                    self.commit_overlay_text_then_pick_style(pane_id, text, window, cx);
                    return;
                }
                tab.overlay_buffer = None;
                tab.overlay_select_all = false;
                self.focus_active(window, cx);
            }
            "escape" => {
                tab.editing_overlay_pane = None;
                tab.overlay_buffer = None;
                tab.overlay_select_all = false;
                self.focus_active(window, cx);
            }
            "backspace" => {
                if tab.overlay_select_all {
                    buffer.clear();
                    tab.overlay_cursor = 0;
                    tab.overlay_select_all = false;
                } else if tab.overlay_cursor > 0 {
                    let previous = previous_char_boundary(buffer, tab.overlay_cursor);
                    buffer.replace_range(previous..tab.overlay_cursor, "");
                    tab.overlay_cursor = previous;
                }
                cx.notify();
            }
            "delete" => {
                if tab.overlay_select_all {
                    buffer.clear();
                    tab.overlay_cursor = 0;
                    tab.overlay_select_all = false;
                } else if tab.overlay_cursor < buffer.len() {
                    let next = next_char_boundary(buffer, tab.overlay_cursor);
                    buffer.replace_range(tab.overlay_cursor..next, "");
                }
                cx.notify();
            }
            "left" => {
                tab.overlay_cursor = if tab.overlay_select_all {
                    0
                } else {
                    previous_char_boundary(buffer, tab.overlay_cursor)
                };
                tab.overlay_select_all = false;
                cx.notify();
            }
            "right" => {
                tab.overlay_cursor = if tab.overlay_select_all {
                    buffer.len()
                } else {
                    next_char_boundary(buffer, tab.overlay_cursor)
                };
                tab.overlay_select_all = false;
                cx.notify();
            }
            "home" => {
                tab.overlay_cursor = 0;
                tab.overlay_select_all = false;
                cx.notify();
            }
            "end" => {
                tab.overlay_cursor = buffer.len();
                tab.overlay_select_all = false;
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                tab.overlay_select_all = !buffer.is_empty();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if tab.overlay_select_all {
                        buffer.clear();
                        tab.overlay_cursor = 0;
                        tab.overlay_select_all = false;
                    }
                    buffer.insert_str(tab.overlay_cursor, text);
                    tab.overlay_cursor += text.len();
                    cx.notify();
                }
            }
            _ => {}
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
                        self.adjust_overlay_hue(-1. / 36., cx)
                    }
                    "right" if event.keystroke.modifiers.shift => {
                        self.adjust_overlay_hue(1. / 36., cx)
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
        handle: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let palette = self.command_palette.as_ref()?;
        let cursor = palette.cursor.min(palette.query.len());
        let (query_before, query_after) = palette.query.split_at(cursor);
        let query_before = query_before.to_owned();
        let query_after = query_after.to_owned();
        let query_empty = palette.query.is_empty();
        let query_selected = palette.select_all;
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
                range
                    .map(|position| {
                        let command_index = palette.matches()[position];
                        let command = &palette.commands[command_index];
                        let command_name = command.name.clone();
                        let shortcut = command.shortcut.clone();
                        let row_handle = row_handle.clone();
                        div()
                            .id(("command-palette-row", command_index))
                            .h_9()
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(row_colors.text)
                            .when(position == palette.selected, |row| {
                                row.bg(row_colors.element_selected)
                            })
                            .hover(|style| style.bg(row_colors.element_hover))
                            .on_click(move |_, window, cx| {
                                row_handle
                                    .update(cx, |this, cx| {
                                        this.run_palette_command(command_index, window, cx)
                                    })
                                    .ok();
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(command_name),
                            )
                            .when_some(shortcut, |row, shortcut| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(row_colors.text_muted)
                                        .child(shortcut),
                                )
                            })
                    })
                    .collect()
            }),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .max_h(px(360.))
        .track_scroll(&palette.scroll)
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        let dismiss_handle = handle.clone();

        Some(
            div()
                .id("command-palette-backdrop")
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    dismiss_handle
                        .update(cx, |this, cx| this.dismiss_command_palette(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id("command-palette")
                        .track_focus(&self.command_palette_focus)
                        .w_full()
                        .max_w(px(680.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_12()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(colors.border)
                                .text_color(colors.text)
                                .child(div().text_color(colors.text_accent).mr_2().child(">"))
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(query_selected, |input| {
                                            input.bg(colors.element_selection_background)
                                        })
                                        .child(div().whitespace_nowrap().child(query_before))
                                        .when(!query_selected, |input| {
                                            input.child(
                                                div()
                                                    .flex_none()
                                                    .w(px(1.0))
                                                    .h(px(16.0))
                                                    .bg(colors.text_accent),
                                            )
                                        })
                                        .child(div().whitespace_nowrap().child(query_after))
                                        .when(query_empty, |input| {
                                            input.child(
                                                div()
                                                    .text_color(colors.text_placeholder)
                                                    .child("Type a command"),
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .py_1()
                                .when(result_count == 0, |list| {
                                    list.child(
                                        div()
                                            .h_12()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .text_sm()
                                            .text_color(colors.text_muted)
                                            .child("No matching commands"),
                                    )
                                })
                                .when(result_count > 0, |list| list.child(rows)),
                        )
                        .child(
                            div()
                                .h_7()
                                .px_3()
                                .flex()
                                .items_center()
                                .border_t_1()
                                .border_color(colors.border)
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "{result_count} command{}",
                                    if result_count == 1 { "" } else { "s" }
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}
