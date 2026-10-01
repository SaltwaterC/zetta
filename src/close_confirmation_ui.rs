use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseConfirmationAction {
    Dismiss,
    Confirm,
    Ignore,
}

fn close_confirmation_action(key: &str) -> CloseConfirmationAction {
    match key {
        "escape" => CloseConfirmationAction::Dismiss,
        "enter" => CloseConfirmationAction::Confirm,
        _ => CloseConfirmationAction::Ignore,
    }
}

fn close_confirmation_targets_tab(confirmation: &CloseTabConfirmation, tab_id: u64) -> bool {
    confirmation.tab_id == tab_id
}

impl Zetta {
    pub(crate) fn prompt_to_confirm_tab_close(
        &mut self,
        tab_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_tab_confirmation.is_some() {
            return;
        }
        if !self.tabs.iter().any(|tab| tab.id == tab_id && tab.pinned) {
            return;
        }
        self.command_palette = None;
        self.multi_command = None;
        self.tab_search = None;
        self.settings_editor = None;
        #[cfg(feature = "serial-console")]
        {
            self.serial_console = None;
        }
        self.close_tab_confirmation = Some(CloseTabConfirmation { tab_id });
        self.close_confirmation_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_tab_close_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_tab_confirmation.take().is_some() {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn confirm_tab_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirmation) = self.close_tab_confirmation.take() else {
            return;
        };
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| close_confirmation_targets_tab(&confirmation, tab.id) && tab.pinned)
        else {
            self.focus_active(window, cx);
            cx.notify();
            return;
        };
        self.close_tab_at(index, window, cx);
    }

    pub(crate) fn close_confirmation_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.close_tab_confirmation.is_none() {
            return false;
        }
        match close_confirmation_action(event.keystroke.key.as_str()) {
            CloseConfirmationAction::Dismiss => self.dismiss_tab_close_confirmation(window, cx),
            CloseConfirmationAction::Confirm => self.confirm_tab_close(window, cx),
            CloseConfirmationAction::Ignore => {}
        }
        true
    }

    pub(crate) fn render_tab_close_confirmation_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirmation = self.close_tab_confirmation.as_ref()?;
        let colors = self.window_theme(cx).colors().clone();
        let tab = self.tabs.iter().find(|tab| tab.id == confirmation.tab_id);
        let title = tab.map_or_else(
            || "this tab".into(),
            |tab| tab_overflow_entry_label(tab, cx),
        );
        let backgrounded = tab.is_some_and(|tab| {
            tab.shared || tab.close_policy.background_authentication().is_some()
        });
        let handle = cx.entity().downgrade();
        let cancel_handle = handle.clone();
        let confirm_handle = handle;
        let panel = dialog_panel("tab-close-confirmation", DIALOG_WIDTH_SMALL, &colors)
            .child(dialog_title("Close pinned tab?", &colors))
            .child(div().text_sm().text_color(colors.text_muted).child(format!(
                "Close {title}? This tab will leave the pinned tab bar.{}",
                if backgrounded {
                    " Its session will continue running in the background."
                } else {
                    ""
                }
            )))
            .child(hint_line(
                key_hints(&[("Enter", "close the tab"), ("Esc", "cancel")]),
                &colors,
            ))
            .child(
                dialog_buttons()
                    .child(
                        DialogButton::new("cancel-tab-close", "Cancel", ButtonRole::Secondary)
                            .key_tooltip("Keep the tab open", SurfaceKey::Escape)
                            .render(&colors, move |_, window, cx| {
                                cancel_handle
                                    .update(cx, |this, cx| {
                                        this.dismiss_tab_close_confirmation(window, cx);
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        DialogButton::new(
                            "confirm-tab-close",
                            "Close tab",
                            ButtonRole::Destructive,
                        )
                        .key_tooltip("Close the tab", SurfaceKey::Enter)
                        .render(&colors, move |_, window, cx| {
                            confirm_handle
                                .update(cx, |this, cx| this.confirm_tab_close(window, cx))
                                .ok();
                        }),
                    ),
            );

        Some(modal(
            modal_backdrop(
                "tab-close-confirmation-overlay",
                Placement::Centered,
                BackdropClick::Swallow,
            )
            .track_focus(&self.close_confirmation_focus),
            panel,
        ))
    }
}

#[cfg(test)]
#[path = "tests/close_confirmation_ui.rs"]
mod tests;
