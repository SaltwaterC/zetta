//! One-shot keyboard routing from a window's focused terminal view.
//!
//! The interceptor exists only while a key is armed or held. It runs before
//! action dispatch so a Zetta binding cannot swallow the terminal's key.

use crate::*;

pub(crate) struct KeyPassthrough {
    pub(crate) held_key: Option<String>,
    target: Entity<TerminalView>,
    _interceptor: Subscription,
    _focus_out: Subscription,
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "shift" | "control" | "ctrl" | "alt" | "platform" | "function"
    )
}

#[derive(Debug, PartialEq, Eq)]
enum KeyRoute {
    Ignore,
    Cancel,
    Forward { first_press: bool },
}

fn route_key(held: Option<&str>, key: &str) -> KeyRoute {
    if let Some(held) = held {
        return if held == key {
            KeyRoute::Forward { first_press: false }
        } else {
            KeyRoute::Ignore
        };
    }
    if key == "escape" {
        KeyRoute::Cancel
    } else if is_modifier_key(key) {
        KeyRoute::Ignore
    } else {
        KeyRoute::Forward { first_press: true }
    }
}

impl Zetta {
    pub(crate) fn send_next_key_to_terminal(
        &mut self,
        _: &SendNextKeyToTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .tabs
            .get(self.active_tab)
            .and_then(Tab::active_pane)
            .and_then(TerminalPane::selected_view)
            .filter(|view| {
                view.focus_handle(cx).is_focused(window) && view.read(cx).input_enabled()
            })
        else {
            return;
        };
        self.cancel_key_passthrough(cx);
        let handle = window.window_handle();
        let this = cx.weak_entity();
        let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle {
                return;
            }
            let consumed = this
                .update(cx, |this, cx| {
                    this.intercept_next_key(&event.keystroke, window, cx)
                })
                .unwrap_or(false);
            if consumed {
                cx.stop_propagation();
            }
        });
        let focus = target.focus_handle(cx);
        let focus_out = cx.on_focus_out(&focus, window, |this, _, _, cx| {
            this.cancel_key_passthrough(cx);
        });
        self.key_passthrough = Some(KeyPassthrough {
            held_key: None,
            target,
            _interceptor: interceptor,
            _focus_out: focus_out,
        });
        cx.notify();
    }

    fn intercept_next_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.key_passthrough.as_ref() else {
            return false;
        };
        if !state.target.focus_handle(cx).is_focused(window) {
            self.cancel_key_passthrough(cx);
            return false;
        }
        let route = route_key(state.held_key.as_deref(), &keystroke.key);
        match route {
            KeyRoute::Ignore => return false,
            KeyRoute::Cancel => {
                self.cancel_key_passthrough(cx);
                return true;
            }
            KeyRoute::Forward { .. } => {}
        }
        let target = state.target.clone();
        if route == (KeyRoute::Forward { first_press: true }) {
            if let Some(state) = self.key_passthrough.as_mut() {
                state.held_key = Some(keystroke.key.clone());
            }
            cx.notify();
        }
        target.update(cx, |view, cx| view.forward_keystroke(keystroke, cx));
        true
    }

    pub(crate) fn key_passthrough_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .key_passthrough
            .as_ref()
            .is_some_and(|state| state.held_key.as_deref() == Some(event.keystroke.key.as_str()))
        {
            self.cancel_key_passthrough(cx);
        }
    }

    pub(crate) fn cancel_key_passthrough(&mut self, cx: &mut Context<Self>) {
        if self.key_passthrough.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn cancel_key_passthrough_if_unfocused(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .key_passthrough
            .as_ref()
            .is_some_and(|state| !state.target.focus_handle(cx).is_focused(window))
        {
            self.cancel_key_passthrough(cx);
        }
    }
}

#[cfg(test)]
#[path = "tests/key_passthrough.rs"]
mod tests;
