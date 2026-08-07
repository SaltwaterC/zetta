use super::*;

impl Zetta {
    pub(crate) fn toggle_maximize_pane_by_id(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.toggle_maximize(pane_id) {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_maximize_pane(
        &mut self,
        _: &ToggleMaximizePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.toggle_maximize_pane_by_id(pane_id, window, cx);
    }

    pub(crate) fn minimize_pane_by_id(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.minimize(pane_id) {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn minimize_pane(
        &mut self,
        _: &MinimizePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.minimize_pane_by_id(pane_id, window, cx);
    }

    pub(crate) fn restore_minimized_pane_by_id(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.restore_minimized(pane_id) {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn restore_minimized_pane(
        &mut self,
        _: &RestoreMinimizedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.restore_last_minimized() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn select_previous_minimized_pane(
        &mut self,
        _: &SelectPreviousMinimizedPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .tabs
            .get_mut(self.active_tab)
            .is_some_and(Tab::select_previous_minimized);
        if selected {
            cx.notify();
        }
    }

    pub(crate) fn select_next_minimized_pane(
        &mut self,
        _: &SelectNextMinimizedPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .tabs
            .get_mut(self.active_tab)
            .is_some_and(Tab::select_next_minimized);
        if selected {
            cx.notify();
        }
    }

    pub(crate) fn focus_pane_left(
        &mut self,
        _: &FocusPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Left, window, cx);
    }

    pub(crate) fn focus_pane_right(
        &mut self,
        _: &FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Right, window, cx);
    }

    pub(crate) fn focus_pane_up(
        &mut self,
        _: &FocusPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Up, window, cx);
    }

    pub(crate) fn focus_pane_down(
        &mut self,
        _: &FocusPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneDirection::Down, window, cx);
    }

    pub(crate) fn increase_terminal_font_size(
        &mut self,
        _: &IncreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::increase_buffer_font_size(cx);
    }

    pub(crate) fn decrease_terminal_font_size(
        &mut self,
        _: &DecreaseTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::decrease_buffer_font_size(cx);
    }

    pub(crate) fn reset_terminal_font_size(
        &mut self,
        _: &ResetTerminalFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        theme_settings::reset_buffer_font_size(cx);
    }

    pub(crate) fn increase_pane_font_size(
        &mut self,
        _: &IncreasePaneFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone())
        {
            view.update(cx, TerminalView::increase_font_size);
        }
    }

    pub(crate) fn decrease_pane_font_size(
        &mut self,
        _: &DecreasePaneFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone())
        {
            view.update(cx, TerminalView::decrease_font_size);
        }
    }

    pub(crate) fn reset_pane_font_size(
        &mut self,
        _: &ResetPaneFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(|pane| pane.view.clone())
        {
            view.update(cx, TerminalView::reset_font_size);
        }
    }
}
