//! Routing an attention ID to the tab that owns it.
//!
//! An attention ID is the stable identity a shell integration, a `zetta`
//! subcommand or a notification uses to name a tab, and it outlives the tab's
//! position and its layout IDs. Everything that has to find, focus, or report
//! on a tab from outside the window comes through here.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusSurface {
    CloseConfirmation,
    SessionAuthentication,
    RemoteSession,
    SerialConsole,
    OverlayStylePicker,
    ThemePicker,
    TabIconPicker,
    Settings,
    TabSearch,
    MultiCommand,
    CommandPalette,
    InlineEditing,
}

#[derive(Clone, Copy, Debug, Default)]
struct FocusSurfaceAvailability {
    close_confirmation: bool,
    session_authentication: bool,
    remote_session: bool,
    serial_console: bool,
    overlay_style_picker: bool,
    theme_picker: bool,
    tab_icon_picker: bool,
    settings: bool,
    tab_search: bool,
    multi_command: bool,
    command_palette: bool,
    inline_editing: bool,
}

fn focus_surface(available: FocusSurfaceAvailability) -> Option<FocusSurface> {
    if available.close_confirmation {
        Some(FocusSurface::CloseConfirmation)
    } else if available.session_authentication {
        Some(FocusSurface::SessionAuthentication)
    } else if available.remote_session {
        Some(FocusSurface::RemoteSession)
    } else if available.serial_console {
        Some(FocusSurface::SerialConsole)
    } else if available.overlay_style_picker {
        Some(FocusSurface::OverlayStylePicker)
    } else if available.theme_picker {
        Some(FocusSurface::ThemePicker)
    } else if available.tab_icon_picker {
        Some(FocusSurface::TabIconPicker)
    } else if available.settings {
        Some(FocusSurface::Settings)
    } else if available.tab_search {
        Some(FocusSurface::TabSearch)
    } else if available.multi_command {
        Some(FocusSurface::MultiCommand)
    } else if available.command_palette {
        Some(FocusSurface::CommandPalette)
    } else if available.inline_editing {
        Some(FocusSurface::InlineEditing)
    } else {
        None
    }
}

impl Zetta {
    fn active_focus_surface(&self) -> Option<FocusSurface> {
        focus_surface(FocusSurfaceAvailability {
            close_confirmation: self.close_tab_confirmation.is_some(),
            session_authentication: self.session_authentication.is_some(),
            remote_session: self.remote_session_picker.is_some(),
            serial_console: self.serial_console_is_open(),
            overlay_style_picker: self.is_picking_overlay_style(),
            theme_picker: self.theme_picker.is_some(),
            tab_icon_picker: self.tab_icon_picker.is_some(),
            settings: self.settings_editor.is_some(),
            tab_search: self.tab_search.is_some(),
            multi_command: self.multi_command.is_some(),
            command_palette: self.command_palette.is_some(),
            inline_editing: self.is_renaming() || self.is_editing_pane_overlay(),
        })
    }

    /// Focuses the highest visible focus-managed surface, if there is one.
    ///
    /// The order mirrors the deferred overlay paint order. It is deliberately
    /// centralized because focus can be restored by window activation, a
    /// terminal finishing its spawn, or a prompt dismissing itself.
    pub(crate) fn focus_active_surface(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(surface) = self.active_focus_surface() else {
            return false;
        };
        match surface {
            FocusSurface::CloseConfirmation => self.close_confirmation_focus.focus(window, cx),
            FocusSurface::SessionAuthentication => {
                self.session_authentication_focus.focus(window, cx);
            }
            FocusSurface::RemoteSession => self.remote_session_focus.focus(window, cx),
            #[cfg(feature = "serial-console")]
            FocusSurface::SerialConsole => self.serial_console_focus.focus(window, cx),
            #[cfg(not(feature = "serial-console"))]
            FocusSurface::SerialConsole => unreachable!("serial console is disabled"),
            FocusSurface::OverlayStylePicker => self.overlay_style_focus.focus(window, cx),
            FocusSurface::ThemePicker => self.theme_picker_focus.focus(window, cx),
            FocusSurface::TabIconPicker => self.tab_icon_picker_focus.focus(window, cx),
            FocusSurface::Settings => self.settings_focus.focus(window, cx),
            FocusSurface::TabSearch => self.tab_search_focus.focus(window, cx),
            FocusSurface::MultiCommand => self.multi_command_focus.focus(window, cx),
            FocusSurface::CommandPalette => self.command_palette_focus.focus(window, cx),
            FocusSurface::InlineEditing => self.rename_focus.focus(window, cx),
        }
        true
    }

