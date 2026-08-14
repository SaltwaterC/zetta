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
        let title = tab
            .map(|tab| tab_overflow_entry_label(tab, cx))
            .unwrap_or_else(|| "this tab".into());
        let backgrounded =
            tab.is_some_and(|tab| tab.close_policy.background_authentication().is_some());
        let handle = cx.entity().downgrade();
        let cancel_handle = handle.clone();
        let confirm_handle = handle;
        let panel = div()
            .w(px(420.))
            .max_w(gpui::relative(0.9))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Label::new("Close pinned tab?").size(LabelSize::Large))
            .child(div().text_sm().text_color(colors.text_muted).child(format!(
                "Close {title}? This tab will leave the pinned tab bar.{}",
                if backgrounded {
                    " Its session will continue running in the background."
                } else {
                    ""
                }
            )))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child("Press Enter to close this tab, or Esc to dismiss."),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-tab-close", "Cancel")
                            .style(ButtonStyle::Outlined)
                            .on_click(move |_, window, cx| {
                                cancel_handle
                                    .update(cx, |this, cx| {
                                        this.dismiss_tab_close_confirmation(window, cx)
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        Button::new("confirm-tab-close", "Close tab")
                            .style(ButtonStyle::Filled)
                            .on_click(move |_, window, cx| {
                                confirm_handle
                                    .update(cx, |this, cx| this.confirm_tab_close(window, cx))
                                    .ok();
                            }),
                    ),
            );

        Some(
            div()
                .id("tab-close-confirmation-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .track_focus(&self.close_confirmation_focus)
                .child(panel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "tests/close_confirmation_ui.rs"]
mod tests;
