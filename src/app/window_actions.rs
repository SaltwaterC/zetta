//! The window-level actions — close, minimize, hide, zoom, fullscreen — and
//! keyboard navigation of the application menus in the title bar.

use super::*;

impl Zetta {
    pub(crate) fn close_window(
        &mut self,
        _: &CloseWindow,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.remove_window();
    }

    pub(crate) fn minimize_window(
        &mut self,
        _: &MinimizeWindow,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if window.is_minimizable() {
            window.minimize_window();
        }
    }

    pub(crate) fn hide_window(
        &mut self,
        _: &HideWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cfg!(target_os = "macos") {
            cx.hide();
        } else if window.is_minimizable() {
            window.minimize_window();
        }
    }

    pub(crate) fn zoom_window(
        &mut self,
        _: &ZoomWindow,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if window.is_resizable() {
            window.zoom_window();
        }
    }

    pub(crate) fn toggle_fullscreen(
        &mut self,
        _: &ToggleFullscreen,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if window.window_controls().fullscreen {
            window.toggle_fullscreen();
        }
    }

    pub(crate) fn close_all_windows(
        &mut self,
        _: &CloseAllWindows,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_window_id = window.window_handle().window_id();
        for window_handle in cx.windows() {
            if window_handle.window_id() == current_window_id {
                window.remove_window();
            } else {
                window_handle
                    .update(cx, |_, window, _| window.remove_window())
                    .log_err();
            }
        }
    }

    pub(crate) fn open_application_menu(
        &mut self,
        _: &OpenApplicationMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.application_menu_handle.show(window, cx);
    }

    fn title_bar_menu_handles(&self) -> [PopoverMenuHandle<ui::ContextMenu>; 2] {
        [
            self.application_menu_handle.clone(),
            self.profile_menu_handle.clone(),
        ]
    }

    fn navigate_application_menus(
        &mut self,
        direction: ApplicationMenuDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Keep auto-repeat from starting another handoff before the new menu
        // receives its deferred focus update.
        if self.application_menu_switch_pending {
            return;
        }

        // Keep the navigable menus in title-bar order. Adding a new top-level
        // menu only requires adding its handle here.
        let handles = self.title_bar_menu_handles();
        let Some(current_index) = handles
            .iter()
            .position(|handle| handle.is_focused(window, cx))
        else {
            cx.propagate();
            return;
        };
        let next_index = adjacent_application_menu_index(handles.len(), current_index, direction);

        // A popover restores its previous focus when dismissed. Hiding the
        // current menu before the next one has focus briefly returns focus to
        // the terminal, causing a visible pane redraw and allowing repeated
        // arrow keys to reach it. Open the replacement first, then dismiss
        // the current menu after the replacement's deferred focus update.
        self.application_menu_switch_pending = true;
        let current_handle = handles[current_index].clone();
        let next_handle = handles[next_index].clone();
        let zetta = cx.entity().downgrade();
        next_handle.show(window, cx);
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, cx| {
                current_handle.hide(cx);
                zetta
                    .update(cx, |this, _| this.application_menu_switch_pending = false)
                    .ok();
            });
        });
    }

    pub(crate) fn activate_application_menu_left(
        &mut self,
        _: &ActivateApplicationMenuLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_application_menus(ApplicationMenuDirection::Left, window, cx);
    }

    pub(crate) fn activate_application_menu_right(
        &mut self,
        _: &ActivateApplicationMenuRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_application_menus(ApplicationMenuDirection::Right, window, cx);
    }
}

#[cfg(test)]
#[path = "../tests/app/window_actions.rs"]
mod tests;
