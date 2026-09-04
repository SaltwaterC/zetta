use super::*;

/// The picker's list for `scope`: the reset entry pinned first, then every
/// installed theme once, in name order.
///
/// Takes the names rather than reading the registry so the list's own rules —
/// the pin, the sort, the dedup, and landing the selection on whatever theme is
/// currently showing — are decided somewhere a test can reach.
fn theme_picker_palette(
    scope: ThemeScope,
    mut theme_names: Vec<String>,
    current: Option<&str>,
) -> CommandPalette {
    theme_names.sort();
    theme_names.dedup();
    let reset_command = PaletteCommand {
        name: match scope {
            ThemeScope::Pane => "Reset to tab default",
            ThemeScope::Tab => "Reset to configured theme",
        }
        .to_owned(),
        shortcut: None,
        action: Box::new(ResetTheme { scope }),
    };
    let theme_commands = theme_names
        .into_iter()
        .map(|name| PaletteCommand {
            name: name.clone(),
            shortcut: None,
            action: Box::new(ApplyTheme { name, scope }),
        })
        .collect();
    let mut picker = CommandPalette::with_pinned_first(reset_command, theme_commands);
    if let Some(index) = current.and_then(|current| {
        picker
            .commands
            .iter()
            .position(|command| command.name == current)
    }) {
        picker.selected = index;
    }
    picker
}

impl Zetta {
    pub(crate) fn change_pane_theme(
        &mut self,
        _: &ChangePaneTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_theme_picker(ThemeScope::Pane, window, cx);
    }

    pub(crate) fn change_tab_theme(
        &mut self,
        _: &ChangeTabTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_theme_picker(ThemeScope::Tab, window, cx);
    }

    pub(crate) fn open_theme_picker(
        &mut self,
        scope: ThemeScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Falls back to the global theme when the pane has neither an
        // override nor a profile theme, so the tick mark always lands on
        // whichever theme is actually showing, not just an active override.
        let current_theme_name = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| match scope {
                ThemeScope::Pane => tab
                    .active_view()
                    .and_then(|view| view.read(cx).theme().cloned()),
                ThemeScope::Tab => Some(self.theme_for_tab(tab, cx)),
            })
            .or_else(|| Some(self.window_theme(cx)))
            .map(|theme| theme.name.to_string());
        let theme_names = ThemeRegistry::global(cx)
            .list()
            .into_iter()
            .map(|theme| theme.name.to_string())
            .collect::<Vec<_>>();
        let picker = theme_picker_palette(scope, theme_names, current_theme_name.as_deref());
        picker.scroll_to_selected();
        self.theme_picker_scope = scope;
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
        // Settled before this picker's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        match apply_clipboard_shortcut(&mut picker.query, &event.keystroke, cx) {
            ClipboardOutcome::Ignored => {}
            ClipboardOutcome::Unchanged => {
                cx.notify();
                return;
            }
            ClipboardOutcome::Edited => {
                // The query filters the theme list, so a cut or a paste rebuilds
                // it rather than only redrawing.
                picker.refresh_matches();
                picker.selected = 0;
                cx.notify();
                return;
            }
        }
        if event.keystroke.key == "escape" {
            self.dismiss_pane_theme_picker(window, cx);
            return;
        }
        match picker.apply_key(&event.keystroke) {
            PaletteKey::Ignored => {}
            PaletteKey::Redraw => cx.notify(),
            PaletteKey::Accept(command) => {
                self.run_pane_theme_picker_command(command, window, cx);
            }
        }
    }

    pub(crate) fn apply_theme(
        &mut self,
        action: &ApplyTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme(action.scope, Some(action.name.clone()), cx);
    }

    pub(crate) fn reset_theme(
        &mut self,
        action: &ResetTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme(action.scope, None, cx);
    }

    /// Keep the pre-scoped action usable for keymaps that already refer to it;
    /// the picker itself dispatches [`ResetTheme`] so its scope is explicit.
    pub(crate) fn reset_pane_theme(
        &mut self,
        _: &ResetPaneTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme(ThemeScope::Pane, None, cx);
    }

    pub(crate) fn reset_tab_theme(
        &mut self,
        _: &ResetTabTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_theme(ThemeScope::Tab, None, cx);
    }

    /// Applies a session-scoped theme to the active pane or tab. It never
    /// touches profiles or the configuration file. The choice follows the
    /// pane/tab through session transfers and is cleared by reset, close, or
    /// the next configuration reload.
    pub(crate) fn set_theme(
        &mut self,
        scope: ThemeScope,
        theme_name: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if scope == ThemeScope::Tab {
            let Some(tab_id) = self.tabs.get(self.active_tab).map(|tab| tab.id) else {
                return false;
            };
            if let Some(name) = theme_name.as_deref()
                && ThemeRegistry::global(cx).get(name).is_err()
            {
                return false;
            }
            self.tabs[self.active_tab].theme_override = theme_name;
            self.refresh_terminal_themes_in_tab(tab_id, cx);
            cx.notify();
            return true;
        }

        let Some((pane_id, selection, project_pane_id, profile, view)) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| {
                let (project_pane_id, profile) = match pane.stack.selected {
                    PaneStackSelection::Base => (pane.id, pane.profile.clone()),
                    PaneStackSelection::Stacked(id) => (
                        id,
                        pane.stack
                            .entries
                            .iter()
                            .find(|entry| entry.id == id)?
                            .profile
                            .clone(),
                    ),
                };
                pane.selected_view()
                    .map(|view| (pane.id, pane.stack.selected, project_pane_id, profile, view))
            })
        else {
            return false;
        };
        let theme = match theme_name {
            Some(name) => match ThemeRegistry::global(cx).get(&name) {
                Ok(theme) => {
                    let pane = self.tabs[self.active_tab]
                        .pane_mut(pane_id)
                        .expect("the active pane still exists");
                    match selection {
                        PaneStackSelection::Base => pane.theme_override = Some(name),
                        PaneStackSelection::Stacked(id) => {
                            let Some(entry) =
                                pane.stack.entries.iter_mut().find(|entry| entry.id == id)
                            else {
                                return false;
                            };
                            entry.theme_override = Some(name);
                        }
                    }
                    Some(theme)
                }
                Err(_) => return false,
            },
            None => {
                let pane = self.tabs[self.active_tab]
                    .pane_mut(pane_id)
                    .expect("the active pane still exists");
                match selection {
                    PaneStackSelection::Base => pane.theme_override = None,
                    PaneStackSelection::Stacked(id) => {
                        let Some(entry) =
                            pane.stack.entries.iter_mut().find(|entry| entry.id == id)
                        else {
                            return false;
                        };
                        entry.theme_override = None;
                    }
                }
                resolve_terminal_theme(
                    None,
                    self.tabs[self.active_tab].theme_override.as_deref(),
                    &profile,
                    self.projects
                        .config_for_pane(project_pane_id)
                        .map(Arc::as_ref),
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
        let theme_scope = self.theme_picker_scope;
        let search_placeholder = match theme_scope {
            ThemeScope::Pane => "Search pane themes",
            ThemeScope::Tab => "Search tab themes",
        };
        let query = field_query_run(&picker.query, Some(search_placeholder), colors);
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
                                        );
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
                                .child(query),
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

#[cfg(test)]
#[path = "tests/pane_theme_picker.rs"]
mod tests;