    /// Gives focus to an automatically selected terminal only when no modal
    /// surface is open. If one is open, restore that surface instead so a late
    /// spawn or byte-stream pane cannot steal its keyboard input.
    pub(crate) fn focus_terminal_if_allowed(
        &self,
        terminal_focus: &gpui::FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.focus_active_surface(window, cx) {
            return false;
        }
        terminal_focus.focus(window, cx);
        true
    }

    pub(super) fn focus_after_window_activation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_active(window, cx);
    }

    pub(crate) fn has_visible_tab_by_attention_id(&self, attention_id: u64) -> bool {
        self.tabs.iter().any(|tab| tab.attention_id == attention_id)
    }

    pub(crate) fn has_tab_by_attention_id(&self, attention_id: u64) -> bool {
        self.has_visible_tab_by_attention_id(attention_id)
            || self
                .background_sessions
                .iter()
                .any(|tab| tab.attention_id == attention_id)
    }

    pub(crate) fn pane_theme_by_attention_id(
        &self,
        attention_id: u64,
        pane_id: Option<u64>,
        cx: &App,
    ) -> Option<String> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.attention_id == attention_id)?;
        let (pane, selection, profile, project_pane_id, view) = match pane_id {
            Some(routing_id) => tab.panes.iter().find_map(|pane| {
                if pane.routing_id == routing_id {
                    return Some((
                        pane,
                        PaneStackSelection::Base,
                        &pane.profile,
                        pane.id,
                        pane.view.clone(),
                    ));
                }
                pane.stack.entries.iter().find_map(|entry| {
                    (entry.routing_id == routing_id).then(|| {
                        (
                            pane,
                            PaneStackSelection::Stacked(entry.id),
                            &entry.profile,
                            entry.id,
                            entry.view.clone(),
                        )
                    })
                })
            })?,
            None => {
                let pane = tab.active_pane()?;
                match pane.stack.selected {
                    PaneStackSelection::Base => (
                        pane,
                        PaneStackSelection::Base,
                        &pane.profile,
                        pane.id,
                        pane.view.clone(),
                    ),
                    PaneStackSelection::Stacked(entry_id) => {
                        let entry = pane
                            .stack
                            .entries
                            .iter()
                            .find(|entry| entry.id == entry_id)?;
                        (
                            pane,
                            PaneStackSelection::Stacked(entry.id),
                            &entry.profile,
                            entry.id,
                            entry.view.clone(),
                        )
                    }
                }
            }
        };

        let theme = view
            .and_then(|view| view.read(cx).theme().cloned())
            .or_else(|| {
                resolve_terminal_theme(
                    pane.theme_override(selection),
                    tab.theme_override.as_deref(),
                    profile,
                    self.projects
                        .config_for_pane(project_pane_id)
                        .map(Arc::as_ref),
                    cx,
                )
                .ok()
                .flatten()
            })
            .unwrap_or_else(|| self.application_theme(cx));
        Some(theme.name.to_string())
    }

    pub(crate) fn focus_tab_by_attention_id(
        &mut self,
        attention_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab_index) = self
            .tabs
            .iter()
            .position(|tab| tab.attention_id == attention_id)
        else {
            return false;
        };
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        self.active_tab = tab_index;
        self.tab_overflow_selection_side = None;
        self.dismiss_tab_overflow_menus(cx);
        self.focus_active(window, cx);
        true
    }

    pub(crate) fn attention_id_for_tab(&self, tab_id: u64) -> Option<u64> {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.attention_id)
            .or_else(|| {
                self.background_sessions
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .map(|tab| tab.attention_id)
            })
    }

    pub(crate) fn run_pane_identity(
        &self,
        tab_id: u64,
        pane_id: u64,
    ) -> Option<crate::run_command::RunPaneIdentity> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .or_else(|| self.background_sessions.iter().find(|tab| tab.id == tab_id))?;
        let pane = tab.panes.iter().find(|pane| pane.id == pane_id)?;
        Some(crate::run_command::RunPaneIdentity::new(
            tab.attention_id,
            pane.routing_id,
        ))
    }

    pub(crate) fn run_stacked_pane_identity(
        &self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
    ) -> Option<crate::run_command::RunPaneIdentity> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .or_else(|| self.background_sessions.iter().find(|tab| tab.id == tab_id))?;
        let entry = tab
            .pane(pane_id)?
            .stack
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)?;
        Some(crate::run_command::RunPaneIdentity::new(
            tab.attention_id,
            entry.routing_id,
        ))
    }

    fn tab_content_is_focused(
        &self,
        tab_index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get(tab_index) else {
            return false;
        };
        if tab.pane_is_visible(tab.active_pane) {
            tab.active_view().map_or_else(
                || self.terminal_placeholder_focus.is_focused(window),
                |view| view.focus_handle(cx).is_focused(window),
            )
        } else {
            !tab.minimized_panes.is_empty() && self.minimized_panes_focus.is_focused(window)
        }
    }

    pub(crate) fn clear_active_tab_attention_if_focused(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !window.is_window_active() {
            return false;
        }
        let active_tab = self.active_tab;
        if !self.tab_content_is_focused(active_tab, window, cx) {
            return false;
        }
        let cleared = self
            .tabs
            .get_mut(active_tab)
            .and_then(|tab| tab.attention.take())
            .is_some();
        if cleared {
            cx.notify();
        }
        cleared
    }

    pub(crate) fn set_tab_attention(
        &mut self,
        request: TabAttentionRequest,
        window: Option<&Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        let attention = TabAttention {
            summary: request.summary,
            body: request.body,
        };
        if let Some(tab_index) = self
            .tabs
            .iter()
            .position(|tab| tab.attention_id == request.attention_id)
        {
            let should_clear = window.is_some_and(|window| {
                self.active_tab == tab_index && self.tab_content_is_focused(tab_index, window, cx)
            });
            self.tabs[tab_index].attention = (!should_clear).then_some(attention);
            cx.notify();
            return true;
        }
        if let Some(tab) = self
            .background_sessions
            .iter_unprotected_mut()
            .find(|tab| tab.attention_id == request.attention_id)
        {
            tab.attention = Some(attention);
            cx.notify();
            return true;
        }
        false
    }

    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_current_project(window, cx);
        if self.focus_active_surface(window, cx) {
            cx.notify();
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let active_is_visible = tab.pane_is_visible(tab.active_pane);
            if active_is_visible {
                if let Some(view) = tab.active_view() {
                    view.focus_handle(cx).focus(window, cx);
                } else {
                    self.terminal_placeholder_focus.focus(window, cx);
                }
            } else if !tab.minimized_panes.is_empty() {
                self.minimized_panes_focus.focus(window, cx);
            }
        }
        self.clear_active_tab_attention_if_focused(window, cx);
        cx.notify();
    }

    pub(crate) fn active_terminal_focus(&self, cx: &App) -> Option<gpui::FocusHandle> {
        let tab = self.tabs.get(self.active_tab)?;
        if !tab.pane_is_visible(tab.active_pane) {
            return None;
        }
        Some(tab.active_view().map_or_else(
            || self.terminal_placeholder_focus.clone(),
            |view| view.focus_handle(cx),
        ))
    }
}

#[cfg(test)]
#[path = "../tests/app/attention.rs"]
mod tests;
