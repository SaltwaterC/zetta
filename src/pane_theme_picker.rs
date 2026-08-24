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
            .and_then(Tab::active_view)
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| Some(self.window_theme(cx)))
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
        let Some((pane_id, selection, profile, view)) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| {
                pane.selected_view()
                    .map(|view| (pane.id, pane.stack.selected, pane.profile.clone(), view))
            })
        else {
            return false;
        };
        let theme = match theme_name {
            Some(name) => match ThemeRegistry::global(cx).get(&name) {
                Ok(theme) => {
                    self.transient_pane_themes
                        .insert((pane_id, selection), theme.clone());
                    Some(theme)
                }
                Err(_) => return false,
            },
            None => {
                self.transient_pane_themes.remove(&(pane_id, selection));
                resolve_project_profile_theme(
                    &profile,
                    self.active_project_config().map(Arc::as_ref),
                    cx,
                )
                .ok()
                .flatten()
            }
        };
        view.update(cx, |view, cx| view.set_theme(theme, cx));
        cx.notify();
        true
    }

    /// The centred pane theme picker, backdrop included.
    pub(crate) fn render_pane_theme_picker_overlay(
        &self,
        colors: &ThemeColors,
        handle: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.theme_picker.as_ref()?;
        let cursor = picker.cursor.min(picker.query.len());
        let (query_before, query_after) = picker.query.split_at(cursor);
        let query_before = query_before.to_owned();
        let query_after = query_after.to_owned();
        let query_empty = picker.query.is_empty();
        let query_selected = picker.select_all;
        let result_count = picker.matches().len();
        let row_handle = handle.clone();
        let row_colors = colors.clone();
        let rows = uniform_list(
            "theme-picker-list",
            result_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                let Some(picker) = this.theme_picker.as_ref() else {
                    return Vec::new();
                };
                let current_name = this.theme_picker_current.clone();
                range
                    .map(|position| {
                        let command_index = picker.matches()[position];
                        let command = &picker.commands[command_index];
                        let command_name = command.name.clone();
                        let is_current = current_name.as_deref() == Some(command.name.as_str());
                        let row_handle = row_handle.clone();
                        div()
                            .id(("theme-picker-row", command_index))
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
                            .when(position == picker.selected, |row| {
                                row.bg(row_colors.element_selected)
                            })
                            .hover(|style| style.bg(row_colors.element_hover))
                            .on_click(move |_, window, cx| {
                                row_handle
                                    .update(cx, |this, cx| {
                                        this.run_pane_theme_picker_command(
                                            command_index,
                                            window,
                                            cx,
                                        )
                                    })
                                    .ok();
                            })
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_2()
                                    .when(is_current, |row| {
                                        row.child(
                                            Icon::new(IconName::Check)
                                                .size(IconSize::Small)
                                                .color(Color::Custom(row_colors.text_accent)),
                                        )
                                    })
                                    .when(!is_current, |row| row.child(div().w_4()))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(command_name),
                                    ),
                            )
                    })
                    .collect()
            }),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .max_h(px(360.))
        .track_scroll(&picker.scroll)
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        let dismiss_handle = handle.clone();

        Some(
            div()
                .id("theme-picker-backdrop")
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
                        .update(cx, |this, cx| this.dismiss_pane_theme_picker(window, cx))
                        .ok();
                })
                .child(
                    div()
                        .id("theme-picker")
                        .track_focus(&self.theme_picker_focus)
                        .w_full()
                        .max_w(px(680.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .text_color(colors.text)
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
                                .child(div().text_color(colors.text_accent).mr_2().child("◑"))
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
                                                    .child("Search pane themes"),
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
                                            .child("No matching themes"),
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
                                .child("Change is not saved to the profile or configuration"),
                        ),
                )
                .into_any_element(),
        )
    }
}
